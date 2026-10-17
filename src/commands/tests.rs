use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::config::{Config, Root};

/// Bubblewrap needs `bwrap` and unprivileged user namespaces; machines
/// without them skip the tests that run something.
fn sandbox_works() -> bool {
    std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-user", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Fixture {
    _dir: tempfile::TempDir,
    folder: std::path::PathBuf,
    config: Config,
    runs: Runs,
}

fn fixture(commands: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("project");
    std::fs::create_dir_all(folder.join("src")).unwrap();
    std::fs::write(folder.join("README.md"), "hello\n").unwrap();
    let paths = Paths::under(&dir.path().join("cww"));
    paths.ensure().unwrap();
    let mut config = Config::default();
    config.roots.push(Root {
        id: "project".into(),
        label: "Project".into(),
        path: folder.clone(),
        follow_symlinks: false,
        writable: true,
        commands,
        network: false,
    });
    config.limits.command_timeout_seconds = 20;
    Fixture {
        _dir: dir,
        folder,
        config,
        runs: Runs::new(paths),
    }
}

fn request(command: &str) -> Request {
    Request {
        root: "project".into(),
        dir: String::new(),
        command: command.into(),
    }
}

fn wait(runs: &Runs, id: &str) -> Finished {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if !runs.status(id).unwrap().running() {
            return runs.finished(id).unwrap();
        }
        assert!(Instant::now() < deadline, "run {id} didn't end");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn only_folders_that_allow_commands_run_them() {
    let fx = fixture(false);
    let err = fx.runs.start(&fx.config, &request("true")).unwrap_err();
    assert_eq!(err.code, "not_allowed");
    let mut config = fx.config.clone();
    config.roots[0].commands = true;
    config.roots[0].writable = false;
    assert_eq!(
        fx.runs.start(&config, &request("true")).unwrap_err().code,
        "not_allowed"
    );
}

#[test]
fn the_working_folder_stays_inside_the_shared_folder() {
    let fx = fixture(true);
    let paths = Paths::under(Path::new("/nonexistent"));
    for dir in ["..", "../..", "/etc", "src/../..", "a\\b"] {
        let req = Request {
            dir: dir.into(),
            ..request("true")
        };
        let err = plan(&fx.config, &paths, &req, "000000000000").unwrap_err();
        assert!(
            matches!(err.code.as_str(), "invalid_path" | "not_found"),
            "{dir}: {err:?}"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", fx.folder.join("out")).unwrap();
        let req = Request {
            dir: "out".into(),
            ..request("true")
        };
        assert_eq!(
            plan(&fx.config, &paths, &req, "000000000000")
                .unwrap_err()
                .code,
            "invalid_path"
        );
    }
    let req = Request {
        dir: "src".into(),
        ..request("true")
    };
    let plan = plan(&fx.config, &paths, &req, "000000000000").unwrap();
    assert!(plan.cwd.ends_with("project/src"));
    assert!(!plan.network);
    for command in ["", "   ", "a\0b"] {
        let err = super::plan(&fx.config, &paths, &request(command), "000000000000").unwrap_err();
        assert_eq!(err.code, "invalid");
    }
}

#[test]
fn the_request_holds_the_run_to_its_folder_without_network() {
    let fx = fixture(true);
    let paths = Paths::under(Path::new("/x/cww"));
    let plan = plan(&fx.config, &paths, &request("make test"), "0123456789ab").unwrap();
    let json: serde_json::Value = serde_json::from_str(&plan.request_json(None)).unwrap();
    assert_eq!(json["network"]["egress"]["default"], "deny");
    assert_eq!(json["process"]["commandLine"], "make test");
    let writable = json["filesystem"]["readwritePaths"].as_array().unwrap();
    assert_eq!(writable.len(), 2);
    assert!(
        writable[1]
            .as_str()
            .unwrap()
            .ends_with("data/runs/0123456789ab")
    );
    assert_eq!(json["resources"]["cpuTimeMs"], 600_000);
    assert_eq!(json["resources"]["memoryMb"], 4096);
    let env: Vec<&str> = json["process"]["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        env.iter()
            .any(|e| e.starts_with("HOME=/x/cww/data/runs/0123456789ab"))
    );
    assert!(env.iter().all(|e| !e.starts_with("CWW_")), "{env:?}");
    let mut fx = fx;
    fx.config.roots[0].network = true;
    let plan = super::plan(&fx.config, &paths, &request("make"), "0123456789ab").unwrap();
    let json: serde_json::Value = serde_json::from_str(&plan.request_json(None)).unwrap();
    assert_eq!(json["network"]["egress"]["default"], "allow");
}

#[test]
fn a_command_runs_in_its_folder_and_reports_how_it_ended() {
    if !sandbox_works() {
        return;
    }
    let fx = fixture(true);
    let status = fx
        .runs
        .start(
            &fx.config,
            &request("cat README.md; echo oops >&2; echo made > out.txt; exit 3"),
        )
        .unwrap();
    assert!(valid_id(&status.id));
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "exited", "{done:?}");
    assert_eq!(done.status.exit_code, Some(3));
    assert_eq!(done.stdout, "hello\n");
    assert_eq!(done.stderr, "oops\n");
    assert_eq!(
        std::fs::read_to_string(fx.folder.join("out.txt")).unwrap(),
        "made\n"
    );
    let output = fx.runs.output(&status.id, 0, 1024).unwrap();
    assert!(output.done);
    assert!(output.text.contains("hello") && output.text.contains("oops"));
}

#[test]
fn a_command_writes_only_its_folder_and_gets_a_clean_environment() {
    if !sandbox_works() {
        return;
    }
    let fx = fixture(true);
    let outside = fx.folder.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "keep\n").unwrap();
    // SAFETY: tests set this before any run reads the environment.
    unsafe { std::env::set_var("CWW_SECRET_FOR_TEST", "leak") };
    let command = format!(
        "echo changed > {} 2>/dev/null; echo \"secret=$CWW_SECRET_FOR_TEST\"; echo \"home=$HOME\"; touch \"$HOME/ok\" && echo home-writable",
        outside.display()
    );
    let status = fx.runs.start(&fx.config, &request(&command)).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
    assert!(done.stdout.contains("secret=\n"), "{done:?}");
    assert!(done.stdout.contains("home-writable"), "{done:?}");
    assert!(done.stdout.contains("home=/"), "{done:?}");
    // Its home goes when it ends.
    let home = done
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("home="))
        .unwrap()
        .to_string();
    assert!(!Path::new(&home).exists());
}

