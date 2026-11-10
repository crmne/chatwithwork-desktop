//! What one run may do, worked out from the config on disk, and the MXC
//! request that holds it to that.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Limits, MAX_COMMAND_CHARS, RunError};
use crate::config::Config;
use crate::paths::Paths;

/// What the worker asks for: a command, in a folder that allows commands,
/// named by its ID, and a folder inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub root: String,
    /// Relative to the folder, `""` for the folder itself.
    #[serde(default)]
    pub dir: String,
    pub command: String,
    /// The Chat with Work bot whose chat runs it, if a bot's: whoever starts
    /// it checks the bot's loan in the config itself before giving it
    /// anything a person's run doesn't get (docs/coding-computers.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    /// Destinations the person allowed for this one command ("Allow once"),
    /// taken from what waited for it in the worker. Placed again by whoever
    /// starts the run, from this computer's own networks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub once: Vec<crate::reach::Grant>,
}

/// The sandbox MXC runs a command in on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    /// Linux, where unprivileged user namespaces work.
    Bubblewrap,
    /// Linux, where they're blocked (Ubuntu's AppArmor restriction, a
    /// container, `user.max_user_namespaces` at 0): MXC falls back to
    /// Landlock and a seccomp filter, from Linux 6.2.
    Landlock,
    /// macOS.
    Seatbelt,
    /// Windows: a process container on the BaseContainer of Windows 11
    /// 24H2 and later (see `windows.rs`).
    ProcessContainer,
}

impl Sandbox {
    /// What this computer can run commands in, or why nothing. Asked for
    /// every run, as MXC does, so allowing namespaces later takes effect.
    pub fn for_host() -> Result<Self, RunError> {
        #[cfg(target_os = "linux")]
        {
            use mxc_sdk::bwrap_common::bwrap_version::probe_bwrap;
            use mxc_sdk::landlock_common::abi;
            let bubblewrap = match probe_bwrap() {
                Ok(_) => return Ok(Self::Bubblewrap),
                Err(e) => e,
            };
            abi::probe().map(|_| Self::Landlock).map_err(|landlock| {
                RunError::new(
                    "unavailable",
                    format!(
                        "commands can't run on this computer. Bubblewrap can't: {bubblewrap} \
                         Landlock can't either: {landlock}"
                    ),
                )
            })
        }
        #[cfg(target_os = "macos")]
        {
            Ok(Self::Seatbelt)
        }
        #[cfg(windows)]
        {
            super::windows::sandbox()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(RunError::new(
                "unavailable",
                "commands can't run on this system yet",
            ))
        }
    }

    /// Its name in `status` and the dry run.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bubblewrap => "bubblewrap",
            Self::Landlock => "landlock",
            Self::Seatbelt => "seatbelt",
            Self::ProcessContainer => "process_container",
        }
    }

    /// Why `plan` can't run in this sandbox, if it can't. Landlock runs a
    /// command in this computer's network and can't tell one address from
    /// another, so it could only allow all of it, this computer's loopback
    /// (its local servers) and connections in included. Bubblewrap gives a
    /// command a network of its own instead.
    pub fn refuses(self, plan: &Plan) -> Option<RunError> {
        (self == Self::Landlock && plan.network_access(self) == crate::reach::Level::Internet).then(
            || {
                RunError::new(
                    "network_unavailable",
                    "this folder allows commands the internet, which this computer can't give them \
                 safely: Bubblewrap can't create a sandbox here, and Landlock, used instead, \
                 could only give the whole network, its local servers included. Turn the \
                 internet off for this folder, or allow unprivileged user namespaces for \
                 Bubblewrap",
                )
            },
        )
    }

    /// Processes in a run's tree, its first one included, that are the
    /// sandbox's rather than the command's. Bubblewrap starts the command
    /// under itself and its init; on Landlock the command's shell is the
    /// first process.
    pub fn own_processes(self) -> usize {
        match self {
            Self::Bubblewrap => 3,
            Self::Landlock | Self::Seatbelt | Self::ProcessContainer => 0,
        }
    }
}

