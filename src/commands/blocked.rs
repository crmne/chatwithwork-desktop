//! What the sandbox refused a run, as its result says it (`blocked`).
//!
//! Connections come from the sandbox itself: on Bubblewrap, MXC logs every
//! connection the run's firewall blocked, one JSON line each, to a file
//! beside the run's home that the run can't write. Paths are a best guess
//! from what the run printed: an error that names a path outside its
//! folder and home, such as "Read-only file system" or "Operation not
//! permitted".

use std::net::IpAddr;
use std::path::Path;

use serde::Deserialize;

use super::Blocked;
use crate::reach::{HostNetwork, Where};

/// slirp's DNS forwarder, which a run's name lookups go to.
const DNS_FORWARDER: &str = "10.0.2.3";
/// slirp's gateway to this computer's loopback.
const HOST_GATEWAY: &str = "10.0.2.2";

#[derive(Deserialize)]
struct Line {
    protocol: String,
    host: String,
    port: u16,
    #[serde(default)]
    name: Option<String>,
}

/// One line of the sandbox's log, placed from this computer's networks. A
/// name lookup is said by the name it looked up (the internet's), and the
/// gateway to this computer's loopback as `localhost`.
pub(super) fn network(line: &str, host: &HostNetwork) -> Option<Blocked> {
    let line: Line = serde_json::from_str(line).ok()?;
    if line.host == DNS_FORWARDER && line.port == 53 {
        return Some(Blocked::Network {
            host: line.name?,
            port: 53,
            protocol: line.protocol,
            place: Where::Internet,
            service: Some("DNS".to_string()),
        });
    }
    let (shown, place) = if line.host == HOST_GATEWAY {
        ("localhost".to_string(), Where::ThisComputer)
    } else {
        let address: IpAddr = line.host.parse().ok()?;
        (line.host.clone(), host.classify(address))
    };
    Some(Blocked::Network {
        host: shown,
        port: line.port,
        protocol: line.protocol,
        place,
        service: crate::reach::service(line.port).map(str::to_string),
    })
}

/// Errors that say the sandbox refused a path, and whether each means a
/// write.
const REFUSALS: &[(&str, Option<&str>)] = &[
    ("Read-only file system", Some("write")),
    ("EROFS", Some("write")),
    ("Operation not permitted", None),
    ("EPERM", None),
    ("Permission denied", None),
    ("EACCES", None),
];

/// Words in the same line that make an unclear refusal a write.
const WRITES: &[&str] = &[
    "mkdir", "write", "create", "open '", "unlink", "rename", "rmdir", "touch", "chmod", "copy",
    "symlink", "EEXIST",
];

/// Paths outside `folder` and `home` that an error in `printed` says the
/// sandbox refused: (path shown with `~` for `person_home`, `write` or
/// `read`).
pub(super) fn paths(
    printed: &str,
    folder: &Path,
    home: &Path,
    person_home: Option<&Path>,
) -> Vec<Blocked> {
    let mut found = Vec::new();
    for line in printed.lines() {
        let Some(&(_, access)) = REFUSALS.iter().find(|(words, _)| line.contains(words)) else {
            continue;
        };
        let access = access.unwrap_or_else(|| {
            let lower = line.to_lowercase();
            if WRITES.iter().any(|w| lower.contains(&w.to_lowercase())) {
                "write"
            } else {
                "read"
            }
        });
        for path in absolute_paths(line) {
            let at = Path::new(&path);
            if at.starts_with(folder) || at.starts_with(home) || at.starts_with("/proc") {
                continue;
            }
            let shown = match person_home.and_then(|h| at.strip_prefix(h).ok()) {
                Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
                Some(rest) => format!("~/{}", rest.display()),
                None => path.clone(),
            };
            let entry = Blocked::Path {
                path: shown,
                access: access.to_string(),
            };
            if !found.contains(&entry) {
                found.push(entry);
            }
        }
    }
    found
}

/// The absolute paths in a line of output, without the quotes, colons and
/// brackets around them.
fn absolute_paths(line: &str) -> Vec<String> {
    line.split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '(' | ')' | ','))
        .filter_map(|word| {
            let word = word.trim_end_matches([':', '.', ';', ']']);
            let start = word.find('/')?;
            let path = &word[start..];
            (start == 0 && path.len() > 1 && !path.starts_with("//")).then(|| path.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn chalk() -> HostNetwork {
        HostNetwork {
            addresses: vec!["192.168.8.20".parse().unwrap()],
            bridges: vec![(Ipv4Addr::new(172, 17, 0, 0), 16)],
        }
    }

    #[test]
    fn a_blocked_connection_is_placed_and_named() {
        assert_eq!(
            network(
                r#"{"protocol":"tcp","host":"172.17.0.4","port":5432}"#,
                &chalk()
            ),
            Some(Blocked::Network {
                host: "172.17.0.4".into(),
                port: 5432,
                protocol: "tcp".into(),
                place: Where::ThisComputer,
                service: Some("PostgreSQL".into()),
            })
        );
        let lan = network(
            r#"{"protocol":"tcp","host":"192.168.8.31","port":6000}"#,
            &chalk(),
        )
        .unwrap();
        assert!(matches!(
            lan,
            Blocked::Network {
                place: Where::LocalNetwork,
                service: None,
                ..
            }
        ));
        let gateway = network(
            r#"{"protocol":"tcp","host":"10.0.2.2","port":9229}"#,
            &chalk(),
        )
        .unwrap();
        assert!(
            matches!(gateway, Blocked::Network { ref host, place: Where::ThisComputer, .. } if host == "localhost")
        );
    }

    #[test]
    fn a_blocked_name_lookup_says_the_name() {
        let lookup = network(
            r#"{"protocol":"udp","host":"10.0.2.3","port":53,"name":"rubygems.org"}"#,
            &chalk(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&lookup).unwrap(),
            serde_json::json!({
                "kind": "network", "host": "rubygems.org", "port": 53, "protocol": "udp",
                "where": "internet", "service": "DNS"
            })
        );
        assert_eq!(
            network(
                r#"{"protocol":"udp","host":"10.0.2.3","port":53}"#,
                &chalk()
            ),
            None
        );
    }

    #[test]
    fn refused_paths_outside_the_folder_are_found_in_the_output() {
        let printed = "error: EROFS: read-only file system, mkdir '/home/ana/.cache/yarn'\n\
            mkdir: cannot create directory '/usr/local/lib/x': Read-only file system\n\
            cat: /home/ana/.ssh/id_ed25519: Operation not permitted\n\
            touch: /home/ana/app/ok: Permission denied\n\
            all good in /etc/hosts\n";
        let found = paths(
            printed,
            Path::new("/home/ana/app"),
            Path::new("/home/ana/.local/share/cww/runs/abc"),
            Some(Path::new("/home/ana")),
        );
        let json = serde_json::to_value(&found).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                { "kind": "path", "path": "~/.cache/yarn", "access": "write" },
                { "kind": "path", "path": "/usr/local/lib/x", "access": "write" },
                { "kind": "path", "path": "~/.ssh/id_ed25519", "access": "read" },
            ])
        );
    }
}
