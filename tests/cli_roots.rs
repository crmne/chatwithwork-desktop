//! `cww roots` edits the config itself when it runs: what it changes there
//! is audited as the daemon audits the same change made through it.

use std::path::Path;
use std::process::Command;

use cww::paths::Paths;
use serde_json::Value;

fn cww(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_cww"))
        .args(args)
        .env("CWW_HOME", home)
        .env("CWW_SECRET_STORE", "file")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cww {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn removing_a_folder_from_the_terminal_is_audited() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("cww");
    let folder = base.path().join("Projects");
    std::fs::create_dir_all(&folder).unwrap();

    cww(
        &home,
        &[
            "roots",
            "add",
            folder.to_str().unwrap(),
            "--label",
            "Projects",
        ],
    );
    let config = cww::config::Config::load(&Paths::under(&home)).unwrap();
    let id = config.roots[0].id.clone();
    let said = cww(&home, &["roots", "remove", "Projects"]);
    assert!(said.contains("Stopped sharing"), "{said}");

    let log = std::fs::read_to_string(Paths::under(&home).audit_file()).unwrap();
    let removed: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["event"] == "root_removed")
        .collect();
    assert_eq!(removed.len(), 1, "{log}");
    assert_eq!(removed[0]["detail"], format!("{id} (Projects)"));
}

/// With the agent running, the removal goes through it: it audits it once,
/// as it does a removal made in the app.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn removing_a_folder_with_the_agent_running_goes_through_it() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    struct Agent(std::process::Child);
    impl Drop for Agent {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    // The agent's macOS sandbox names real paths: /private/var, not /var.
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("cww");
    let folder = base.join("Projects");
    std::fs::create_dir_all(&folder).unwrap();
    cww(
        &home,
        &[
            "roots",
            "add",
            folder.to_str().unwrap(),
            "--label",
            "Projects",
        ],
    );
    let paths = Paths::under(&home);
    let mut config = cww::config::Config::load(&paths).unwrap();
    config.index.watch = false;
    config.secret_store = Some("file".into());
    config.save(&paths).unwrap();
    let id = config.roots[0].id.clone();

    let _agent = Agent(
        Command::new(env!("CARGO_BIN_EXE_cww"))
            .args(["daemon", "run"])
            .env("CWW_HOME", &home)
            .env("CWW_SECRET_STORE", "file")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(base.join("agent.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    loop {
        let status: Value =
            serde_json::from_str(&cww(&home, &["status", "--json"])).unwrap_or_default();
        if !status["pid"].is_null() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the agent didn't start: {}",
            std::fs::read_to_string(base.join("agent.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let said = cww(&home, &["roots", "remove", "Projects"]);
    assert!(said.contains("Stopped sharing"), "{said}");
    // The by-hand path tells the agent afterwards; this one didn't need to.
    assert!(!said.contains("picked up the change"), "{said}");
    assert!(cww::config::Config::load(&paths).unwrap().roots.is_empty());
    let log = std::fs::read_to_string(paths.audit_file()).unwrap();
    let removed: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["event"] == "root_removed")
        .collect();
    assert_eq!(removed.len(), 1, "{log}");
    assert_eq!(removed[0]["detail"], format!("{id} (Projects)"));
}
