//! Large local indexes must not hold control/status or sandbox restarts open.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cww::config::{Config, Root};
use cww::control::{BoundedClient, ControlRequest};
use cww::paths::Paths;
use cww::reader::index::SearchIndex;
use serde_json::Value;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        let _ = self.0.wait();
    }
}

fn status(client: &mut BoundedClient, accepts: impl Fn(&Value) -> bool) -> Value {
    let start = Instant::now();
    loop {
        if let Ok(value) = client.request(&ControlRequest::Status)
            && accepts(&value)
        {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "control did not recover"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn status_and_sandbox_restart_work_during_a_large_index_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let docs = base.join("docs");
    std::fs::create_dir(&docs).unwrap();
    for i in 0..10000 {
        std::fs::write(docs.join(format!("file-{i}.bin")), "synthetic").unwrap();
    }
    let home = base.join("cww");
    let paths = Paths::under(&home);
    let mut config = Config {
        paused: true,
        secret_store: Some("file".into()),
        roots: vec![Root {
            id: "docs".into(),
            label: "Synthetic documents".into(),
            path: docs,
            bots: vec![],
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.watch = false;
    config.save(&paths).unwrap();
    {
        let index = SearchIndex::open(&paths.index_dir()).unwrap();
        for i in 0..100000 {
            index
                .upsert("docs", &format!("old-{i}.bin"), "", 1, 1)
                .unwrap();
        }
        index.commit().unwrap();
    }
    let started = Instant::now();
    let mut process = Process(
        Command::new(env!("CARGO_BIN_EXE_cww"))
            .env("CWW_HOME", &home)
            .args(["daemon", "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut client = BoundedClient::new(paths.socket_path(), Duration::from_millis(500));
    let initial = status(&mut client, |_| true);
    let startup = started.elapsed();
    assert_eq!(initial["paused"], true);
    assert_eq!(initial["paired"], false);
    assert_ne!(
        initial["roots"][0]["index"], "ready",
        "control must answer before the full scan"
    );
    let scanning = status(&mut client, |s| s["roots"][0]["index"] == "indexing");
    let initial_pid = scanning["pid"].clone();
    let started = Instant::now();
    // The request may complete just as its old socket closes. Its persisted
    // setting, never a second mutation, is used to observe completion.
    let _ = client.request(&ControlRequest::RootsWritable {
        root: "docs".into(),
        writable: true,
        i_know: false,
    });
    let restarted = status(&mut client, |s| {
        s["pid"] != initial_pid && s["roots"][0]["writable"] == true
    });
    let restart = started.elapsed();
    assert_eq!(restarted["paused"], true);
    assert_eq!(restarted["paired"], false);
    assert!(Config::load(&paths).unwrap().roots[0].writable);
    assert!(
        process.0.try_wait().unwrap().is_none(),
        "supervisor remains alive"
    );
    eprintln!(
        "100,000 index entries / 10,000 files: status {startup:?}, sandbox restart {restart:?}"
    );
    client.request(&ControlRequest::Shutdown).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "shutdown waited on indexing");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_outdated_supervisor_cannot_run_a_new_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    Config {
        secret_store: Some("file".into()),
        ..Config::default()
    }
    .save(&paths)
    .unwrap();
    for previous in [None, Some("0.9.0"), Some("future-version")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cww"));
        command
            .env("CWW_HOME", tmp.path())
            .args(["daemon", "run", "--worker"]);
        match previous {
            Some(version) => {
                command.env("CWW_SUPERVISOR_VERSION", version);
            }
            None => {
                command.env_remove("CWW_SUPERVISOR_VERSION");
            }
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("restart the full background agent")
        );
        assert!(
            !paths.socket_path().exists(),
            "mixed versions must never serve local settings or tools"
        );
    }
}