/// A run, checked: everything its sandbox is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub id: String,
    pub command: String,
    /// The shared folder, canonical: the only place it writes, besides
    /// `home`.
    pub folder: PathBuf,
    /// Where it starts, inside `folder`.
    pub cwd: PathBuf,
    /// The folder's "Internet": the internet and the local network, its
    /// local services with them.
    pub network: bool,
    /// The folder's local services: this computer's own (`reach.rs`).
    pub local_services: bool,
    /// Exact local-network addresses the folder allows besides.
    pub reach: Vec<std::net::SocketAddrV4>,
    /// What the person allowed for this command alone.
    pub once: Vec<crate::reach::Grant>,
    /// This computer's networks when the run starts, filled in by its
    /// thread ([`Networks::current`]).
    pub networks: Networks,
    pub limits: Limits,
    /// Its own home and temporary folder, made for it and removed after.
    pub home: PathBuf,
    /// The folder's repository mirrors, and the coding agents for a bot
    /// allowed them. Set by whoever starts the run, after [`plan`].
    pub coding: crate::coding::RunCoding,
    /// The folder's environment (`env.rs`): read by whoever starts the run,
    /// its values worked out by the run's thread before the command starts.
    pub env: super::env::RunEnv,
    /// On Linux, the cgroup that holds it to its memory limit, made by its
    /// thread before it starts (`cgroup.rs`). None: no memory limit.
    pub cgroup: Option<PathBuf>,
    /// The internet only: never this computer's own services, its own
    /// addresses, its containers' networks or the local network, where the
    /// sandbox can tell them apart (Bubblewrap). A thinking step's
    /// (`think.rs`), whose agent only needs its provider.
    pub internet_only: bool,
}

/// Check `request` against `config`: the folder allows commands, the
/// working folder is inside it (after following links), and the command
/// is a single, reasonable line of shell.
pub fn plan(config: &Config, paths: &Paths, request: &Request, id: &str) -> Result<Plan, RunError> {
    let root = config
        .roots
        .iter()
        .find(|r| r.id == request.root)
        .ok_or_else(|| RunError::new("not_found", format!("no shared folder {}", request.root)))?;
    if !root.runs_commands() {
        return Err(RunError::new(
            "not_allowed",
            format!(
                "{} doesn't allow commands. Allow them on this computer to run one there",
                root.label
            ),
        ));
    }
    let command = request.command.trim();
    if command.is_empty() {
        return Err(RunError::new("invalid", "the command is empty"));
    }
    if command.chars().count() > MAX_COMMAND_CHARS || command.contains('\0') {
        return Err(RunError::new(
            "invalid",
            format!("a command is at most {MAX_COMMAND_CHARS} characters, without NUL"),
        ));
    }
    // Canonical, and on Windows without the `\\?\` prefix, which the
    // sandbox's own paths don't carry.
    let folder = crate::roots::canonical(&root.path)
        .map_err(|_| RunError::new("not_found", format!("{} isn't there", root.label)))?;
    let relative = Path::new(&request.dir);
    let plain = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !plain || request.dir.contains('\0') || request.dir.contains('\\') {
        return Err(RunError::new(
            "invalid_path",
            "the working folder must be a relative path inside the shared folder",
        ));
    }
    let cwd = crate::roots::canonical(&folder.join(relative))
        .map_err(|_| RunError::new("not_found", format!("no folder {}", request.dir)))?;
    if !cwd.starts_with(&folder) || !cwd.is_dir() {
        return Err(RunError::new(
            "invalid_path",
            "the working folder must be a folder inside the shared folder",
        ));
    }
    Ok(Plan {
        id: id.to_string(),
        command: command.to_string(),
        folder,
        cwd,
        network: root.network,
        local_services: root.local_services,
        reach: root
            .reach
            .iter()
            .filter_map(|entry| crate::reach::parse_reach(entry).ok())
            .collect(),
        once: request.once.clone(),
        networks: Networks::default(),
        limits: Limits::from_config(&config.limits),
        home: paths.runs_dir().join(id),
        coding: Default::default(),
        env: Default::default(),
        cgroup: None,
        internet_only: false,
    })
}

/// This computer's networks as one run's rules need them, read when it
/// starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Networks {
    pub host: crate::reach::HostNetwork,
    /// TCP ports this computer serves on its loopback, less those a command
    /// never reaches.
    pub loopback: Vec<u16>,
    /// Ports it listens on that a command never reaches on any address
    /// (`reach::closed_ports`), refused on its own addresses and its
    /// containers' networks like `reach::EXCLUDED_PORTS`.
    pub closed: Vec<u16>,
    /// Where the sandbox logs the connections it blocked, when it can
    /// (Bubblewrap with a network of the run's own).
    pub log: Option<PathBuf>,
    /// Whether the run can have a network of its own, which local services
    /// need on Linux (Bubblewrap with slirp4netns and unprivileged network
    /// namespaces; Seatbelt always).
    pub own_network: bool,
}

