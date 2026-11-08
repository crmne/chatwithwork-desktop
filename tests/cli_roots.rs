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
