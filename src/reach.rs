//! Where a command may connect: this computer's own services, the local
//! network, the internet (docs/commands.md, "Its network").
//!
//! A folder that allows commands has a level: `none`, `local` (this
//! computer's services: its loopback, its own addresses, and the networks
//! of the containers and virtual machines it runs, such as Docker's
//! `docker0`) or `internet` (everything, local services included). Exact
//! local-network addresses can be added one by one (`reach`). Everything
//! here is worked out on this computer, from its own interfaces, never from
//! what a page or the server says an address is.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4};

use serde::{Deserialize, Serialize};

/// How far a folder's commands reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Nothing at all.
    None,
    /// This computer's own services, never the local network or the internet.
    Local,
    /// The internet and the local network, local services included.
    Internet,
}

impl Level {
    /// Its name in `roots`, the dry run and `status` (`network_access`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Local => "local",
            Self::Internet => "internet",
        }
    }

    /// What the app and `cww` call it.
    pub fn words(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Local => "Local services",
            Self::Internet => "Local services and the internet",
        }
    }

    /// From a folder's two switches.
    pub fn of(local_services: bool, internet: bool) -> Self {
        if internet {
            Self::Internet
        } else if local_services {
            Self::Local
        } else {
            Self::None
        }
    }
}

/// Where an address is, from this computer's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Where {
    ThisComputer,
    LocalNetwork,
    Internet,
}

impl Where {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThisComputer => "this_computer",
            Self::LocalNetwork => "local_network",
            Self::Internet => "internet",
        }
    }

    /// The words the app's dialog and `cww` use.
    pub fn words(self) -> &'static str {
        match self {
            Self::ThisComputer => "this computer",
            Self::LocalNetwork => "the local network",
            Self::Internet => "the internet",
        }
    }
}

/// The well-known service on a port, for what a person reads ("PostgreSQL
/// (172.17.0.4:5432)"). The same table everywhere: the `blocked` list, the
/// setup's question and the app's dialog.
pub fn service(port: u16) -> Option<&'static str> {
    Some(match port {
        80 => "HTTP",
        443 => "HTTPS",
        53 => "DNS",
        1433 => "SQL Server",
        3000 => "a development server",
        3306 => "MySQL",
        4200 => "a development server",
        5173 => "a development server",
        5432 => "PostgreSQL",
        5672 => "RabbitMQ",
        6379 => "Redis",
        7700 => "Meilisearch",
        8000 | 8080 => "a development server",
        8108 => "Typesense",
        9000 => "MinIO",
        9092 => "Kafka",
        9200 => "Elasticsearch",
        11211 => "Memcached",
        27017 => "MongoDB",
        _ => return None,
    })
}

/// Ports on this computer a command never reaches, at any level: each
/// would let it out of its sandbox, not just reach a service.
///
/// - 2375 and 2376: the Docker API over TCP (and Podman's, when served
///   there). Whoever reaches it can start a container that mounts any
///   folder of this computer.
/// - 6000 to 6063: X11 servers listening on TCP, which take keystrokes and
///   read every window.
/// - 9222: Chrome's and Chromium's DevTools port, which drives that browser
///   with every sign-in in it.
/// - 9229: Node.js's inspector, which runs code in the process it inspects.
/// - 5900 to 5999: VNC servers, the desktop's screen and keyboard.
///
/// Desktop's own listeners are left out too ([`own_listening_ports`]).
pub const EXCLUDED_PORTS: &[(u16, u16)] = &[
    (2375, 2376),
    (5900, 5999),
    (6000, 6063),
    (9222, 9222),
    (9229, 9229),
];

/// Whether `port` is one of [`EXCLUDED_PORTS`].
pub fn excluded(port: u16) -> bool {
    EXCLUDED_PORTS
        .iter()
        .any(|(start, end)| (*start..=*end).contains(&port))
}