impl Networks {
    /// Read now, for a run in `sandbox` whose blocked connections go to
    /// `log` where the sandbox can say them.
    pub fn current(sandbox: Sandbox, log: PathBuf) -> Self {
        match sandbox {
            Sandbox::Bubblewrap if own_network_available() => Self {
                host: crate::reach::HostNetwork::current(),
                loopback: crate::reach::loopback_services(),
                closed: crate::reach::closed_ports().into_iter().collect(),
                log: Some(log),
                own_network: true,
            },
            Sandbox::Seatbelt => Self {
                host: crate::reach::HostNetwork::current(),
                loopback: crate::reach::loopback_services(),
                closed: crate::reach::closed_ports().into_iter().collect(),
                log: None,
                own_network: true,
            },
            Sandbox::Bubblewrap | Sandbox::Landlock | Sandbox::ProcessContainer => Self::default(),
        }
    }
}

/// Whether a run on Bubblewrap can have a network of its own: slirp4netns,
/// util-linux and iptables there, and unprivileged user and network
/// namespaces allowed (Ubuntu 23.10 and later refuse them to `unshare` with
/// AppArmor, though not to Bubblewrap itself). Without it a run with no
/// network still starts, unlogged and without local services; one with
/// the internet fails and says why, as before. Asked of MXC, at most once a
/// minute.
#[cfg(target_os = "linux")]
pub(super) fn own_network_available() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static KNOWN: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut known = KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, answer)) = *known
        && at.elapsed() < Duration::from_secs(60)
    {
        return answer;
    }
    let answer = mxc_sdk::bwrap_common::proxy_network::probe_proxy_enforcement().is_ok();
    if !answer {
        tracing::info!("commands get no network of their own here: no local services");
    }
    *known = Some((Instant::now(), answer));
    answer
}

#[cfg(not(target_os = "linux"))]
fn own_network_available() -> bool {
    false
}

/// A language toolchain a command may use: folders it reads, where its
/// programs are, and what tells them where they live.
struct Toolchain {
    read: &'static str,
    bins: &'static [&'static str],
    env: Option<&'static str>,
}

/// Toolchains under the person's home (`~/`) or the system, used when
/// they exist. Only what runs programs: never `~/.ssh`, credentials or the
/// rest of the home folder. Cargo's own home holds registry credentials,
/// so only its `bin` is read.
const TOOLCHAINS: &[Toolchain] = &[
    Toolchain {
        read: "~/.local/share/mise",
        bins: &["shims"],
        env: Some("MISE_DATA_DIR"),
    },
    Toolchain {
        read: "~/.config/mise",
        bins: &[],
        env: Some("MISE_CONFIG_DIR"),
    },
    Toolchain {
        read: "~/.rbenv",
        bins: &["shims", "bin"],
        env: Some("RBENV_ROOT"),
    },
    Toolchain {
        read: "~/.pyenv",
        bins: &["shims", "bin"],
        env: Some("PYENV_ROOT"),
    },
    Toolchain {
        read: "~/.rustup",
        bins: &[],
        env: Some("RUSTUP_HOME"),
    },
    Toolchain {
        read: "~/.cargo/bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        read: "~/.volta",
        bins: &["bin"],
        env: Some("VOLTA_HOME"),
    },
    Toolchain {
        read: "~/.bun/bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        read: "~/.deno/bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        read: "~/.local/bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        read: "~/.nix-profile",
        bins: &["bin"],
        env: None,
    },
    Toolchain {
        read: "/nix",
        bins: &["var/nix/profiles/default/bin"],
        env: None,
    },
    Toolchain {
        read: "/run/current-system/sw",
        bins: &["bin"],
        env: None,
    },
    Toolchain {
        read: "/opt/homebrew",
        bins: &["bin", "sbin"],
        env: None,
    },
    Toolchain {
        read: "/home/linuxbrew/.linuxbrew",
        bins: &["bin", "sbin"],
        env: None,
    },
    Toolchain {
        read: "/usr/local",
        bins: &["bin", "sbin"],
        env: None,
    },
    Toolchain {
        read: "/opt",
        bins: &[],
        env: None,
    },
    // The system's C and C++ headers, which MXC's Bubblewrap baseline leaves
    // out of `/usr`: without them a compiler finds no `stdio.h`, and any
    // build that compiles C (a `-sys` crate, a native gem) fails. Read only,
    // and not there on macOS, whose headers are in the SDK MXC already reads.
    Toolchain {
        read: "/usr/include",
        bins: &[],
        env: None,
    },
];

