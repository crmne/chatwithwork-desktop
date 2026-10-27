use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::config::{Config, Root};

/// The tests that run something run on Linux, where Bubblewrap or Landlock
/// works. Run them again under
/// `bwrap --dev-bind / / --unshare-user --disable-userns --` to try Landlock.
fn sandbox_works() -> bool {
    cfg!(target_os = "linux") && Sandbox::for_host().is_ok()
}

fn on_landlock() -> bool {
    Sandbox::for_host() == Ok(Sandbox::Landlock)
}

/// Whether a run with `config` reaches a server on this computer's
/// loopback. `None` without bash, which dials it.
fn reaches_this_computers_loopback(fx: &Fixture, config: &Config) -> Option<bool> {
    let bash = std::process::Command::new("bash")
        .args(["-c", "true"])
        .status();
    if !bash.is_ok_and(|s| s.success()) {
        return None;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dial = format!("bash -c 'echo hi > /dev/tcp/127.0.0.1/{port}'");
    let status = fx.runs.start(config, &request(&dial)).unwrap();
    wait(&fx.runs, &status.id);
    Some(listener.accept().is_ok())
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
        bots: Vec::new(),
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

/// A request for a command that uses tools like `cat`, `sleep` and `seq`.
/// A run's `PATH` names only the usual system folders, which Nix's build
/// sandbox leaves without coreutils, so the command also looks where this
/// test found its own: the run can read `/nix`, so they work there too.
/// Elsewhere the system folders come first and nothing changes.
fn shell(command: &str) -> Request {
    let ours = std::env::var("PATH").unwrap_or_default();
    let dirs: Vec<&str> = ours
        .split(':')
        .filter(|dir| dir.starts_with('/') && !dir.contains('\''))
        .collect();
    if dirs.is_empty() {
        return request(command);
    }
    request(&format!("PATH=\"$PATH:{}\"; {command}", dirs.join(":")))
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

/// The Linux and macOS requests.
#[cfg(unix)]
#[test]
fn the_request_holds_the_run_to_its_folder_without_network() {
    let fx = fixture(true);
    let paths = Paths::under(Path::new("/x/cww"));
    let plan = plan(&fx.config, &paths, &request("make test"), "0123456789ab").unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&plan.request_json(None, Sandbox::Bubblewrap)).unwrap();
    assert_eq!(json["containment"], "process");
    assert_eq!(json["network"]["egress"]["default"], "deny");
    assert!(json["network"].get("ingress").is_none());
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
    if cfg!(target_os = "macos") {
        assert!(json["resources"].get("memoryMb").is_none(), "{json}");
    } else {
        assert_eq!(json["resources"]["memoryMb"], 4096);
    }
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
    let json: serde_json::Value =
        serde_json::from_str(&plan.request_json(None, Sandbox::Bubblewrap)).unwrap();
    assert_eq!(json["network"]["egress"]["default"], "allow");
    assert!(json["network"].get("ingress").is_none());
}

/// The Linux and macOS requests.
#[cfg(unix)]
#[test]
fn landlock_refuses_commands_the_network() {
    let mut fx = fixture(true);
    let paths = Paths::under(Path::new("/x/cww"));
    let offline = plan(&fx.config, &paths, &request("make"), "0123456789ab").unwrap();
    assert_eq!(Sandbox::Landlock.refuses(&offline), None);
    let json: serde_json::Value =
        serde_json::from_str(&offline.request_json(None, Sandbox::Landlock)).unwrap();
    assert_eq!(json["containment"], "process");
    assert_eq!(json["network"]["egress"]["default"], "deny");
    assert!(json["network"].get("ingress").is_none());
    fx.config.roots[0].network = true;
    let online = plan(&fx.config, &paths, &request("make"), "0123456789ab").unwrap();
    let refused = Sandbox::Landlock.refuses(&online).unwrap();
    assert_eq!(refused.code, "network_unavailable");
    assert!(
        refused.message.contains("Turn the network off"),
        "{refused:?}"
    );
    assert_eq!(Sandbox::Bubblewrap.refuses(&online), None);
    assert_eq!(Sandbox::Seatbelt.refuses(&online), None);
}

/// The Linux and macOS requests.
#[cfg(unix)]
#[test]
fn on_landlock_toolchains_inside_the_folder_are_left_to_it() {
    let fx = fixture(true);
    // A home folder that is the shared folder, with a toolchain in it.
    std::fs::create_dir_all(fx.folder.join(".cargo/bin")).unwrap();
    let paths = Paths::under(Path::new("/x/cww"));
    let plan = plan(&fx.config, &paths, &request("make"), "0123456789ab").unwrap();
    let readonly = |sandbox| -> Vec<serde_json::Value> {
        let json: serde_json::Value =
            serde_json::from_str(&plan.request_json(Some(&fx.folder), sandbox)).unwrap();
        json["filesystem"]["readonlyPaths"]
            .as_array()
            .unwrap()
            .clone()
    };
    let cargo = fx.folder.join(".cargo/bin").canonicalize().unwrap();
    let cargo = serde_json::Value::from(cargo.to_string_lossy().into_owned());
    assert!(readonly(Sandbox::Bubblewrap).contains(&cargo));
    assert!(!readonly(Sandbox::Landlock).contains(&cargo));
}

/// The Linux and macOS requests.
#[cfg(unix)]
#[test]
fn seatbelt_is_named() {
    let fx = fixture(true);
    let paths = Paths::under(Path::new("/x/cww"));
    let plan = plan(&fx.config, &paths, &request("make"), "0123456789ab").unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&plan.request_json(None, Sandbox::Seatbelt)).unwrap();
    assert_eq!(json["containment"], "seatbelt");
    assert_eq!(json["seatbelt"]["guiAccess"], false);
    // MXC refuses a memory limit on Seatbelt, and with it every run.
    assert!(json["resources"].get("memoryMb").is_none(), "{json}");
    assert_eq!(json["resources"]["cpuTimeMs"], 600_000);
}

#[test]
fn macos_promises_no_memory_or_process_limit() {
    let limits = Limits::from_config(&crate::config::Limits::default());
    if cfg!(target_os = "macos") {
        assert_eq!((limits.memory_mb, limits.processes), (0, 0));
    } else {
        assert_eq!((limits.memory_mb, limits.processes), (4096, 256));
    }
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
            &shell("cat README.md; echo oops >&2; echo made > out.txt; exit 3"),
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let command = format!(
        "echo changed > {0} 2>/dev/null; chmod 644 {0} 2>/dev/null; echo \"secret=$CWW_SECRET_FOR_TEST\"; echo \"home=$HOME\"; touch \"$HOME/ok\" && echo home-writable",
        outside.display()
    );
    let status = fx.runs.start(&fx.config, &shell(&command)).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&outside).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "its mode changed");
    }
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
    let mut fx = fixture(true);
    if !on_landlock() {
        // Only the loopback interface, and it's down: nothing to reach.
        let status = fx
            .runs
            .start(
                &fx.config,
                &shell("tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '"),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.stdout.trim(), "lo", "{done:?}");
    }
    assert_ne!(reaches_this_computers_loopback(&fx, &fx.config), Some(true));
    // Landlock could only give it this computer's whole network, so a
    // folder that allows the network runs nothing there.
    if on_landlock() {
        fx.config.roots[0].network = true;
        let status = fx.runs.start(&fx.config, &request("true")).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.state, "failed", "{done:?}");
        assert!(
            done.status
                .error
                .as_deref()
                .is_some_and(|e| e.contains("Turn the network off")),
            "{done:?}"
        );
    }
}