/// This computer's own networks, as a command's rules need them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostNetwork {
    /// Its own addresses, loopback aside.
    pub addresses: Vec<IpAddr>,
    /// The IPv4 networks of the interfaces its containers and virtual
    /// machines use (`docker0`, `br-*`, `podman*`, `virbr*`, `cni*`, ...),
    /// as (network, prefix length).
    pub bridges: Vec<(Ipv4Addr, u8)>,
}

/// Interface names that are this computer's own virtual networks: Docker's
/// (`docker0`, and `br-<id>` for Compose's networks), Podman's, libvirt's,
/// CNI's, LXC's, LXD's and Incus's.
const BRIDGE_PREFIXES: &[&str] = &[
    "docker", "br-", "podman", "virbr", "cni", "lxcbr", "lxdbr", "incusbr", "vboxnet", "vmnet",
];

fn is_bridge(name: &str) -> bool {
    BRIDGE_PREFIXES.iter().any(|p| name.starts_with(p))
}

impl HostNetwork {
    /// Read from this computer's interfaces now.
    pub fn current() -> Self {
        let mut network = Self::default();
        for (name, address, prefix) in interfaces() {
            if address.is_loopback() {
                continue;
            }
            if !network.addresses.contains(&address) {
                network.addresses.push(address);
            }
            if let IpAddr::V4(v4) = address
                && is_bridge(&name)
                && prefix >= 8
            {
                let block = (mask(v4, prefix), prefix);
                if !network.bridges.contains(&block) {
                    network.bridges.push(block);
                }
            }
        }
        network
    }

    /// Where `address` is: this computer (its loopback, its own addresses,
    /// the networks of its containers), the local network (private,
    /// link-local and unique local addresses, and shared address space), or
    /// the internet.
    pub fn classify(&self, address: IpAddr) -> Where {
        if address.is_loopback() || address.is_unspecified() || self.addresses.contains(&address) {
            return Where::ThisComputer;
        }
        if let IpAddr::V4(v4) = address
            && self
                .bridges
                .iter()
                .any(|(network, prefix)| mask(v4, *prefix) == *network)
        {
            return Where::ThisComputer;
        }
        if is_local_network(address) {
            Where::LocalNetwork
        } else {
            Where::Internet
        }
    }
}

/// Private (RFC 1918), link-local, shared (RFC 6598, carrier NAT and
/// Tailscale) and unique local (fc00::/7) addresses.
pub fn is_local_network(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn mask(address: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let bits = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix.min(32)))
    };
    Ipv4Addr::from(bits & mask)
}

