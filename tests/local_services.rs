//! Commands reaching this computer's own services, for real: Bubblewrap with
//! slirp4netns, and two throwaway PostgreSQL containers the person starts
//! first, one on Docker's bridge and one published on the loopback:
//!
//! ```sh
//! docker run --rm -d --name cwwd-ls-pg -e POSTGRES_HOST_AUTH_METHOD=trust postgres:17
//! docker run --rm -d --name cwwd-ls-pg-local -p 127.0.0.1:55432:5432 \
//!     -e POSTGRES_HOST_AUTH_METHOD=trust postgres:17
//! CWW_LS_BRIDGE_PG=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' cwwd-ls-pg) \
//! CWW_LS_LAN=192.168.1.1:80 \
//!     cargo test -p cww --test local_services -- --ignored --nocapture
//! ```
//!
//! Never starts or changes the installed daemon, and reads no one's data.
#![cfg(any(target_os = "linux", target_os = "macos"))]
#![cfg_attr(target_os = "macos", allow(dead_code, unused_imports))]

use std::time::{Duration, Instant};

use cww::commands::{Blocked, Finished, Request, Runs, Sandbox};
use cww::config::{Config, Root};
use cww::paths::Paths;
use cww::reach::{Grant, Where};

fn run(runs: &Runs, config: &Config, script: &str, once: Vec<Grant>) -> Finished {
    let started = runs
        .start(
            config,
            &Request {
                root: "shared".into(),
                dir: String::new(),
                command: script.into(),
                bot: None,
                once,
            },
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    while runs.status(&started.id).unwrap().running() {
        assert!(Instant::now() < until, "the command didn't finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    let finished = runs.finished(&started.id).unwrap();
    eprintln!("$ {script}\n{}", serde_json::to_string(&finished).unwrap());
    finished
}

/// Bash's own TCP client: connects, or says why not, within `seconds`.
fn connect(host: &str, port: u16) -> String {
    format!("timeout 10 bash -c 'exec 3<>/dev/tcp/{host}/{port}' && echo connected")
}

fn network_blocked(finished: &Finished) -> Vec<(String, u16, Where)> {
    finished
        .status
        .blocked
        .iter()
        .filter_map(|b| match b {
            Blocked::Network {
                host, port, place, ..
            } => Some((host.clone(), *port, *place)),
            Blocked::Path { .. } => None,
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs Bubblewrap, slirp4netns and the two PostgreSQL containers named above"]
fn local_services_reach_this_computer_and_nothing_else() {
    assert_eq!(Sandbox::for_host(), Ok(Sandbox::Bubblewrap));
    let bridge = std::env::var("CWW_LS_BRIDGE_PG").expect("CWW_LS_BRIDGE_PG");
    let lan = std::env::var("CWW_LS_LAN").unwrap_or_else(|_| "192.168.1.1:80".into());
    let (lan_host, lan_port) = lan.split_once(':').unwrap();
    let lan_port: u16 = lan_port.parse().unwrap();
    let home = cww::paths::home_dir().unwrap();
    let temp = tempfile::Builder::new()
        .prefix(".cww-local-services-test-")
        .tempdir_in(home)
        .unwrap();
    let base = temp.path();
    std::fs::create_dir(base.join("shared")).unwrap();
    let paths = Paths::under(&base.join("cww"));
    paths.ensure().unwrap();
    let mut config = Config {
        secret_store: Some("file".into()),
        roots: vec![Root {
            id: "shared".into(),
            label: "Synthetic shared folder".into(),
            path: base.join("shared"),
            bots: vec![],
            follow_symlinks: false,
            writable: true,
            commands: true,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.enabled = false;
    config.save(&paths).unwrap();
    let runs = Runs::new(paths.clone());

    // Local services: the container on Docker's bridge, and the one
    // published on this computer's loopback, as the run's own localhost.
    let done = run(&runs, &config, &connect(&bridge, 5432), vec![]);
    assert_eq!(done.stdout.trim(), "connected", "{done:?}");
    let done = run(&runs, &config, &connect("127.0.0.1", 55432), vec![]);
    assert_eq!(done.stdout.trim(), "connected", "{done:?}");
    let done = run(&runs, &config, &connect("localhost", 55432), vec![]);
    assert_eq!(done.stdout.trim(), "connected", "{done:?}");

    // The run's own server on its own localhost still answers it.
    let own = "python3 -c 'import socket,threading\n\
s=socket.socket(); s.bind((\"127.0.0.1\", 8765)); s.listen()\n\
threading.Thread(target=lambda: s.accept()[0].sendall(b\"own\"), daemon=True).start()\n\
c=socket.create_connection((\"127.0.0.1\", 8765)); print(c.recv(3).decode())'";
    let done = run(&runs, &config, own, vec![]);
    assert_eq!(done.stdout.trim(), "own", "{done:?}");

    // The internet: refused at once, and said.
    let started = Instant::now();
    let done = run(&runs, &config, &connect("1.1.1.1", 443), vec![]);
    assert_ne!(done.status.exit_code, Some(0));
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "it waited: {done:?}"
    );
    assert!(
        network_blocked(&done).contains(&("1.1.1.1".into(), 443, Where::Internet)),
        "{done:?}"
    );
    // A name lookup: said by its name.
    let done = run(&runs, &config, "getent hosts rubygems.org", vec![]);
    assert_ne!(done.status.exit_code, Some(0));
    assert!(
        network_blocked(&done).contains(&("rubygems.org".into(), 53, Where::Internet)),
        "{done:?}"
    );
    // The local network: refused, and said.
    let done = run(&runs, &config, &connect(lan_host, lan_port), vec![]);
    assert_ne!(done.status.exit_code, Some(0));
    assert!(
        network_blocked(&done).contains(&(lan_host.into(), lan_port, Where::LocalNetwork)),
        "{done:?}"
    );
    // ...unless allowed once for this command.
    let done = run(
        &runs,
        &config,
        &connect(lan_host, lan_port),
        vec![Grant {
            host: lan_host.parse().unwrap(),
            port: lan_port,
            internet: false,
        }],
    );
    assert_eq!(done.stdout.trim(), "connected", "{done:?}");

    // No network at all: this computer's services are refused too, and said.
    config.roots[0].local_services = false;
    let done = run(&runs, &config, &connect(&bridge, 5432), vec![]);
    assert_ne!(done.status.exit_code, Some(0));
    assert!(
        network_blocked(&done).contains(&(bridge.clone(), 5432, Where::ThisComputer)),
        "{done:?}"
    );

    // The internet: everything, local services included.
    config.roots[0].network = true;
    for (host, port) in [
        ("1.1.1.1", 443),
        (bridge.as_str(), 5432),
        ("127.0.0.1", 55432),
    ] {
        let done = run(&runs, &config, &connect(host, port), vec![]);
        assert_eq!(done.stdout.trim(), "connected", "{done:?}");
    }
}

/// macOS, Seatbelt: a service this computer serves on its loopback (a
/// throwaway Python server the test starts), and the internet refused at
/// once.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "macOS: runs Seatbelt for real"]
fn local_services_reach_this_mac_and_not_the_internet() {
    let mut service = std::process::Command::new("/usr/bin/python3")
        .args(["-m", "http.server", "18765", "--bind", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let home = cww::paths::home_dir().unwrap();
    let temp = tempfile::Builder::new()
        .prefix(".cww-local-services-test-")
        .tempdir_in(home)
        .unwrap();
    let base = temp.path();
    std::fs::create_dir(base.join("shared")).unwrap();
    let paths = Paths::under(&base.join("cww"));
    paths.ensure().unwrap();
    let mut config = Config {
        secret_store: Some("file".into()),
        roots: vec![Root {
            id: "shared".into(),
            label: "Synthetic shared folder".into(),
            path: base.join("shared"),
            bots: vec![],
            follow_symlinks: false,
            writable: true,
            commands: true,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.enabled = false;
    config.save(&paths).unwrap();
    let runs = Runs::new(paths.clone());
    let probe = |host: &str, port: u16| {
        format!(
            "/usr/bin/python3 -c 'import socket; socket.create_connection((\"{host}\", {port}), 5); print(\"connected\")'"
        )
    };
    let own = "/usr/bin/python3 -c 'import socket,threading\n\
s=socket.socket(); s.bind((\"127.0.0.1\", 18766)); s.listen()\n\
threading.Thread(target=lambda: s.accept()[0].sendall(b\"own\"), daemon=True).start()\n\
c=socket.create_connection((\"127.0.0.1\", 18766)); print(c.recv(3).decode())'";

    let local = run(&runs, &config, &probe("127.0.0.1", 18765), vec![]);
    let localhost = run(&runs, &config, &probe("localhost", 18765), vec![]);
    let own_server = run(&runs, &config, own, vec![]);
    let started = Instant::now();
    let internet = run(&runs, &config, &probe("1.1.1.1", 443), vec![]);
    let refused_in = started.elapsed();
    config.roots[0].local_services = false;
    let none = run(&runs, &config, &probe("127.0.0.1", 18765), vec![]);
    let none_own = run(&runs, &config, own, vec![]);
    config.roots[0].network = true;
    let open_internet = run(&runs, &config, &probe("1.1.1.1", 443), vec![]);
    let open_local = run(&runs, &config, &probe("127.0.0.1", 18765), vec![]);
    let _ = service.kill();
    let _ = service.wait();

    assert_eq!(local.stdout.trim(), "connected", "{local:?}");
    assert_eq!(localhost.stdout.trim(), "connected", "{localhost:?}");
    // Seatbelt can't hold a listener to this machine alone (a server on
    // every address would take connections from the network), so a run
    // serves nothing on macOS, as before (docs/commands.md).
    eprintln!("own server with local services: {own_server:?}");
    assert_ne!(internet.status.exit_code, Some(0), "{internet:?}");
    assert!(
        refused_in < Duration::from_secs(4),
        "it waited: {internet:?}"
    );
    assert_ne!(none.status.exit_code, Some(0), "{none:?}");
    eprintln!("own server with no network: {none_own:?}");
    assert_eq!(
        open_internet.stdout.trim(),
        "connected",
        "{open_internet:?}"
    );
    assert_eq!(open_local.stdout.trim(), "connected", "{open_local:?}");
}