#[test]
fn a_command_is_stopped_on_request_and_at_its_time_limit() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    let status = fx
        .runs
        .start(&fx.config, &shell("echo started; sleep 30"))
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
    let status = fx.runs.start(&fx.config, &shell("sleep 30")).unwrap();
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
    let status = fx.runs.start(&fx.config, &shell("seq 1 2000")).unwrap();
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
    let first = fx.runs.start(&fx.config, &shell("sleep 30")).unwrap();
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
fn a_paused_computer_runs_nothing_and_stops_what_runs() {
    let mut fx = fixture(true);
    fx.config.paused = true;
    assert_eq!(
        fx.runs
            .start(&fx.config, &request("true"))
            .unwrap_err()
            .code,
        "paused"
    );
    if !sandbox_works() {
        return;
    }
    fx.config.paused = false;
    let running = fx.runs.start(&fx.config, &shell("sleep 30")).unwrap();
    fx.config.paused = true;
    fx.runs.stop_outside(&fx.config);
    assert_eq!(wait(&fx.runs, &running.id).status.state, "stopped");
}

#[test]
fn the_list_says_what_runs_where_running_first() {
    if !sandbox_works() {
        return;
    }
    let fx = fixture(true);
    let done = fx.runs.start(&fx.config, &request("true")).unwrap();
    wait(&fx.runs, &done.id);
    let req = Request {
        dir: "src".into(),
        ..shell("sleep 30")
    };
    let running = fx.runs.start(&fx.config, &req).unwrap();
    let list = fx.runs.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].status.id, running.id);
    assert_eq!(list[0].command, req.command);
    assert_eq!(list[0].dir, "src");
    assert_eq!(list[0].root, "project");
    assert!(list[0].started_at > 0);
    assert_eq!(list[1].status.state, "exited");
    fx.runs.stop_all();
    wait(&fx.runs, &running.id);
}