/// Every interface's addresses: (name, address, prefix length).
#[cfg(unix)]
fn interfaces() -> Vec<(String, IpAddr, u8)> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list`, freed below; each entry is read
    // within its own lifetime.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut entry = list;
        while !entry.is_null() {
            let current = &*entry;
            entry = current.ifa_next;
            if current.ifa_addr.is_null() || current.ifa_name.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(current.ifa_name)
                .to_string_lossy()
                .into_owned();
            let (address, prefix) = match i32::from((*current.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let address = &*(current.ifa_addr as *const libc::sockaddr_in);
                    let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
                    let prefix = if current.ifa_netmask.is_null() {
                        32
                    } else {
                        let netmask = &*(current.ifa_netmask as *const libc::sockaddr_in);
                        u32::from_be(netmask.sin_addr.s_addr).count_ones() as u8
                    };
                    (IpAddr::V4(ip), prefix)
                }
                libc::AF_INET6 => {
                    let address = &*(current.ifa_addr as *const libc::sockaddr_in6);
                    (IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)), 128)
                }
                _ => continue,
            };
            found.push((name, address, prefix));
        }
        libc::freeifaddrs(list);
    }
    found
}

#[cfg(not(unix))]
fn interfaces() -> Vec<(String, IpAddr, u8)> {
    Vec::new()
}

/// TCP ports this computer listens on at its loopback, or on every address
/// (which includes the loopback): what a command at the `local` level may
/// reach as its own `localhost:<port>`. Without [`EXCLUDED_PORTS`],
/// Desktop's own listeners, and the person's own programs' listeners on
/// a port the system picked for them ([`ephemeral_ports`]): a service
/// meant to be found has a port of its own, while a random one is an
/// editor's, a browser's debugger or a sign-in's callback, meant for one
/// program only. A container's published port is Docker's (root's), so it
/// stays, whatever its number.
pub fn loopback_services() -> Vec<u16> {
    let closed = closed_ports();
    let ports: BTreeSet<u16> = listening()
        .into_iter()
        .filter(|l| l.address.is_loopback() || l.address.is_unspecified())
        .map(|l| l.port)
        .filter(|port| !excluded(*port) && !closed.contains(port))
        .collect();
    ports.into_iter().collect()
}

/// Ports this computer listens on that a command never reaches, on any of
/// its addresses, besides [`EXCLUDED_PORTS`]: Desktop's own listeners, and
/// the person's own programs' on a port the system picked for them.
pub fn closed_ports() -> BTreeSet<u16> {
    let ephemeral = ephemeral_ports();
    let mut closed = own_listening_ports();
    closed.extend(
        listening()
            .into_iter()
            .filter(|l| l.mine && ephemeral.contains(&l.port))
            .map(|l| l.port),
    );
    closed
}

/// A listening TCP socket on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listener {
    address: IpAddr,
    port: u16,
    /// This user's own program's.
    mine: bool,
}

/// The ports the system gives a program that asks for any.
fn ephemeral_ports() -> std::ops::RangeInclusive<u16> {
    #[cfg(target_os = "linux")]
    if let Ok(text) = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range") {
        let mut bounds = text
            .split_whitespace()
            .filter_map(|n| n.parse::<u16>().ok());
        if let (Some(low), Some(high)) = (bounds.next(), bounds.next()) {
            return low..=high;
        }
    }
    if cfg!(target_os = "linux") {
        32768..=60999
    } else {
        49152..=65535
    }
}

/// Every listening TCP socket on this computer.
#[cfg(target_os = "linux")]
fn listening() -> Vec<Listener> {
    let me = rustix::process::getuid().as_raw();
    let mut found = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        found.extend(text.lines().skip(1).filter_map(|line| {
            let socket = proc_net_socket(line, v6)?;
            (socket.state == 0x0a).then_some(Listener {
                address: socket.address,
                port: socket.port,
                mine: socket.uid == me,
            })
        }));
    }
    found
}

/// This user's own listening sockets (macOS): `lsof` sees only those
/// without root, and `netstat` lists none from macOS 27 on. Development
/// services on a Mac (Homebrew's, Docker Desktop's published ports) run as
/// the person. Docker Desktop's and its like's count as a container's
/// (not "mine"), whatever port they took.
#[cfg(target_os = "macos")]
fn listening() -> Vec<Listener> {
    own_user_listeners()
        .into_iter()
        .map(|(command, address, port)| {
            let containers = [
                "com.docker",
                "docker",
                "vpnkit",
                "orbstack",
                "colima",
                "limactl",
                "gvproxy",
                "podman",
            ]
            .iter()
            .any(|p| command.starts_with(p));
            Listener {
                address,
                port,
                mine: !containers,
            }
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn listening() -> Vec<Listener> {
    Vec::new()
}

/// `lsof -F cn`'s output: `c<program>` lines, each followed by `n<address>`
/// lines such as `n127.0.0.1:5432`, `n*:7000` or `n[::1]:6379`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsof(text: &str) -> Vec<(String, IpAddr, u16)> {
    let mut found = Vec::new();
    let mut command = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('c') {
            command = name.to_lowercase();
        } else if let Some(name) = line.strip_prefix('n') {
            let Some((address, port)) = name.rsplit_once(':') else {
                continue;
            };
            let Ok(port) = port.parse() else {
                continue;
            };
            let address = address.trim_start_matches('[').trim_end_matches(']');
            let address = match address {
                "*" => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                other => match other.split('%').next().and_then(|a| a.parse().ok()) {
                    Some(address) => address,
                    None => continue,
                },
            };
            found.push((command.clone(), address, port));
        }
    }
    found
}

/// One line of `/proc/net/tcp` or `tcp6`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct ProcSocket {
    address: IpAddr,
    port: u16,
    state: u8,
    uid: u32,
    inode: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_net_socket(line: &str, v6: bool) -> Option<ProcSocket> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let (address, port) = fields.get(1)?.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let state = u8::from_str_radix(fields.get(3)?, 16).ok()?;
    let uid = fields.get(7)?.parse().ok()?;
    let inode = fields.get(9)?.parse().ok()?;
    let address = if v6 {
        if address.len() != 32 {
            return None;
        }
        // Four 32-bit words, each in host (little-endian) order.
        let mut bytes = [0u8; 16];
        for word in 0..4 {
            let value = u32::from_str_radix(&address[word * 8..word * 8 + 8], 16).ok()?;
            bytes[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        let v6 = Ipv6Addr::from(bytes);
        v6.to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6))
    } else {
        let value = u32::from_str_radix(address, 16).ok()?;
        IpAddr::V4(Ipv4Addr::from(value.to_le_bytes()))
    };
    Some(ProcSocket {
        address,
        port,
        state,
        uid,
        inode,
    })
}

/// Ports Desktop's own programs listen on (its app's one-shot sign-in
/// callback, anything else of its own): a command never reaches them,
/// whatever its folder allows. Found by the sockets of this user's
/// processes whose program is Desktop's (`cww`, the app, or anything in the
/// folder they're installed in).
#[cfg(target_os = "linux")]
pub fn own_listening_ports() -> BTreeSet<u16> {
    let ours = own_program_dirs();
    let mut inodes: BTreeSet<u64> = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return BTreeSet::new();
    };
    for entry in entries.flatten() {
        let pid = entry.file_name();
        if !pid.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        let name = exe
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let desktop = name.starts_with("cww")
            || name.contains("chatwithwork")
            || name.contains("chat-with-work")
            || exe
                .parent()
                .is_some_and(|dir| ours.iter().any(|o| o == dir));
        if !desktop {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                let target = target.to_string_lossy();
                if let Some(inode) = target
                    .strip_prefix("socket:[")
                    .and_then(|rest| rest.strip_suffix(']'))
                    .and_then(|n| n.parse().ok())
                {
                    inodes.insert(inode);
                }
            }
        }
    }
    let mut ports = BTreeSet::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for socket in text.lines().skip(1).filter_map(|l| proc_net_socket(l, v6)) {
            if socket.state == 0x0a && inodes.contains(&socket.inode) {
                ports.insert(socket.port);
            }
        }
    }
    ports
}

/// The folders Desktop's own programs are in: this program's.
#[cfg(target_os = "linux")]
fn own_program_dirs() -> Vec<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .filter(|dir| {
            // A shared system folder holds every program: only names count there.
            !["/usr/bin", "/bin", "/usr/local/bin", "/opt/homebrew/bin"]
                .iter()
                .any(|shared| dir == std::path::Path::new(shared))
        })
        .into_iter()
        .collect()
}

/// macOS: this user's own listeners, (program, address, port), from `lsof`.
#[cfg(target_os = "macos")]
fn own_user_listeners() -> Vec<(String, IpAddr, u16)> {
    let Ok(output) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-u"])
        .arg(rustix::process::getuid().as_raw().to_string())
        .args(["-F", "cn"])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof(&String::from_utf8_lossy(&output.stdout))
}

/// macOS: the ports Desktop's own programs listen on.
#[cfg(target_os = "macos")]
pub fn own_listening_ports() -> BTreeSet<u16> {
    own_user_listeners()
        .into_iter()
        .filter(|(command, ..)| command.starts_with("cww") || command.contains("chat with work"))
        .map(|(.., port)| port)
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn own_listening_ports() -> BTreeSet<u16> {
    BTreeSet::new()
}

/// A `reach` entry: an exact `ip:port` on the local network. This
/// computer's own addresses need local services instead, and an internet
/// address the internet, so both are refused here.
pub fn parse_reach(text: &str) -> Result<SocketAddrV4, String> {
    let address: SocketAddrV4 = text
        .trim()
        .parse()
        .map_err(|_| format!("{text} isn't an IPv4 address and port, such as 192.168.1.20:5432"))?;
    if address.port() == 0 {
        return Err("the port must be from 1 to 65535".to_string());
    }
    if excluded(address.port()) {
        return Err(format!(
            "port {} is never reachable from commands: it would let them out of their sandbox",
            address.port()
        ));
    }
    let ip = IpAddr::V4(*address.ip());
    if ip.is_loopback() || ip.is_unspecified() {
        return Err(format!(
            "{} is this computer: allow local services for the folder instead",
            address.ip()
        ));
    }
    if !is_local_network(ip) {
        return Err(format!(
            "{} isn't on the local network: allow the internet for the folder instead",
            address.ip()
        ));
    }
    Ok(address)
}

/// What this computer's sandbox can give for an address at `place`: on
/// Linux exactly that; on macOS an address on the local network only with
/// the internet (Seatbelt names no address but this machine's); on Windows
/// the local network only with the internet, and this computer's own
/// services not at all. `None` when it can't.
pub fn grantable(place: Where) -> Option<Where> {
    grantable_on(place, cfg!(target_os = "macos"), cfg!(windows))
}

fn grantable_on(place: Where, macos: bool, windows: bool) -> Option<Where> {
    match place {
        Where::ThisComputer if windows => None,
        Where::LocalNetwork if macos || windows => Some(Where::Internet),
        other => Some(other),
    }
}

/// The address a host names, as this computer finds it: an IP literal
/// (bracketed or not), `localhost`, or a name it looks up itself (a blocked
/// lookup's), its IPv4 address first.
pub fn resolve(host: &str, port: u16) -> Option<IpAddr> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    if let Ok(address) = host.parse() {
        return Some(address);
    }
    if host.is_empty() || host.len() > 253 || host.contains(['/', ' ', '\0']) {
        return None;
    }
    use std::net::ToSocketAddrs;
    let found: Vec<IpAddr> = (host, port)
        .to_socket_addrs()
        .ok()?
        .map(|a| a.ip())
        .collect();
    found
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| found.first())
        .copied()
}

/// "PostgreSQL (172.17.0.4:5432)", or the address alone.
pub fn describe(host: &str, port: u16) -> String {
    match service(port) {
        Some(service) => format!("{service} ({host}:{port})"),
        None => format!("{host}:{port}"),
    }
}

/// One destination a person allowed for a single command, from "Allow
/// once" in the app's dialog: an exact address and port on this computer or
/// the local network, or, for an internet address, the internet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub host: IpAddr,
    pub port: u16,
    /// The internet for that command, where this computer's sandbox can
    /// give the address no other way ([`grantable`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internet: bool,
}

/// How long an unused "Allow once" waits for its command.
pub const ONCE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// "Allow once" answers waiting for the next command in their folder (for
/// one bot's, when the answer named it), held in memory only: the next such
/// command takes them, and they end with it, or after [`ONCE_LIFETIME`].
#[derive(Clone, Default)]
pub struct OnceGrants(std::sync::Arc<std::sync::Mutex<Held>>);

/// The grants waiting, and how many times they were all dropped: what a
/// command took before that is never put back.
#[derive(Default)]
struct Held {
    waiting: Vec<Waiting>,
    generation: u64,
}

/// What a command took from [`OnceGrants`].
#[derive(Default)]
pub struct Taken {
    taken: Vec<Waiting>,
    generation: u64,
}

impl Taken {
    pub fn grants(&self) -> Vec<Grant> {
        self.taken.iter().map(|w| w.grant.clone()).collect()
    }
}

struct Waiting {
    root: String,
    bot: Option<String>,
    grant: Grant,
    until: std::time::Instant,
}

impl OnceGrants {
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn add(&self, root: &str, bot: Option<&str>, grant: Grant) {
        let mut held = self.held();
        let now = std::time::Instant::now();
        held.waiting.retain(|w| w.until > now);
        held.waiting.push(Waiting {
            root: root.to_string(),
            bot: bot.map(str::to_string),
            grant,
            until: now + ONCE_LIFETIME,
        });
    }

    /// The grants a command in `root` (a bot's, or a person's) takes, which
    /// no other command gets after it. [`OnceGrants::restore`] puts them
    /// back, as they were, for a command that didn't start.
    pub fn take(&self, root: &str, bot: Option<&str>) -> Taken {
        let mut held = self.held();
        let now = std::time::Instant::now();
        held.waiting.retain(|w| w.until > now);
        let (mine, others): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(&mut held.waiting)
            .into_iter()
            .partition(|w| w.root == root && (w.bot.is_none() || w.bot.as_deref() == bot));
        held.waiting = others;
        Taken {
            taken: mine,
            generation: held.generation,
        }
    }

    /// Put back what a command that didn't start took, unless they were
    /// all dropped meanwhile ([`OnceGrants::clear`], [`OnceGrants::forget`]).
    pub fn restore(&self, taken: Taken) {
        let mut held = self.held();
        if held.generation == taken.generation {
            held.waiting.extend(taken.taken);
        }
    }

    /// Forget every one: this computer was paired again.
    pub fn clear(&self) {
        let mut held = self.held();
        held.waiting.clear();
        held.generation += 1;
    }

    /// Forget a folder's, when its commands stop.
    pub fn forget(&self, root: &str) {
        let mut held = self.held();
        held.waiting.retain(|w| w.root != root);
        held.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chalk() -> HostNetwork {
        HostNetwork {
            addresses: vec![
                "192.168.8.20".parse().unwrap(),
                "172.17.0.1".parse().unwrap(),
            ],
            bridges: vec![(Ipv4Addr::new(172, 17, 0, 0), 16)],
        }
    }

    #[test]
    fn addresses_are_placed_from_this_computers_own_networks() {
        let host = chalk();
        let at = |a: &str| host.classify(a.parse().unwrap());
        assert_eq!(at("127.0.0.1"), Where::ThisComputer);
        assert_eq!(at("::1"), Where::ThisComputer);
        assert_eq!(at("172.17.0.4"), Where::ThisComputer);
        assert_eq!(at("192.168.8.20"), Where::ThisComputer);
        assert_eq!(at("192.168.8.31"), Where::LocalNetwork);
        assert_eq!(at("10.1.2.3"), Where::LocalNetwork);
        assert_eq!(at("100.101.1.2"), Where::LocalNetwork);
        assert_eq!(at("fd12::1"), Where::LocalNetwork);
        assert_eq!(at("fe80::1"), Where::LocalNetwork);
        assert_eq!(at("151.101.1.227"), Where::Internet);
        assert_eq!(at("2a00:1450::1"), Where::Internet);
    }

    #[test]
    fn only_what_this_systems_sandbox_holds_is_granted() {
        use Where::*;
        assert_eq!(grantable_on(LocalNetwork, false, false), Some(LocalNetwork));
        assert_eq!(grantable_on(LocalNetwork, true, false), Some(Internet));
        assert_eq!(grantable_on(ThisComputer, true, false), Some(ThisComputer));
        assert_eq!(grantable_on(ThisComputer, false, true), None);
        assert_eq!(grantable_on(LocalNetwork, false, true), Some(Internet));
        assert_eq!(grantable_on(Internet, false, true), Some(Internet));
    }

    #[test]
    fn hosts_are_read_as_this_computer_reads_them() {
        assert_eq!(
            resolve("localhost", 5432),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(
            resolve("[::1]", 5432),
            Some(IpAddr::V6(Ipv6Addr::LOCALHOST))
        );
        assert_eq!(resolve("172.17.0.4", 5432), "172.17.0.4".parse().ok());
        assert_eq!(resolve("bad host", 1), None);
    }

    #[test]
    fn a_reach_entry_is_an_exact_local_network_address() {
        assert_eq!(
            parse_reach("192.168.1.20:5432").unwrap().to_string(),
            "192.168.1.20:5432"
        );
        assert!(parse_reach("127.0.0.1:5432").is_err());
        assert!(parse_reach("8.8.8.8:53").is_err());
        assert!(parse_reach("192.168.1.20").is_err());
        assert!(parse_reach("192.168.1.20:0").is_err());
        assert!(parse_reach("192.168.1.20:2375").is_err());
    }

    #[test]
    fn ports_that_lead_out_of_the_sandbox_are_excluded() {
        for port in [2375, 2376, 5900, 6000, 6063, 9222, 9229] {
            assert!(excluded(port), "{port}");
        }
        for port in [5432, 6379, 3306, 3000, 8080, 6064] {
            assert!(!excluded(port), "{port}");
        }
    }

    #[test]
    fn services_are_named_by_their_port() {
        assert_eq!(describe("172.17.0.4", 5432), "PostgreSQL (172.17.0.4:5432)");
        assert_eq!(describe("10.0.0.2", 4444), "10.0.0.2:4444");
    }

    #[test]
    fn the_levels_read_as_the_app_says_them() {
        assert_eq!(Level::of(true, false), Level::Local);
        assert_eq!(Level::of(false, true), Level::Internet);
        assert_eq!(Level::of(false, false), Level::None);
        assert_eq!(Level::Internet.words(), "Local services and the internet");
    }

    #[test]
    fn an_allow_once_goes_to_the_next_command_in_its_folder_alone() {
        let grants = OnceGrants::default();
        let grant = Grant {
            host: "172.17.0.4".parse().unwrap(),
            port: 5432,
            internet: false,
        };
        grants.add("app", Some("42"), grant.clone());
        assert!(grants.take("docs", Some("42")).grants().is_empty());
        assert!(grants.take("app", Some("7")).grants().is_empty());
        let taken = grants.take("app", Some("42"));
        assert_eq!(taken.grants(), vec![grant.clone()]);
        assert!(grants.take("app", Some("42")).grants().is_empty());
        // A command that didn't start gives it back.
        grants.restore(taken);
        assert_eq!(grants.take("app", Some("42")).grants(), vec![grant.clone()]);
        // Nor after the computer was paired again.
        grants.add("app", Some("42"), grant.clone());
        let taken = grants.take("app", Some("42"));
        assert_eq!(taken.grants().len(), 1);
        grants.clear();
        grants.restore(taken);
        assert!(grants.take("app", Some("42")).grants().is_empty());
        grants.add("app", None, grant.clone());
        assert_eq!(grants.take("app", None).grants(), vec![grant.clone()]);
        grants.add("app", None, grant);
        grants.clear();
        assert!(grants.take("app", None).grants().is_empty());
    }

    #[test]
    fn proc_net_lines_are_read() {
        let v4 = "   1: 0100007F:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1";
        let socket = proc_net_socket(v4, false).unwrap();
        assert_eq!(socket.address, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(socket.port, 5432);
        assert_eq!(socket.state, 0x0a);
        assert_eq!(socket.inode, 4242);
        let v6 = "   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99 1";
        let socket = proc_net_socket(v6, true).unwrap();
        assert_eq!(socket.address, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(socket.port, 8080);
    }

    #[test]
    fn lsof_listeners_are_read() {
        let text = "p881\ncControlCenter\nf9\nn*:7000\np900\ncpostgres\nf7\nn127.0.0.1:5432\nf8\nn[::1]:5432\n";
        assert_eq!(
            parse_lsof(text),
            vec![
                (
                    "controlcenter".into(),
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    7000
                ),
                ("postgres".into(), IpAddr::V4(Ipv4Addr::LOCALHOST), 5432),
                ("postgres".into(), IpAddr::V6(Ipv6Addr::LOCALHOST), 5432),
            ]
        );
    }
}
