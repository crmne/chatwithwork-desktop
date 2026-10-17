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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub root: String,
    /// Relative to the folder, `""` for the folder itself.
    #[serde(default)]
    pub dir: String,
    pub command: String,
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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(RunError::new(
                "unavailable",
                "commands can't run on this system yet",
            ))
        }
    }

    /// Why `plan` can't run in this sandbox, if it can't. Landlock runs a
    /// command in this computer's network and can't tell one address from
    /// another, so it could only allow all of it, this computer's loopback
    /// (its local servers) and connections in included. Bubblewrap gives a
    /// command a network of its own instead.
    pub fn refuses(self, plan: &Plan) -> Option<RunError> {
        (self == Self::Landlock && plan.network).then(|| {
            RunError::new(
                "network_unavailable",
                "this folder allows commands the network, which this computer can't give them \
                 safely: Bubblewrap can't create a sandbox here, and Landlock, used instead, \
                 could only give the whole network, its local servers included. Turn the \
                 network off for this folder, or allow unprivileged user namespaces for \
                 Bubblewrap",
            )
        })
    }

    /// Processes in a run's tree, its first one included, that are the
    /// sandbox's rather than the command's. Bubblewrap starts the command
    /// under itself and its init; on Landlock the command's shell is the
    /// first process.
    pub fn own_processes(self) -> usize {
        match self {
            Self::Bubblewrap => 3,
            Self::Landlock | Self::Seatbelt => 0,
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
    pub network: bool,
    pub limits: Limits,
    /// Its own home and temporary folder, made for it and removed after.
    pub home: PathBuf,
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
    let folder = root
        .path
        .canonicalize()
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
    let cwd = folder
        .join(relative)
        .canonicalize()
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
        limits: Limits::from_config(&config.limits),
        home: paths.runs_dir().join(id),
    })
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
];

/// The system resolver's socket, hidden when there's no network so a
/// command can't ask it to look names up (`HOST_RESOLVER_SOCKETS` in the
/// mxc gem).
const RESOLVER_SOCKETS: &[&str] = &["/run/systemd/resolve"];

/// The paths, `PATH` and variables the toolchains that exist here add.
struct Found {
    read: Vec<String>,
    path: Vec<String>,
    env: Vec<(String, String)>,
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

impl Plan {
    /// The environment the command gets: nothing of the daemon's, a home
    /// and temporary folder of its own, and where the toolchains are.
    pub fn environment(&self, home: Option<&Path>) -> Vec<(String, String)> {
        let found = toolchains(home);
        let own = display(&self.home);
        let mut path = found.path;
        path.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(String::from));
        path.dedup();
        let mut env = vec![
            ("PATH".to_string(), path.join(":")),
            ("HOME".to_string(), own.clone()),
            ("TMPDIR".to_string(), format!("{own}/tmp")),
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
                "MISE_STATE_DIR".to_string(),
                format!("{own}/.local/state/mise"),
            ),
        ];
        env.extend(found.env);
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
        let mut found = toolchains(home);
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
        if !self.network {
            denied.extend(
                RESOLVER_SOCKETS
                    .iter()
                    .filter(|p| Path::new(p).exists())
                    .map(|p| p.to_string()),
            );
        }
        let env: Vec<String> = self
            .environment(home)
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let mut document = json!({
            "version": "1.1.0-alpha",
            "containerId": format!("cww-run-{}", self.id),
            "lifecycle": { "destroyOnExit": true, "preservePolicy": false },
            "process": {
                "commandLine": self.command,
                // The runner keeps the wall clock itself, so it can say why
                // a run ended.
                "timeout": 0,
                "cwd": display(&self.cwd),
                "env": env,
            },
            "filesystem": {
                "readwritePaths": [display(&self.folder), display(&self.home)],
                "readonlyPaths": found.read,
                "deniedPaths": denied,
            },
            "network": { "egress": { "default": if self.network { "allow" } else { "deny" } } },
            "resources": {
                "cpuTimeMs": self.limits.cpu_seconds.saturating_mul(1000),
                "memoryMb": self.limits.memory_mb,
                "priority": "low",
            },
        });
        if sandbox == Sandbox::Seatbelt {
            document["containment"] = "seatbelt".into();
            document["seatbelt"] = json!({
                "guiAccess": false,
                "nestedPty": false,
                "keychainAccess": false,
                "extraMachLookups": [],
            });
        } else {
            document["containment"] = "process".into();
        }
        document.to_string()
    }
}