#[test]
fn a_command_is_stopped_at_its_process_limit() {
    if !sandbox_works() || !cfg!(target_os = "linux") {
        return;
    }
    let mut fx = fixture(true);
    fx.config.limits.command_processes = 4;
    let status = fx
        .runs
        .start(
            &fx.config,
            &shell("for i in 1 2 3 4 5 6 7 8 9 10; do sleep 20 & done; wait"),
        )
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "stopped");
    assert_eq!(done.status.limit.as_deref(), Some("processes"));
    assert!(done.status.duration_ms < 10_000);

    fx.config.limits.command_processes = 64;
    let status = fx
        .runs
        .start(&fx.config, &shell("sleep 0.3 & sleep 0.3 & wait"))
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "exited");
    assert_eq!(done.status.limit, None);
}

#[test]
fn a_command_that_spins_ends_at_its_cpu_limit() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    fx.config.limits.command_cpu_seconds = 1;
    let status = fx
        .runs
        .start(&fx.config, &request("while :; do :; done"))
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "exited", "{done:?}");
    assert_eq!(done.status.limit.as_deref(), Some("cpu_time"), "{done:?}");
}

#[test]
fn utf8_is_never_cut_in_the_middle_of_a_character() {
    let plan = Plan {
        id: "0".into(),
        command: "true".into(),
        folder: "/".into(),
        cwd: "/".into(),
        network: false,
        limits: Limits {
            cpu_seconds: 1,
            memory_mb: 1,
            timeout_seconds: 1,
            processes: 1,
            output_chars: 2,
        },
        home: "/".into(),
    };
    let mut cell = Cell::new(&plan, "r".into(), String::new());
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

/// Commands on Windows, in a process container, where this Windows has one
/// and the tests run in a desktop session (`windows::sandbox`): elsewhere
/// they check nothing.
#[cfg(windows)]
mod on_windows {
    use super::*;

    fn works() -> bool {
        crate::commands::windows::sandbox().is_ok()
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().map(str::trim_end).collect()
    }

    #[test]
    fn a_command_runs_in_powershell_in_its_folder_and_reports_how_it_ended() {
        if !works() {
            return;
        }
        let fx = fixture(true);
        let status = fx
            .runs
            .start(
                &fx.config,
                &request(
                    "Get-Content README.md; [Console]::Error.WriteLine('oops'); \
                     'made' | Set-Content out.txt; 'é ü 中文'; cmd /c exit 3",
                ),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.state, "exited", "{done:?}");
        assert_eq!(
            done.status.exit_code,
            Some(3),
            "the last command's: {done:?}"
        );
        assert_eq!(lines(&done.stdout), ["hello", "é ü 中文"], "{done:?}");
        assert_eq!(lines(&done.stderr), ["oops"], "{done:?}");
        assert_eq!(
            std::fs::read_to_string(fx.folder.join("out.txt"))
                .unwrap()
                .trim(),
            "made"
        );
    }

    #[test]
    fn a_command_writes_only_its_folder_and_gets_a_clean_environment() {
        if !works() {
            return;
        }
        let fx = fixture(true);
        let outside = fx.folder.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "keep\n").unwrap();
        // SAFETY: tests set this before any run reads the environment.
        unsafe { std::env::set_var("CWW_SECRET_FOR_TEST", "leak") };
        let command = format!(
            "try {{ Get-Content -LiteralPath '{0}' -ErrorAction Stop; 'read' }} \
             catch {{ 'no-read' }}; \
             try {{ Set-Content -LiteralPath '{0}' changed -ErrorAction Stop; 'wrote' }} \
             catch {{ 'no-write' }}; \
             try {{ Get-ChildItem -LiteralPath '{1}' -ErrorAction Stop | Out-Null; 'listed' }} \
             catch {{ 'no-list' }}; \
             \"secret=$env:CWW_SECRET_FOR_TEST\"; \"home=$env:USERPROFILE\"; \
             New-Item -ItemType File \"$env:USERPROFILE\\ok\" | Out-Null; 'home-writable'",
            outside.display(),
            fx.folder.parent().unwrap().display(),
        );
        let status = fx.runs.start(&fx.config, &request(&command)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
        let out = lines(&done.stdout);
        for line in ["no-read", "no-write", "no-list", "secret=", "home-writable"] {
            assert!(out.contains(&line), "{line}: {done:?}");
        }
        let home = out
            .iter()
            .find_map(|l| l.strip_prefix("home="))
            .unwrap()
            .to_string();
        assert!(home.ends_with(&status.id), "its own home: {home}");
        // Removed once the run's processes let go of it, a moment after.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Path::new(&home).exists() {
            assert!(Instant::now() < deadline, "its home goes when it ends");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_command_never_reaches_this_computer_and_the_network_only_where_allowed() {
        if !works() {
            return;
        }
        let mut fx = fixture(true);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dial = format!(
            "$c = New-Object Net.Sockets.TcpClient; \
             try {{ $c.Connect('127.0.0.1', {port}); 'connected' }} catch {{ 'refused' }}; \
             try {{ [Net.Dns]::GetHostAddresses('example.com') | Out-Null; 'resolved' }} \
             catch {{ 'no-dns' }}"
        );
        let status = fx.runs.start(&fx.config, &request(&dial)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(lines(&done.stdout), ["refused", "no-dns"], "{done:?}");
        assert!(listener.accept().is_err(), "nothing reached this computer");

        // With the network, still not this computer's own servers.
        fx.config.roots[0].network = true;
        let status = fx.runs.start(&fx.config, &request(&dial)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(lines(&done.stdout)[0], "refused", "{done:?}");
        assert!(listener.accept().is_err());
    }

    #[test]
    fn a_command_is_stopped_on_request_with_everything_it_started() {
        if !works() {
            return;
        }
        let mut fx = fixture(true);
        let status = fx
            .runs
            .start(
                &fx.config,
                &request(
                    "[Diagnostics.Process]::Start('cmd.exe', '/d /c for /l %i in (0,0,1) do @rem') \
                     | Out-Null; Write-Output started; Start-Sleep -Seconds 60",
                ),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !fx
            .runs
            .output(&status.id, 0, 100)
            .unwrap()
            .text
            .contains("started")
        {
            assert!(Instant::now() < deadline, "it never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        fx.runs.stop(&status.id).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.state, "stopped");
        assert!(done.status.duration_ms < 20_000);

        fx.config.limits.command_timeout_seconds = 3;
        let status = fx
            .runs
            .start(&fx.config, &request("Start-Sleep -Seconds 60"))
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.state, "timed_out");
    }

    #[test]
    fn output_is_kept_in_pieces_and_the_result_keeps_the_end() {
        if !works() {
            return;
        }
        let mut fx = fixture(true);
        fx.config.limits.command_output_chars = 100;
        let status = fx
            .runs
            .start(&fx.config, &request("1..2000 | ForEach-Object { $_ }"))
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.stdout.chars().count(), 100);
        assert!(
            done.stdout.ends_with("1999\r\n2000\r\n"),
            "{:?}",
            done.stdout
        );
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
    fn a_command_runs_into_its_cpu_memory_and_process_limits() {
        if !works() {
            return;
        }
        let mut fx = fixture(true);
        fx.config.limits.command_cpu_seconds = 2;
        let status = fx
            .runs
            .start(&fx.config, &request("while ($true) {}"))
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.limit.as_deref(), Some("cpu_time"), "{done:?}");

        fx.config.limits.command_cpu_seconds = 60;
        fx.config.limits.command_memory_mb = 300;
        let status = fx
            .runs
            .start(
                &fx.config,
                &request(
                    "$ErrorActionPreference = 'Stop'; $b = New-Object byte[] (900MB); 'allocated'",
                ),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert!(!done.stdout.contains("allocated"), "{done:?}");
        assert_eq!(done.status.limit.as_deref(), Some("memory"), "{done:?}");

        fx.config.limits.command_memory_mb = 4096;
        fx.config.limits.command_processes = 4;
        let status = fx
            .runs
            .start(
                &fx.config,
                &request(
                    "$ErrorActionPreference = 'Stop'; 1..6 | ForEach-Object { \
                     [Diagnostics.Process]::Start('cmd.exe', '/d /c for /l %i in (1,1,400000) do @rem') \
                     } | Out-Null",
                ),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.limit.as_deref(), Some("processes"), "{done:?}");
    }
}