/// The system resolver's sockets, hidden without the internet so a command
/// can't ask it to look names up past its network (`HOST_RESOLVER_SOCKETS`
/// in the mxc gem). Only the sockets: the stub's `resolv.conf` beside them
/// stays, so MXC can point it at slirp's forwarder, where the firewall
/// refuses the lookup and logs the name it asked for.
/// The local network's addresses, refused to a run that gets only the
/// internet ([`Plan::internet_only`]). 10.0.0.0/8 is listed around slirp's
/// own `10.0.2.0/24`, whose name server (`10.0.2.3`) looks names up on the
/// internet; its gateway to this computer (`10.0.2.2`) is refused.
const PRIVATE_RANGES: &[&str] = &[
    "10.0.0.0/23",
    "10.0.2.2/32",
    "10.0.3.0/24",
    "10.0.4.0/22",
    "10.0.8.0/21",
    "10.0.16.0/20",
    "10.0.32.0/19",
    "10.0.64.0/18",
    "10.0.128.0/17",
    "10.1.0.0/16",
    "10.2.0.0/15",
    "10.4.0.0/14",
    "10.8.0.0/13",
    "10.16.0.0/12",
    "10.32.0.0/11",
    "10.64.0.0/10",
    "10.128.0.0/9",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    // Benchmarking addresses, which some home and VPN networks use.
    "198.18.0.0/15",
];

const RESOLVER_SOCKETS: &[&str] = &[
    "/run/systemd/resolve/io.systemd.Resolve",
    "/run/systemd/resolve/io.systemd.Resolve.Monitor",
];

/// The paths, `PATH` and variables the toolchains that exist here add.
struct Found {
    read: Vec<String>,
    path: Vec<String>,
    env: Vec<(String, String)>,
}