#[test]
fn a_command_has_no_network_unless_the_folder_allows_it() {
    if !sandbox_works() {
        return;
    }
    let fx = fixture(true);
    // Only the loopback interface, and it's down: nothing to reach.
    let status = fx
        .runs
        .start(
            &fx.config,
            &request("tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '"),
        )
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.stdout.trim(), "lo", "{done:?}");
}

#[test]
fn a_command_is_stopped_on_request_and_at_its_time_limit() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    let status = fx
        .runs
        .start(&fx.config, &request("echo started; sleep 30"))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fx
        .runs
        .output(&status.id, 0, 100)
        .unwrap()
        .text
        .contains("started")
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    fx.runs.stop(&status.id).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "stopped");
    assert!(done.status.duration_ms < 10_000);

    fx.config.limits.command_timeout_seconds = 1;
    let status = fx.runs.start(&fx.config, &request("sleep 30")).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "timed_out");
}

#[test]
fn output_is_read_in_pieces_and_the_result_keeps_the_end() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    fx.config.limits.command_output_chars = 100;
    let status = fx.runs.start(&fx.config, &request("seq 1 2000")).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.stdout.chars().count(), 100);
    assert!(done.stdout.ends_with("1999\n2000\n"));
    assert!(done.stdout_cut > 0);
    let mut offset = 0;
    let mut text = String::new();
    loop {
        let piece = fx.runs.output(&status.id, offset, 1000).unwrap();
        text.push_str(&piece.text);
        offset = piece.next_offset;
        if piece.done {
            break;
        }
    }
    assert_eq!(text.lines().count(), 2000);
}

#[test]
fn too_many_running_commands_wait_their_turn() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    fx.config.limits.commands_running = 1;
    let first = fx.runs.start(&fx.config, &request("sleep 30")).unwrap();
    assert_eq!(
        fx.runs
            .start(&fx.config, &request("true"))
            .unwrap_err()
            .code,
        "busy"
    );
    fx.config.roots[0].commands = false;
    fx.runs.stop_outside(&fx.config);
    assert_eq!(wait(&fx.runs, &first.id).status.state, "stopped");
    assert_eq!(
        fx.runs.status("ffffffffffff").unwrap_err().code,
        "not_found"
    );
}

#[test]
fn utf8_is_never_cut_in_the_middle_of_a_character() {
    let mut cell = Cell::new(
        "0".into(),
        "r".into(),
        &Limits {
            cpu_seconds: 1,
            memory_mb: 1,
            timeout_seconds: 1,
            output_chars: 2,
        },
    );
    cell.append(Stream::Out, "héllo".as_bytes());
    let piece = cell.output(0, 2);
    assert_eq!(piece.text, "h");
    assert_eq!(piece.next_offset, 1);
    let piece = cell.output(1, 2);
    assert_eq!(piece.text, "é");
    cell.finish(State::Exited(0));
    let done = cell.finished(2);
    assert_eq!(done.stdout, "lo");
    assert_eq!(done.stdout_cut, 3);
}

/// Manual check of the host's toolchains: `cargo test commands::tests::toolchains -- --ignored --nocapture`.
#[test]
#[ignore]
fn toolchains() {
    let fx = fixture(true);
    let status = fx
        .runs
        .start(
            &fx.config,
            &request("echo $PATH; ruby -v; node -v; python3 -V; cargo -V; git --version"),
        )
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    println!("{}\n{}", done.stdout, done.stderr);
}
