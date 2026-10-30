//! Explicit host integration check, never starts or changes the installed daemon.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use cww::commands::{Finished, Request, Runs, Sandbox};
use cww::config::{Config, Root};
use cww::paths::Paths;

const CHILD: &str = "CWW_SYNTHETIC_SYSTEMD_TEST";

fn command(runs: &Runs, config: &Config, script: String) -> Finished {
    let started = runs
        .start(
            config,
            &Request {
                root: "shared".into(),
                dir: String::new(),
                command: script,
            },
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while runs.status(&started.id).unwrap().running() {
        assert!(Instant::now() < until, "synthetic command did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = runs.finished(&started.id).unwrap();
    assert_eq!(result.status.exit_code, Some(0), "{result:?}");
    result
}

fn exercise(base: &Path) {
    let paths = Paths::under(&base.join("cww"));
    let mut config = Config::load(&paths).unwrap();
    assert_eq!(Sandbox::for_host(), Ok(Sandbox::Bubblewrap));
    let runs = Runs::new(paths.clone());
    let script = format!(
        "set -eu; printf changed > result.txt; test -z \"${{CWW_SYNTHETIC_PARENT:-}}\"; \
         test ! -e '{private}'; (printf overwritten > '{private}') 2>/dev/null || true; \
         /usr/bin/node -e 'console.log(6 * 7)'",
        private = base.join("private.txt").display(),
    );
    let result = command(&runs, &config, script);
    assert_eq!(result.stdout.trim(), "42");
    // MXC may create an ephemeral path under its private mount tree. It must
    // never expose or mutate the adjacent host file at the same pathname.
    assert_eq!(
        std::fs::read_to_string(base.join("shared/result.txt")).unwrap(),
        "changed"
    );
    assert_eq!(
        std::fs::read_to_string(base.join("private.txt")).unwrap(),
        "private synthetic"
    );
    // Starting an allowed-network sandbox exercises slirp, without contacting
    // any external server or reading any user's data.
    config.roots[0].network = true;
    assert_eq!(
        command(&runs, &config, "printf network-ready".into()).stdout,
        "network-ready"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cww"))
        .env("CWW_HOME", base.join("cww"))
        .args(["debug", "sandbox", "--write"])
        .arg(base.join("shared/worker-write.txt"))
        .arg("--write")
        .arg(base.join("private.txt"))
        .arg(base.join("private.txt"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\"state\":\"enforced\""), "{text}");
    assert!(
        text.contains("wrote ") && text.contains("worker-write.txt"),
        "{text}"
    );
    assert!(
        text.contains("refused writing ") && text.contains("refused "),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(base.join("private.txt")).unwrap(),
        "private synthetic"
    );
}

#[test]
#[ignore = "requires a real systemd user session, Bubblewrap, slirp4netns and Node"]
fn commands_and_worker_remain_confined_under_the_packaged_unit() {
    if let Some(base) = std::env::var_os(CHILD) {
        exercise(Path::new(&base));
        return;
    }
    let home = cww::paths::home_dir().unwrap();
    let temp = tempfile::Builder::new()
        .prefix(".cww-unit-test-")
        .tempdir_in(home)
        .unwrap();
    let base = temp.path();
    std::fs::create_dir(base.join("shared")).unwrap();
    std::fs::write(base.join("private.txt"), "private synthetic").unwrap();
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
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.enabled = false;
    // V8 reserves a large virtual cage even for this tiny script. This test
    // isolates inherited executable-memory policy from the separate run limit.
    config.limits.command_memory_mb = 0;
    config.save(&paths).unwrap();
    let mut run = Command::new("systemd-run");
    run.args(["--user", "--quiet", "--wait", "--collect", "--pipe"])
        .arg(format!("--unit=cww-unit-test-{}", std::process::id()));
    let mut section = "";
    for line in include_str!("../packaging/systemd/cww.service")
        .lines()
        .map(str::trim)
    {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        if section != "[Service]" || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, _) = line.split_once('=').unwrap();
        if name.starts_with("Exec")
            || matches!(
                name,
                "Type"
                    | "Restart"
                    | "RestartSec"
                    | "RuntimeDirectory"
                    | "RuntimeDirectoryMode"
                    | "ReadWritePaths"
            )
        {
            continue;
        }
        run.arg("-p").arg(line);
    }
    run.args([
        "-p",
        "RuntimeMaxSec=45",
        "-p",
        "TimeoutStopSec=2",
        "-p",
        "LimitCORE=0",
    ])
    .arg("-p")
    .arg(format!("Environment={CHILD}={}", base.display()))
    .args([
        "-p",
        "Environment=CWW_SYNTHETIC_PARENT=must-not-reach-command",
    ])
    .arg(std::env::current_exe().unwrap())
    .args([
        "--ignored",
        "--exact",
        "commands_and_worker_remain_confined_under_the_packaged_unit",
        "--nocapture",
    ]);
    let output = run.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
}