/// The toolchain folders a run reads on Linux and macOS.
#[cfg(not(windows))]
pub(super) fn toolchain_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    toolchains(home)
        .read
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn toolchains(home: Option<&Path>) -> Found {
    let mut found = Found {
        read: Vec::new(),
        path: Vec::new(),
        env: Vec::new(),
    };
    for toolchain in TOOLCHAINS {
        let dir = match toolchain.read.strip_prefix("~/") {
            Some(rest) => match home {
                Some(home) => home.join(rest),
                None => continue,
            },
            None => PathBuf::from(toolchain.read),
        };
        // Canonical, so a link (Nix's profiles) is bound where it leads.
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        if !real.is_dir() {
            continue;
        }
        found.read.push(display(&real));
        for bin in toolchain.bins {
            let bin = if bin.is_empty() {
                dir.clone()
            } else {
                dir.join(bin)
            };
            if bin.is_dir() {
                found.path.push(display(&bin));
            }
        }
        if let Some(var) = toolchain.env {
            found.env.push((var.to_string(), display(&dir)));
        }
    }
    found
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `text` as one word of `sh`, in single quotes.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

impl Plan {
    /// How far this run reaches in `sandbox`: its folder's level, as far as
    /// the sandbox holds it, raised to the internet by an "Allow once" for an
    /// internet address. Landlock can't tell addresses apart and the
    /// process container can't reach this computer's own services, so there
    /// a folder's local services alone give nothing.
    pub fn network_access(&self, sandbox: Sandbox) -> crate::reach::Level {
        use crate::reach::Level;
        let internet = self.network || self.once_internet();
        match sandbox {
            Sandbox::Landlock | Sandbox::ProcessContainer => {
                if internet {
                    Level::Internet
                } else {
                    Level::None
                }
            }
            Sandbox::Bubblewrap | Sandbox::Seatbelt => {
                if internet {
                    Level::Internet
                } else if self.local_services && self.networks.own_network {
                    Level::Local
                } else {
                    Level::None
                }
            }
        }
    }

    /// Whether an "Allow once" for this run names an internet address.
    fn once_internet(&self) -> bool {
        self.once.iter().any(|g| {
            g.internet || self.networks.host.classify(g.host) == crate::reach::Where::Internet
        })
    }

    /// Where the sandbox logs what it blocked for this run.
    pub fn blocked_log(&self) -> PathBuf {
        let mut name = self.home.clone().into_os_string();
        name.push(".blocked");
        PathBuf::from(name)
    }

    /// The MXC `network` section for this run in `sandbox` (not Windows,
    /// whose request says only on or off).
    ///
    /// - `none`: nothing. On Bubblewrap with slirp4netns the run still gets
    ///   a network of its own that blocks everything, so what it tried is
    ///   logged.
    /// - `local`: this computer's loopback services (as the run's own
    ///   `localhost:<port>`), its own addresses and its containers'
    ///   networks, never the ports in `reach::EXCLUDED_PORTS`.
    /// - `internet`: everything, the same ports of this computer aside.
    ///
    /// The folder's `reach` entries and this run's "Allow once" add exact
    /// destinations. A blocked connection is refused
    /// at once.
    pub fn network_json(&self, sandbox: Sandbox) -> serde_json::Value {
        use crate::reach::{Level, Where};
        let level = self.network_access(sandbox);
        let host = &self.networks.host;
        let mut ports: Vec<u16> = Vec::new();
        let mut allow: Vec<serde_json::Value> = Vec::new();
        let mut deny: Vec<serde_json::Value> = Vec::new();
        let this_computer: Vec<serde_json::Value> = host
            .addresses
            .iter()
            .filter_map(|a| match a {
                std::net::IpAddr::V4(v4) => Some(json!({ "cidr": format!("{v4}/32") })),
                std::net::IpAddr::V6(_) => None,
            })
            .chain(
                host.bridges
                    .iter()
                    .map(|(net, prefix)| json!({ "cidr": format!("{net}/{prefix}") })),
            )
            .collect();
        let rules = sandbox == Sandbox::Bubblewrap;
        if self.internet_only {
            return self.internet_only_json(level, rules, this_computer);
        }
        if level != Level::None {
            ports.extend(&self.networks.loopback);
            if rules && level == Level::Local && !this_computer.is_empty() {
                allow.push(json!({ "to": this_computer }));
            }
        }
        let mut exact: Vec<(std::net::IpAddr, u16)> = Vec::new();
        // A folder's exact addresses hold at every level below the internet,
        // which has them already: each was allowed by name.
        if level != Level::Internet {
            // Never one of this computer's own addresses: those are local
            // services, which the level gives or doesn't.
            exact.extend(
                self.reach
                    .iter()
                    .map(|a| (std::net::IpAddr::V4(*a.ip()), a.port()))
                    .filter(|(ip, _)| host.classify(*ip) != Where::ThisComputer),
            );
        }
        for grant in &self.once {
            // An "Allow once" never opens what no command reaches.
            if crate::reach::excluded(grant.port) || self.networks.closed.contains(&grant.port) {
                continue;
            }
            match host.classify(grant.host) {
                // Seatbelt's `localhost:<port>` is every address of this
                // machine, so on macOS its own addresses go there too.
                Where::ThisComputer if grant.host.is_loopback() || !rules => ports.push(grant.port),
                Where::ThisComputer | Where::LocalNetwork if level != Level::Internet => {
                    exact.push((grant.host, grant.port))
                }
                _ => {}
            }
        }
        // The ports no command reaches stay closed on this computer's own
        // addresses and its containers' networks whatever else is allowed:
        // its local services, the internet, or an exact address that turns
        // out to be one of its own.
        if rules && !this_computer.is_empty() && (level != Level::None || !exact.is_empty()) {
            let excluded: Vec<serde_json::Value> = crate::reach::EXCLUDED_PORTS
                .iter()
                .copied()
                .chain(self.networks.closed.iter().map(|port| (*port, *port)))
                .map(|(start, end)| {
                    if start == end {
                        json!({ "protocol": "tcp", "port": start })
                    } else {
                        json!({ "protocol": "tcp", "port": start, "endPort": end })
                    }
                })
                .collect();
            deny.push(json!({ "to": this_computer, "ports": excluded }));
        }
        if rules {
            for (ip, port) in exact {
                if let std::net::IpAddr::V4(v4) = ip {
                    allow.push(json!({
                        "to": [{ "cidr": format!("{v4}/32") }],
                        "ports": [{ "protocol": "tcp", "port": port }],
                    }));
                }
            }
        }
        // slirp's name server forwarder is the internet's, even where a
        // container network this computer runs covers its address.
        if rules && level != Level::Internet && !allow.is_empty() {
            deny.insert(0, json!({ "to": [{ "cidr": "10.0.2.3/32" }] }));
        }
        ports.sort_unstable();
        ports.dedup();
        let mut egress = json!({
            "default": if level == Level::Internet { "allow" } else { "deny" },
        });
        let logged = rules && self.networks.log.is_some();
        let walled = level != Level::None || !allow.is_empty() || !ports.is_empty() || logged;
        if matches!(sandbox, Sandbox::Bubblewrap | Sandbox::Seatbelt) && walled {
            if !allow.is_empty() {
                egress["allow"] = allow.into();
            }
            if !deny.is_empty() {
                egress["deny"] = deny.into();
            }
            if !ports.is_empty() {
                egress["hostLoopbackPorts"] = ports.into();
            }
            egress["blocked"] = "reject".into();
            if let (true, Some(log)) = (rules, self.networks.log.as_ref()) {
                egress["blockedLog"] = display(log).into();
            }
        }
        json!({ "egress": egress })
    }

    /// The `network` section of a run that gets the internet and nothing
    /// of this computer or its network: no loopback ports, and on
    /// Bubblewrap every address of this computer's, its containers'
    /// networks and the private ranges refused, slirp's name server aside
    /// (`10.0.2.3`, which looks names up on the internet). Seatbelt can't
    /// tell addresses apart, so there it's the internet without this
    /// computer's loopback ports.
    fn internet_only_json(
        &self,
        level: crate::reach::Level,
        rules: bool,
        this_computer: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        if level == crate::reach::Level::None {
            return json!({ "egress": { "default": "deny" } });
        }
        let mut egress = json!({ "default": "allow", "blocked": "reject" });
        if rules {
            let mut deny = this_computer;
            deny.extend(PRIVATE_RANGES.iter().map(|cidr| json!({ "cidr": cidr })));
            egress["deny"] = json!([{ "to": deny }]);
            if let Some(log) = self.networks.log.as_ref() {
                egress["blockedLog"] = display(log).into();
            }
        }
        json!({ "egress": egress })
    }

    /// Where a run on Bubblewrap finds its folder's values: Bubblewrap
    /// takes a run's environment on its own command line (`--setenv NAME
    /// VALUE`), which any user of this computer can read (`ps`,
    /// `/proc/<pid>/cmdline`). So a run whose environment holds secrets
    /// gets them from this file in its own home (0600, in Desktop's private
    /// data folder), which its shell reads first and deletes. Linux's other
    /// sandbox, macOS and Windows pass the environment to the process
    /// itself, which only this user can read.
    pub fn values_file(&self) -> PathBuf {
        self.home.join(".cww-env")
    }

    fn values_in_file(&self, sandbox: Sandbox) -> bool {
        sandbox == Sandbox::Bubblewrap && !self.env.values.is_empty()
    }

    fn is_folder_value(&self, name: &str, value: &str) -> bool {
        self.env
            .values
            .iter()
            .any(|(n, v)| n == name && v.expose() == value)
    }

    /// Write [`Plan::values_file`] where the run needs it, before the
    /// sandbox starts: `export` lines the shell reads, each value quoted.
    /// The values are exactly those [`Plan::environment`] would set.
    pub fn write_values(&self, home: Option<&Path>, sandbox: Sandbox) -> std::io::Result<()> {
        if !self.values_in_file(sandbox) {
            return Ok(());
        }
        let mut script = String::new();
        let values = self
            .environment(home)
            .into_iter()
            .filter(|(name, value)| self.is_folder_value(name, value));
        for (name, value) in values {
            script.push_str(&format!("export {name}={}\n", shell_quote(&value)));
        }
        // A `$( )` ran in the sandbox with this home before: it may have
        // left a link here, which this process, outside the sandbox, must
        // never write through. So whatever is there goes (a link itself,
        // never what it leads to), and the file is made new.
        let file = self.values_file();
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        options.open(&file)?.write_all(script.as_bytes())
    }

    /// The folder in the run's home that holds only a link to each agent's
    /// program setup checked, by its name (Linux and macOS).
    pub fn agents_bin(&self) -> PathBuf {
        self.home.join(".cww-bin")
    }

    /// What a bot's run allowed an agent gets: no self-updates, whose folder
    /// it can't write, and its temporary files in the run's own. Its sign-in
    /// is where the agent keeps it ([`Plan::coding_writable`]).
    pub fn coding_environment(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .coding
            .agents
            .iter()
            .flat_map(|a| {
                a.agent
                    .environment()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .chain(a.login.env.iter().cloned())
            })
            .collect();
        if self
            .coding
            .agents
            .iter()
            .any(|a| a.agent == crate::coding::Agent::ClaudeCode)
        {
            env.extend([
                // Claude Code keeps its own files under /tmp otherwise,
                // which Seatbelt and the process container don't give a
                // run: the run's own temporary folder instead.
                (
                    "CLAUDE_CODE_TMPDIR".to_string(),
                    display(&self.home.join("tmp")),
                ),
            ]);
        }
        env
    }

    /// Where a run prepared a repository in this folder, what tells package
    /// managers to keep their files in the folder's own place. After the
    /// toolchains' variables, which it takes over.
    pub fn tools_environment(&self) -> Vec<(String, String)> {
        self.coding
            .tools
            .iter()
            .flat_map(|tools| {
                let mut env = crate::coding::prepare::tools_env(tools);
                env.extend(crate::coding::prepare::tools_mise_env(tools));
                env
            })
            .collect()
    }

    /// What the run's output has taken out: the tokens of the agents'
    /// sign-ins Desktop could read when it started, and the folder's values.
    pub fn redactor(&self) -> crate::coding::Redactor {
        crate::coding::Redactor::new(
            self.coding
                .agents
                .iter()
                .flat_map(|run| {
                    run.login
                        .secrets
                        .iter()
                        .map(|secret| (secret, run.agent.redacted().to_string()))
                })
                .chain(
                    self.env
                        .values
                        .iter()
                        .map(|(_, value)| (value, super::env::REDACTED.to_string())),
                ),
        )
    }

    /// What a run allowed an agent reads and writes beyond its folder and
    /// home: `~/.claude` and `~/.claude.json`, `~/.codex`, or
    /// `~/.local/share/opencode`, where they
    /// really are, which its own home links to (`run.rs`). The agent
    /// refreshes its tokens there. Nothing else of the person's home.
    pub fn coding_writable(&self) -> Vec<String> {
        self.coding
            .agents
            .iter()
            .flat_map(|c| c.login.paths())
            .map(|p| display(p))
            .collect()
    }

    /// Folders a run reads beyond the toolchains: the folder's mirrors, and
    /// the folder each agent's program really is in.
    pub fn coding_readable(&self) -> Vec<String> {
        // Canonical, as the toolchains are: a sandbox binds a folder where
        // it is, and Bubblewrap can't start with a link (mise's
        // `installs/claude/latest`) given as one. An agent's is only what
        // `coding::grant` allowed: its own folder, or the program.
        let mut dirs: Vec<PathBuf> = self.coding.mirrors.iter().map(|(m, _)| m.clone()).collect();
        dirs.extend(self.coding.read.iter().cloned());
        for run in &self.coding.agents {
            dirs.push(run.read.clone());
            dirs.extend(run.entry.clone());
            dirs.extend(run.helpers());
        }
        let mut read: Vec<String> = Vec::new();
        for dir in dirs {
            let Ok(real) = crate::roots::canonical(&dir) else {
                continue;
            };
            if real.exists() {
                let real = display(&real);
                if !read.contains(&real) {
                    read.push(real);
                }
            }
        }
        read
    }

    /// The environment the command gets: nothing of the daemon's, a home
    /// and temporary folder of its own, and where the toolchains are.
    pub fn environment(&self, home: Option<&Path>) -> Vec<(String, String)> {
        let found = toolchains(home);
        let own = display(&self.home);
        let mut path = Vec::new();
        if !self.coding.agents.is_empty() {
            // Its own agents first, so no other program of that name (mise's,
            // npm's) shadows the one setup checked.
            path.push(display(&self.agents_bin()));
        }
        if let Some(tools) = &self.coding.tools {
            path.extend(crate::coding::prepare::tools_path(tools));
        }
        path.extend(found.path);
        path.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(String::from));
        path.dedup();
        let mut env = vec![
            ("PATH".to_string(), path.join(":")),
            ("HOME".to_string(), own.clone()),
            ("TMPDIR".to_string(), format!("{own}/tmp")),
            // The person's own /run/user/<uid> is out of the sandbox's
            // reach: tools that keep sockets or locks there get their own.
            ("XDG_RUNTIME_DIR".to_string(), format!("{own}/run")),
            ("XDG_CACHE_HOME".to_string(), format!("{own}/.cache")),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("TERM".to_string(), "dumb".to_string()),
            // mise reads a project's own `mise.toml` only once trusted.
            (
                "MISE_TRUSTED_CONFIG_PATHS".to_string(),
                display(&self.folder),
            ),
            ("MISE_CACHE_DIR".to_string(), format!("{own}/.cache/mise")),
            (
                "MISE_DATA_DIR".to_string(),
                format!("{own}/.local/share/mise"),
            ),
            (
                "MISE_SHIMS_DIR".to_string(),
                format!("{own}/.local/share/mise/shims"),
            ),
            (
                "MISE_STATE_DIR".to_string(),
                format!("{own}/.local/state/mise"),
            ),
        ];
        env.extend(self.coding_environment());
        for (name, value) in found.env {
            if name == "MISE_DATA_DIR" {
                // mise writes command-wrapper shims and their lock in its
                // data directory even when executing an installed tool.
                // Only the installations/plugins belong to the host.
                env.extend([
                    ("MISE_INSTALLS_DIR".to_string(), format!("{value}/installs")),
                    (
                        "MISE_INSTALL_STORE_DIR".to_string(),
                        format!("{value}/installs"),
                    ),
                    ("MISE_PLUGINS_DIR".to_string(), format!("{value}/plugins")),
                ]);
            } else {
                env.push((name, value));
            }
        }
        // Later values win, so what the toolchains named goes first.
        let tools = self.tools_environment();
        env.retain(|(name, _)| !tools.iter().any(|(own, _)| own == name));
        env.extend(tools);
        self.env.add_to(&mut env);
        env
    }

    /// The 1.1.0-alpha MXC request for this run in `sandbox`: the contract
    /// that carries resource limits (`resources`), which the typed API
    /// doesn't yet.
    ///
    /// On Linux it asks for `process`, MXC's own choice, which is Bubblewrap
    /// or, where Bubblewrap can't run, Landlock; `sandbox` says which this
    /// computer gets, for what the two hold differently. A run with the
    /// network never gets here on Landlock ([`Sandbox::refuses`]).
    pub fn request_json(&self, home: Option<&Path>, sandbox: Sandbox) -> String {
        if sandbox == Sandbox::ProcessContainer {
            return self
                .windows_request(&super::windows::Host::current())
                .to_string();
        }
        let mut found = toolchains(home);
        for dir in self.coding_readable() {
            if !found.read.contains(&dir) {
                found.read.push(dir);
            }
        }
        if sandbox == Sandbox::Landlock {
            // Landlock can't take write access back below a folder it
            // grants, so it refuses a read-only path there. The run may
            // write those already.
            found.read.retain(|path| {
                let path = Path::new(path);
                !path.starts_with(&self.folder) && !path.starts_with(&self.home)
            });
        }
        let mut denied: Vec<String> = Vec::new();
        if self.network_access(sandbox) != crate::reach::Level::Internet {
            denied.extend(
                RESOLVER_SOCKETS
                    .iter()
                    .filter(|p| Path::new(p).exists())
                    .map(|p| p.to_string()),
            );
        }
        let mut readwrite = vec![display(&self.folder), display(&self.home)];
        readwrite.extend(self.coding_writable());
        let in_file = self.values_in_file(sandbox);
        let env: Vec<String> = self
            .environment(home)
            .into_iter()
            .filter(|(name, value)| !(in_file && self.is_folder_value(name, value)))
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let command = if in_file {
            let file = shell_quote(&display(&self.values_file()));
            format!(". {file}; rm -f {file}\n{}", self.command)
        } else {
            self.command.clone()
        };
        let mut document = json!({
            "version": "1.1.0-alpha",
            "containerId": format!("cww-run-{}", self.id),
            "lifecycle": { "destroyOnExit": true, "preservePolicy": false },
            "process": {
                "commandLine": command,
                // The runner keeps the wall clock itself, so it can say why
                // a run ended.
                "timeout": 0,
                "cwd": display(&self.cwd),
                "env": env,
            },
            "filesystem": {
                "readwritePaths": readwrite,
                "readonlyPaths": found.read,
                "deniedPaths": denied,
            },
            "network": self.network_json(sandbox),
            "resources": { "priority": "low" },
        });
        // 0 is no CPU limit, which MXC takes as the field left out.
        if self.limits.cpu_seconds > 0 {
            document["resources"]["cpuTimeMs"] =
                self.limits.cpu_seconds.saturating_mul(1000).into();
        }
        // On Linux the memory limit is the run's cgroup's (`cgroup.rs`),
        // never an rlimit: those count reserved address space, which rustc,
        // a JVM or Go reserve far beyond what they use. Seatbelt can't hold
        // one at all.
        if let Some(cgroup) = &self.cgroup {
            document["resources"]["cgroup"] = display(cgroup).into();
        }
        if sandbox == Sandbox::Seatbelt {
            document["containment"] = "seatbelt".into();
            document["seatbelt"] = json!({
                "guiAccess": false,
                "nestedPty": false,
                // Claude Code keeps its sign-in in the login keychain on
                // macOS: a run allowed it reaches the keychain. Codex's is a
                // file, so a run allowed only Codex doesn't.
                "keychainAccess": self.coding.agents.iter().any(|c| c.login.keychain),
                "extraMachLookups": [],
            });
        } else {
            document["containment"] = "process".into();
        }
        document.to_string()
    }
}
