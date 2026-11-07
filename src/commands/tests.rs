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
        read_budget: None,
    });
    // Windows PowerShell can take long to start on a machine that hasn't
    // compiled .NET's assemblies ahead of time yet (a fresh CI runner).
    config.limits.command_timeout_seconds = if cfg!(windows) { 120 } else { 20 };
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
        bot: None,
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
    wait_for(
        runs,
        id,
        Duration::from_secs(if cfg!(windows) { 150 } else { 30 }),
    )
}

fn wait_for(runs: &Runs, id: &str, limit: Duration) -> Finished {
    let deadline = Instant::now() + limit;
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
    // No CPU limit by default: MXC gets none.
    assert!(json["resources"].get("cpuTimeMs").is_none(), "{json}");
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
    assert!(json["resources"].get("cpuTimeMs").is_none(), "{json}");
    let mut limited = plan.clone();
    limited.limits.cpu_seconds = 600;
    let json: serde_json::Value =
        serde_json::from_str(&limited.request_json(None, Sandbox::Seatbelt)).unwrap();
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

/// A compiler in a run finds the system's headers (`/usr/include`), which
/// MXC's Bubblewrap baseline leaves out. Skipped without a sandbox, the
/// headers, or a C compiler.
#[test]
fn a_command_compiles_c_against_the_system_headers() {
    let compiles = std::process::Command::new("cc").arg("--version").output();
    if !sandbox_works()
        || !Path::new("/usr/include/stdio.h").is_file()
        || !compiles.is_ok_and(|o| o.status.success())
    {
        return;
    }
    let fx = fixture(true);
    std::fs::write(
        fx.folder.join("hello.c"),
        "#include <stdio.h>\nint main(void) { puts(\"compiled\"); return 0; }\n",
    )
    .unwrap();
    let status = fx
        .runs
        .start(&fx.config, &shell("cc hello.c -o hello && ./hello"))
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.exit_code, Some(0), "{done:?}");
    assert_eq!(done.stdout, "compiled\n");
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
    let busy = fx.runs.start(&fx.config, &request("true")).unwrap_err();
    assert_eq!(busy.code, "busy");
    assert!(
        busy.message.contains("1 commands are already running"),
        "{busy:?}"
    );
    assert!(busy.message.contains("Wait for one to end"), "{busy:?}");
    // It's not a rate limit, and the model hears so.
    assert_eq!(
        super::tools::busy_code_for_tests(busy),
        crate::error::ErrorCode::Busy
    );
    // 0 is no limit.
    fx.config.limits.commands_running = 0;
    let second = fx.runs.start(&fx.config, &shell("sleep 30")).unwrap();
    fx.runs.stop(&second.id).unwrap();
    fx.config.roots[0].commands = false;
    fx.runs.stop_outside(&fx.config);
    assert_eq!(wait(&fx.runs, &first.id).status.state, "stopped");
    assert_eq!(
        fx.runs.status("ffffffffffff").unwrap_err().code,
        "not_found"
    );
}

/// No secret goes on a command line, where any user of this computer could
/// read it (`ps`): Bubblewrap takes `--setenv NAME VALUE` there, so the
/// folder's values reach the run through a file of its own instead, which
/// it reads and deletes. The request MXC turns into Bubblewrap's arguments
/// holds none of them, and while the run sleeps no process on this
/// computer has one in its arguments.
#[test]
fn no_secret_goes_on_a_command_line() {
    const SECRET: &str = "argvSECRETvalue0123456789";
    let mut fx = fixture(true);
    fx.config.secret_store = Some("file".into());
    let paths = fx.runs.paths.clone();
    crate::commands::env::save(
        &mut fx.config,
        &paths,
        "project",
        &format!("API_TOKEN={SECRET}\n"),
    )
    .unwrap();
    let mut plan = plan(
        &fx.config,
        &fx.runs.paths,
        &request("echo hi"),
        "000000000000",
    )
    .unwrap();
    plan.env.values = vec![(
        "API_TOKEN".into(),
        crate::coding::Secret::new(SECRET.into()),
    )];
    let request_text = plan.request_json(None, Sandbox::Bubblewrap);
    assert!(!request_text.contains(SECRET), "{request_text}");
    let json: serde_json::Value = serde_json::from_str(&request_text).unwrap();
    let command = json["process"]["commandLine"].as_str().unwrap();
    assert!(command.starts_with(". '"), "{command}");
    assert!(command.ends_with("\necho hi"), "{command}");
    // The other sandboxes pass it to the process, not its arguments.
    assert!(plan.request_json(None, Sandbox::Seatbelt).contains(SECRET));
    // The file says each value exactly, quotes and all.
    std::fs::create_dir_all(&plan.home).unwrap();
    plan.env.values.push((
        "QUOTED".into(),
        crate::coding::Secret::new("it's $HOME `x`".into()),
    ));
    // A link a `$( )` left there is never written through.
    #[cfg(unix)]
    let target = {
        let target = fx._dir.path().join("bashrc");
        std::fs::write(&target, "mine\n").unwrap();
        std::os::unix::fs::symlink(&target, plan.values_file()).unwrap();
        target
    };
    plan.write_values(None, Sandbox::Bubblewrap).unwrap();
    #[cfg(unix)]
    {
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine\n");
        assert!(
            !std::fs::symlink_metadata(plan.values_file())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(plan.values_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let read = std::process::Command::new("sh")
            .arg("-c")
            .arg(". \"$1\"; printf '%s|%s' \"$API_TOKEN\" \"$QUOTED\"")
            .arg("sh")
            .arg(plan.values_file())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&read.stdout),
            format!("{SECRET}|it's $HOME `x`")
        );
    }

    if !sandbox_works() || on_landlock() {
        return;
    }
    let line = "echo \"t:$API_TOKEN\"; test -e \"$HOME/.cww-env\" && echo left; sleep 3";
    let status = fx.runs.start(&fx.config, &shell(line)).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let mut seen = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        if let Ok(argv) = std::fs::read(entry.path().join("cmdline"))
            && String::from_utf8_lossy(&argv).contains(SECRET)
        {
            seen.push(entry.file_name());
        }
    }
    let done = wait(&fx.runs, &status.id);
    assert!(
        seen.is_empty(),
        "a process has the secret in its arguments: {seen:?}"
    );
    assert_eq!(done.status.exit_code, Some(0), "{done:?}");
    assert!(done.stdout.contains("t:[redacted]"), "{done:?}");
    assert!(!done.stdout.contains("left"), "the file stayed: {done:?}");
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
        coding: Default::default(),
        env: Default::default(),
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

#[cfg(unix)]
#[test]
fn mise_data_is_private_and_installed_tools_remain_readonly() {
    let fx = fixture(true);
    let host = fx._dir.path().join("host-home");
    let data = host.join(".local/share/mise");
    for directory in ["installs", "plugins", "shims"] {
        std::fs::create_dir_all(data.join(directory)).unwrap();
    }
    std::fs::create_dir_all(host.join(".config/mise")).unwrap();
    let plan = plan(
        &fx.config,
        &fx.runs.paths,
        &request("node -v"),
        "mise-check",
    )
    .unwrap();
    let env: std::collections::HashMap<_, _> = plan.environment(Some(&host)).into_iter().collect();
    assert_eq!(
        env["MISE_DATA_DIR"],
        plan.home.join(".local/share/mise").to_string_lossy()
    );
    assert_eq!(
        env["MISE_SHIMS_DIR"],
        plan.home.join(".local/share/mise/shims").to_string_lossy()
    );
    assert_eq!(
        env["MISE_INSTALLS_DIR"],
        data.join("installs").to_string_lossy()
    );
    assert_eq!(
        env["MISE_INSTALL_STORE_DIR"],
        data.join("installs").to_string_lossy()
    );
    assert_eq!(
        env["MISE_PLUGINS_DIR"],
        data.join("plugins").to_string_lossy()
    );
    let document: serde_json::Value =
        serde_json::from_str(&plan.request_json(Some(&host), Sandbox::Bubblewrap)).unwrap();
    assert!(
        document["filesystem"]["readonlyPaths"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(data.canonicalize().unwrap()))
    );
    assert_eq!(
        document["filesystem"]["readwritePaths"],
        serde_json::json!([plan.folder, plan.home])
    );
}

/// Real installed mise runtimes, with synthetic config only. Run explicitly:
/// `cargo test commands::tests::toolchains -- --ignored`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires installed mise Node/Ruby and a working Linux command sandbox"]
fn toolchains() {
    assert!(
        sandbox_works(),
        "this check requires a working command sandbox"
    );
    let fx = fixture(true);
    let installs = crate::paths::home_dir()
        .unwrap()
        .join(".local/share/mise/installs");
    let installed = |tool: &str| {
        std::fs::read_dir(installs.join(tool))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let version = entry.file_name().into_string().ok()?;
                (version.chars().all(|c| c.is_ascii_digit() || c == '.')
                    && entry.path().join("bin").join(tool).is_file())
                .then_some(version)
            })
            .max_by_key(|v| {
                v.split('.')
                    .map(|n| n.parse::<u32>().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| panic!("install a mise-managed {tool} before this check"))
    };
    let config = fx.folder.join("mise-probe");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "[tools]\nnode = {:?}\nruby = {:?}\n[wrappers]\ncww-probe = \"echo\"\n",
            installed("node"),
            installed("ruby")
        ),
    )
    .unwrap();
    let status = fx
        .runs
        .start(
            &fx.config,
            &request("set -e; export MISE_CONFIG_DIR=\"$PWD/mise-probe\" MISE_SYSTEM_CONFIG_DIR=\"$HOME/empty-mise-system\"; node -p \"'cww-node-check'\"; ruby -e \"puts 'cww-ruby-check'\"; test -f \"$MISE_DATA_DIR/command-wrappers/bin.lock\"; printf 'cww-private-state-check\\n'"),
        )
        .unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.exit_code, Some(0), "{done:?}");
    assert_eq!(
        done.stdout.trim(),
        "cww-node-check\ncww-ruby-check\ncww-private-state-check",
        "{done:?}"
    );
}

/// Commands on Windows, in a process container, where this Windows has one
/// and the tests run in a desktop session (`windows::sandbox`): elsewhere
/// they check nothing.
#[cfg(windows)]
mod on_windows {
    use super::*;

    fn works() -> bool {
        match crate::commands::windows::sandbox() {
            Ok(_) => true,
            Err(e) => {
                eprintln!("not running commands here: {}", e.message);
                false
            }
        }
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
        assert_eq!(entries(&fx.folder), ["README.md", "out.txt", "src"]);
    }

    /// What's in a folder, by name: a run leaves nothing there but what its
    /// command wrote.
    fn entries(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// PowerShell finds its own commands, which live in modules, and says
    /// where it looked when it doesn't.
    #[test]
    fn powershell_finds_its_modules() {
        if !works() {
            return;
        }
        let fx = fixture(true);
        let status = fx
            .runs
            .start(
                &fx.config,
                &request(
                    "[Console]::Out.WriteLine(\"home=$PSHOME\"); \
                     foreach ($p in $env:PSModulePath.Split(';')) { \
                     [Console]::Out.WriteLine(\"modules=$p \" + [IO.Directory]::Exists($p)) }; \
                     (Get-Command Write-Output).Source; (Get-Command Get-ChildItem).Source; \
                     if (-not (Get-Command Get-ChildItem -ErrorAction SilentlyContinue)) { \
                     $listed = @(Get-Module -ListAvailable -ErrorAction SilentlyContinue -ErrorVariable failed); \
                     [Console]::Out.WriteLine(\"listed=$($listed.Count)\"); \
                     foreach ($f in $failed) { [Console]::Out.WriteLine('list ' + $f.ToString()) } }",
                ),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        let out = lines(&done.stdout);
        assert!(out.contains(&"Microsoft.PowerShell.Utility"), "{done:?}");
        assert!(out.contains(&"Microsoft.PowerShell.Management"), "{done:?}");
        assert!(done.stderr.is_empty(), "{done:?}");
        assert_eq!(done.status.exit_code, Some(0), "{done:?}");
        // PowerShell keeps what it learned of its modules in the run's home,
        // not in the folder it runs in.
        assert_eq!(entries(&fx.folder), ["README.md", "src"]);
    }

    /// PowerShell saves what it learned of its modules a while after it
    /// loads one, from a run still going by then: into the run's home, never
    /// into the folder the run works in.
    #[test]
    fn powershell_keeps_its_module_cache_out_of_the_folder() {
        if !works() {
            return;
        }
        let fx = fixture(true);
        let status = fx
            .runs
            .start(
                &fx.config,
                &request("Get-ChildItem | Out-Null; Start-Sleep -Seconds 50"),
            )
            .unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.exit_code, Some(0), "{done:?}");
        assert_eq!(entries(&fx.folder), ["README.md", "src"]);
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
        // Each refusal says why, so a command PowerShell couldn't find
        // doesn't pass for one.
        let command = format!(
            "try {{ Get-Content -LiteralPath '{0}' -ErrorAction Stop; 'read' }} \
             catch {{ 'no-read ' + $_.FullyQualifiedErrorId }}; \
             try {{ Set-Content -LiteralPath '{0}' changed -ErrorAction Stop; 'wrote' }} \
             catch {{ 'no-write ' + $_.FullyQualifiedErrorId }}; \
             try {{ Get-ChildItem -LiteralPath '{1}' -ErrorAction Stop | Out-Null; 'listed' }} \
             catch {{ 'no-list ' + $_.FullyQualifiedErrorId }}; \
             \"secret=$env:CWW_SECRET_FOR_TEST\"; \"home=$env:USERPROFILE\"; \
             New-Item -ItemType File \"$env:USERPROFILE\\ok\" -ErrorAction Stop | Out-Null; \
             'home-writable'",
            outside.display(),
            fx.folder.parent().unwrap().display(),
        );
        let status = fx.runs.start(&fx.config, &request(&command)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
        assert!(done.stderr.is_empty(), "{done:?}");
        let out = lines(&done.stdout);
        for refusal in ["no-read ", "no-write ", "no-list "] {
            let line = out.iter().find(|l| l.starts_with(refusal));
            assert!(
                line.is_some_and(|l| !l.contains("CommandNotFound")),
                "{refusal}: {done:?}"
            );
        }
        for line in ["secret=", "home-writable"] {
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
        // Each failure says whether the socket refused, so a script that
        // broke earlier doesn't pass for a refusal.
        let dial = format!(
            "$c = [Net.Sockets.TcpClient]::new(); \
             try {{ $c.Connect('127.0.0.1', {port}); 'connected' }} \
             catch {{ 'refused ' + ($_.Exception.InnerException -is [Net.Sockets.SocketException]) }}; \
             try {{ [Net.Dns]::GetHostAddresses('example.com') | Out-Null; 'resolved' }} \
             catch {{ 'no-dns ' + ($_.Exception.InnerException -is [Net.Sockets.SocketException]) }}"
        );
        let status = fx.runs.start(&fx.config, &request(&dial)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(
            lines(&done.stdout),
            ["refused True", "no-dns True"],
            "{done:?}"
        );
        assert!(listener.accept().is_err(), "nothing reached this computer");

        // With the network, still not this computer's own servers.
        fx.config.roots[0].network = true;
        let status = fx.runs.start(&fx.config, &request(&dial)).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(lines(&done.stdout)[0], "refused True", "{done:?}");
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
                // The marker is put together so an error quoting the line
                // doesn't print it.
                &request(
                    "[Diagnostics.Process]::Start('cmd.exe', '/d /c for /l %i in (0,0,1) do @rem') \
                     | Out-Null; 'start' + 'ed'; Start-Sleep -Seconds 60",
                ),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let piece = fx.runs.output(&status.id, 0, 4000).unwrap();
            if piece.text.lines().any(|l| l.trim_end() == "started") {
                break;
            }
            assert!(!piece.done, "it ended first: {}", piece.text);
            assert!(Instant::now() < deadline, "it never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        let asked = Instant::now();
        fx.runs.stop(&status.id).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.status.state, "stopped", "{done:?}");
        assert!(asked.elapsed() < Duration::from_secs(10), "{done:?}");

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

/// The person's home in a fixture, signed in to Claude Code as Claude Code
/// leaves it on Linux and Windows, with something else of theirs beside
/// it. Returns the access token.
fn signed_in_person_home(paths: &crate::paths::Paths) -> &'static str {
    const ACCESS: &str = "sk-ant-oat01-testtokenTESTTOKEN0123456789abcdefghijklmnop_-";
    let home = paths.person_home.clone().unwrap();
    let claude = home.join(crate::coding::CONFIG_DIR);
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join(crate::coding::CREDENTIALS_FILE),
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": ACCESS,
                "refreshToken": "sk-ant-ort01-refreshTESTREFRESH0123456789",
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join(crate::coding::CONFIG_FILE),
        "{\"oauthAccount\":{}}",
    )
    .unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh").join("id_ed25519"), "private key\n").unwrap();
    ACCESS
}

/// A bot allowed Claude Code here gets its `claude` first on `PATH` and the
/// computer's Claude Code sign-in where Claude Code keeps it, to read and
/// write, and nothing else of the person's home; what it prints never
/// carries the tokens. A person's run, and another bot's, get none of it.
/// On Linux where Bubblewrap runs, on Windows where process containers do
/// (CI's Windows on Arm), and on macOS with `CWW_TEST_SEATBELT=1` (CI's
/// Macs don't run commands).
#[test]
fn only_a_bot_allowed_it_gets_claude_codes_sign_in_and_its_tokens_never_print() {
    let runs_here = if cfg!(windows) {
        Sandbox::for_host().is_ok()
    } else if cfg!(target_os = "macos") {
        std::env::var("CWW_TEST_SEATBELT").as_deref() == Ok("1")
    } else {
        sandbox_works() && !on_landlock()
    };
    if !runs_here {
        return;
    }
    let mut fx = fixture(true);
    let paths = fx.runs.paths.clone();
    let access = signed_in_person_home(&paths);
    let home = paths.person_home.clone().unwrap();
    let install = fx._dir.path().join("claude-install");
    std::fs::create_dir_all(&install).unwrap();
    // A stand-in `claude` that prints its sign-in, as a careless command
    // might, and keeps a file of its own beside it, as Claude Code does
    // when it refreshes its tokens.
    let program = if cfg!(windows) {
        let program = install.join("claude.cmd");
        std::fs::write(
            &program,
            "@echo claude here\r\n\
             @type \"%CLAUDE_CONFIG_DIR%\\.credentials.json\"\r\n\
             @type \"%CLAUDE_CONFIG_DIR%\\.credentials.json\" 1>&2\r\n\
             @echo.\r\n\
             @echo refreshed> \"%CLAUDE_CONFIG_DIR%\\refreshed\"\r\n",
        )
        .unwrap();
        program
    } else {
        let program = install.join("claude");
        std::fs::write(
            &program,
            "#!/bin/sh\necho claude here\ncat \"$HOME/.claude/.credentials.json\"\necho\n\
             cat \"$HOME/.claude/.credentials.json\" >&2\necho >&2\n\
             cat \"$HOME/.claude.json\"\necho\necho refreshed > \"$HOME/.claude/refreshed\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        program
    };
    fx.config.roots[0].network = true;
    fx.config.roots[0].bots.push(crate::config::BotLoan {
        id: "42".into(),
        name: String::new(),
        unattended: false,
        claude_code: true,
        codex: false,
        owners: Vec::new(),
    });
    fx.config.coding.claude = Some(program);
    // Claude Code needs the network; where the sandbox can't give one (CI's
    // Ubuntu refuses network namespaces), there's nothing to check.
    let probe = fx.runs.start(&fx.config, &request("exit 0")).unwrap();
    if wait(&fx.runs, &probe.id).status.state == "failed" {
        eprintln!("no network for commands here");
        return;
    }

    let real = crate::roots::canonical(&home).unwrap();
    let ssh = real.join(".ssh").join("id_ed25519");
    let credentials = real
        .join(crate::coding::CONFIG_DIR)
        .join(crate::coding::CREDENTIALS_FILE);
    let reads = |path: &Path| {
        if cfg!(windows) {
            format!(
                "if (Test-Path -LiteralPath '{0}') {{ \"read:$(Get-Content -Raw -LiteralPath '{0}')\" }} else {{ 'hidden' }}",
                path.display()
            )
        } else {
            format!(
                "if cat '{}' >/dev/null 2>&1; then echo read; else echo hidden; fi",
                path.display()
            )
        }
    };
    let line = format!("claude; {}", reads(&ssh));
    let bot = Request {
        bot: Some("42".into()),
        ..shell(&line)
    };
    let status = fx.runs.start(&fx.config, &bot).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.exit_code, Some(0), "{done:?}");
    assert!(done.stdout.contains("claude here"), "{done:?}");
    // The sign-in was read, with its tokens taken out.
    assert!(done.stdout.contains("claudeAiOauth"), "{done:?}");
    assert!(done.stdout.contains("[claude-code-token]"), "{done:?}");
    assert!(done.stderr.contains("[claude-code-token]"), "{done:?}");
    if !cfg!(windows) {
        assert!(
            done.stdout.contains("oauthAccount"),
            "~/.claude.json: {done:?}"
        );
    }
    // Nothing else of the home folder.
    assert!(
        done.stdout.trim_end().ends_with("hidden"),
        "~/.ssh: {done:?}"
    );
    // What it wrote there is in the person's own folder.
    assert!(
        real.join(crate::coding::CONFIG_DIR)
            .join("refreshed")
            .is_file(),
        "{done:?}"
    );
    let window = fx.runs.output(&status.id, 0, MAX_OUTPUT_CHUNK).unwrap();
    for text in [&done.stdout, &done.stderr, &window.text] {
        assert!(
            !text.contains("testtokenTESTTOKEN"),
            "the access token printed"
        );
        assert!(
            !text.contains("refreshTESTREFRESH"),
            "the refresh token printed"
        );
    }
    assert!(!done.stdout.contains(access));

    // A person's run, and another bot's, see none of it.
    for request in [
        shell(&reads(&credentials)),
        Request {
            bot: Some("7".into()),
            ..shell(&reads(&credentials))
        },
    ] {
        let status = fx.runs.start(&fx.config, &request).unwrap();
        let done = wait(&fx.runs, &status.id);
        assert_eq!(done.stdout.trim(), "hidden", "{request:?}: {done:?}");
    }
}

/// A folder's environment reaches every command there: text as it is, and
/// `$( )` worked out in the run's own sandbox, with the values before it.
/// Neither ever prints, and a `$( )` that fails stops the run, naming its
/// variable. Where Bubblewrap or Landlock runs.
#[test]
fn a_folders_environment_reaches_its_commands_and_never_prints() {
    if !sandbox_works() {
        return;
    }
    let mut fx = fixture(true);
    fx.config.secret_store = Some("file".into());
    let paths = fx.runs.paths.clone();
    crate::commands::env::save(
        &mut fx.config,
        &paths,
        "project",
        "OP_SERVICE_ACCOUNT_TOKEN=ops_literalTOKEN0123456789\n\
         DB_PASSWORD=\"pw-$(printf %s-v2 \"$OP_SERVICE_ACCOUNT_TOKEN\")\"\n",
    )
    .unwrap();
    // Only the shell's own commands: where the package is built (Nix's
    // sandbox) the sandbox has no `tr` or `cut`.
    let line = "echo \"t:$OP_SERVICE_ACCOUNT_TOKEN\"; echo \"p:$DB_PASSWORD\" >&2; \
                piece=\"${DB_PASSWORD%%TOKEN*}\"; echo \"piece:${piece#pw-}\"; \
                echo \"tail:${DB_PASSWORD##*TOKEN0123456789}\"";
    let status = fx.runs.start(&fx.config, &shell(line)).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.exit_code, Some(0), "{done:?}");
    assert!(done.stdout.contains("t:[redacted]"), "{done:?}");
    assert!(done.stderr.contains("p:[redacted]"), "{done:?}");
    // Only the whole text is taken out: a piece of it still shows, which is
    // how a test can see the value was worked out.
    assert!(done.stdout.contains("piece:ops_literal"), "{done:?}");
    assert!(done.stdout.contains("tail:-v2"), "{done:?}");
    let window = fx.runs.output(&status.id, 0, MAX_OUTPUT_CHUNK).unwrap();
    for text in [&done.stdout, &done.stderr, &window.text] {
        assert!(
            !text.contains("literalTOKEN0123456789"),
            "the value printed"
        );
        assert!(
            !text.contains("TOKEN0123456789-v2"),
            "the worked-out value printed"
        );
    }

    crate::commands::env::save(
        &mut fx.config,
        &paths,
        "project",
        "KEY=secretKEYvalue42\nBAD=$(echo \"nope $KEY\" >&2; exit 3)\n",
    )
    .unwrap();
    let status = fx.runs.start(&fx.config, &shell("echo ran")).unwrap();
    let done = wait(&fx.runs, &status.id);
    assert_eq!(done.status.state, "failed", "{done:?}");
    assert_eq!(
        done.status.error.as_deref(),
        Some("BAD's $( ) exited with 3"),
        "{done:?}"
    );
    assert!(!done.stdout.contains("ran"), "{done:?}");
    assert!(done.stderr.contains("nope [redacted]"), "{done:?}");
    assert!(!done.stderr.contains("secretKEYvalue42"), "{done:?}");
}

/// A bot allowed Codex gets its `codex` first on `PATH` and `~/.codex`, to read
/// and write, with the tokens in `auth.json` out of what it prints. Claude
/// Code's sign-in, which it wasn't allowed, isn't there.
#[cfg(unix)]
#[test]
fn only_a_bot_allowed_it_gets_codexs_sign_in_and_its_tokens_never_print() {
    const ACCESS: &str = "eyJhbGciOiJSUzI1NiJ9.codex-access-token-value";
    // Codex can't run in Seatbelt yet (`Agent::unavailable`): Linux only.
    if cfg!(target_os = "macos") || !(sandbox_works() && !on_landlock()) {
        return;
    }
    let mut fx = fixture(true);
    let paths = fx.runs.paths.clone();
    signed_in_person_home(&paths);
    let home = paths.person_home.clone().unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/auth.json"),
        serde_json::json!({ "tokens": { "access_token": ACCESS } }).to_string(),
    )
    .unwrap();
    let install = fx._dir.path().join("codex-install");
    std::fs::create_dir_all(&install).unwrap();
    let program = install.join("codex");
    std::fs::write(
        &program,
        "#!/bin/sh\necho codex here\ncat \"$HOME/.codex/auth.json\"\necho\n\
         echo refreshed > \"$HOME/.codex/refreshed\"\nls -a \"$HOME\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.config.roots[0].bots.push(crate::config::BotLoan {
        id: "42".into(),
        name: String::new(),
        unattended: false,
        claude_code: false,
        codex: true,
        owners: Vec::new(),
    });
    fx.config.coding.codex = Some(program);
    fx.config.roots[0].network = true;
    let probe = fx.runs.start(&fx.config, &request("exit 0")).unwrap();
    if wait(&fx.runs, &probe.id).status.state == "failed" {
        eprintln!("no network for commands here");
        return;
    }
    let as_bot = Request {
        bot: Some("42".into()),
        ..request("codex")
    };
    let started = fx.runs.start(&fx.config, &as_bot).unwrap();
    let done = wait(&fx.runs, &started.id);
    assert!(done.stdout.contains("codex here"), "{done:?}");
    assert!(done.stdout.contains("[codex-token]"), "{done:?}");
    assert!(!done.stdout.contains(ACCESS), "{done:?}");
    assert!(
        !done.stdout.contains(".claude"),
        "not Claude Code's: {done:?}"
    );
    assert!(
        home.join(".codex/refreshed").is_file(),
        "it can write there"
    );

    let as_person = request("cat \"$HOME/.codex/auth.json\"; echo end");
    let started = fx.runs.start(&fx.config, &as_person).unwrap();
    let done = wait(&fx.runs, &started.id);
    assert!(!done.stdout.contains(ACCESS), "{done:?}");
}

/// A bare repository with these files, as GitHub would hold it.
fn bare_repository(base: &Path, files: &[(&str, &str)]) -> std::path::PathBuf {
    let work = base.join("origin-work");
    std::fs::create_dir_all(&work).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let done = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(done.status.success(), "{args:?}: {done:?}");
    };
    git(&work, &["init", "-q", "-b", "main"]);
    for (name, text) in files {
        std::fs::write(work.join(name), text).unwrap();
    }
    git(&work, &["add", "."]);
    git(
        &work,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "first"],
    );
    let bare = base.join("origin.git");
    git(
        base,
        &["clone", "-q", "--bare", "origin-work", "origin.git"],
    );
    bare
}

fn prepared(fx: &Fixture, ask: &crate::coding::prepare::Ask) -> crate::coding::prepare::View {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let view = fx.runs.preparations.status(ask).expect("started");
        if view.state != "running" || std::time::Instant::now() > deadline {
            return view;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A bot's first run on a repository prepares it by itself: a mirror only
/// Desktop writes, a checkout with no credentials that can't push, its tests
/// run once in the sandbox, with no agent's sign-in. Then it's prepared; a
/// failing step says which, and running again carries on.
#[cfg(unix)]
#[test]
fn the_first_run_prepares_a_repository_by_itself_and_names_a_step_that_fails() {
    let runs_here = if cfg!(target_os = "macos") {
        std::env::var("CWW_TEST_SEATBELT").as_deref() == Ok("1")
    } else {
        sandbox_works() && !on_landlock()
    };
    if !runs_here {
        return;
    }
    let mut fx = fixture(true);
    let paths = fx.runs.paths.clone();
    signed_in_person_home(&paths);
    let program = fx._dir.path().join("claude");
    std::fs::write(&program, "").unwrap();
    fx.config.coding.claude = Some(program);
    fx.config.roots[0].network = true;
    let probe = fx.runs.start(&fx.config, &request("exit 0")).unwrap();
    if wait(&fx.runs, &probe.id).status.state == "failed" {
        eprintln!("no network for commands here");
        return;
    }
    fx.config.roots[0].bots.push(crate::config::BotLoan {
        id: "42".into(),
        name: String::new(),
        unattended: false,
        claude_code: true,
        codex: false,
        owners: vec!["acme".into()],
    });
    let bare = bare_repository(
        fx._dir.path(),
        &[(
            "Makefile",
            "test:\n\t@echo \"$$HOME\" > .ran; test ! -e \"$$HOME/.claude\"; test ! -e failing\n",
        )],
    );
    fx.config
        .coding
        .repositories
        .push(crate::config::Repository {
            root: "project".into(),
            name: "app".into(),
            url: format!("file://{}", bare.display()),
            ssh_key: None,
            github: Some("acme/app".into()),
        });
    fx.config.save(&paths).unwrap();
    let ask = crate::coding::prepare::Ask {
        bot: "42".into(),
        repository: "acme/app".into(),
        agent: None,
        tests: true,
    };

    let started = fx
        .runs
        .preparations
        .start(&fx.runs, &fx.config, &paths, &ask)
        .unwrap();
    assert_eq!(started.state, "running");
    let view = prepared(&fx, &ask);
    assert_eq!(view.state, "ready", "{view:?}");
    assert_eq!(view.checkout.as_deref(), Some("app"));
    assert_eq!(view.test_command.as_deref(), Some("make test"));
    assert_eq!(view.tests.as_deref(), Some("passed"));
    assert!(
        view.steps.iter().all(|step| step.state == "done"),
        "{view:?}"
    );

    let checkout = fx.folder.join("app");
    let mirror = crate::coding::repositories::mirror_dir(&paths, &fx.config.coding.repositories[0]);
    let origin = std::process::Command::new("git")
        .args([
            "-C",
            checkout.to_str().unwrap(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&origin.stdout).trim(),
        mirror.to_str().unwrap(),
        "its origin is the mirror"
    );
    assert!(
        !std::fs::read_to_string(checkout.join(".git/config"))
            .unwrap()
            .contains(&bare.to_string_lossy().to_string())
    );
    assert!(checkout.join(".ran").is_file(), "its tests ran");

    let again = fx
        .runs
        .preparations
        .start(&fx.runs, &fx.config, &paths, &ask)
        .unwrap();
    assert_eq!(again.state, "ready", "once for each repository");
    assert!(again.steps.is_empty(), "{again:?}");

    let push = Request {
        bot: Some("42".into()),
        dir: "app".into(),
        ..request(
            "git -c commit.gpgsign=false -c user.name=x -c user.email=x@example.com commit -q --allow-empty -m x && git push origin HEAD:refs/heads/main",
        )
    };
    let started = fx.runs.start(&fx.config, &push).unwrap();
    assert_ne!(
        wait(&fx.runs, &started.id).status.exit_code,
        Some(0),
        "a push fails"
    );

    // A failing step says which, and asks nothing of the mirror.
    std::fs::remove_file(mirror.join("cww-prepared.json")).unwrap();
    std::fs::write(checkout.join("failing"), "").unwrap();
    fx.runs
        .preparations
        .start(&fx.runs, &fx.config, &paths, &ask)
        .unwrap();
    let view = prepared(&fx, &ask);
    assert_eq!(view.state, "failed", "{view:?}");
    assert_eq!(view.failed_step.as_deref(), Some("Running its tests once"));
    assert!(view.detail.is_some());

    // Going on without them, as the person chose, doesn't run them.
    let without = crate::coding::prepare::Ask {
        tests: false,
        ..ask.clone()
    };
    fx.runs
        .preparations
        .start(&fx.runs, &fx.config, &paths, &without)
        .unwrap();
    let view = prepared(&fx, &without);
    assert_eq!(view.state, "ready", "{view:?}");
    assert_eq!(view.tests.as_deref(), Some("skipped"));
}

/// The real Claude Code, found as `cww coding setup` finds it, answers from
/// the sandbox with this computer's own Claude Code sign-in, as a bot's run
/// gets it. Needs Claude Code signed in here and the network; spends a few
/// tokens of that subscription. Its failures never print what the run
/// printed if a token leaked.
#[test]
#[ignore = "needs Claude Code signed in on this computer, and the network"]
fn real_claude_code_answers_from_the_sandbox() {
    let program = crate::coding::setup::find_claude().expect("Claude Code installed");
    let mut fx = fixture(true);
    let mut paths = fx.runs.paths.clone();
    paths.person_home = crate::paths::home_dir().ok();
    let login = crate::coding::Login::find(paths.person_home.as_deref().unwrap());
    assert!(login.signed_in(), "run `claude` once and sign in");
    fx.runs = Runs::new(paths);
    fx.config.limits.command_timeout_seconds = 180;
    fx.config.roots[0].network = true;
    fx.config.roots[0].bots.push(crate::config::BotLoan {
        id: "42".into(),
        name: String::new(),
        unattended: false,
        claude_code: true,
        codex: false,
        owners: Vec::new(),
    });
    fx.config.coding.claude = Some(program);
    let ask = Request {
        bot: Some("42".into()),
        ..request("claude -p 'Reply with the word ready.' --max-turns 1")
    };
    let status = fx.runs.start(&fx.config, &ask).unwrap();
    let done = wait_for(&fx.runs, &status.id, Duration::from_secs(190));
    let window = fx.runs.output(&status.id, 0, MAX_OUTPUT_CHUNK).unwrap();
    let leaked = [&done.stdout, &done.stderr, &window.text]
        .iter()
        .any(|text| login.secrets.iter().any(|s| text.contains(s.expose())));
    assert!(!leaked, "a token printed");
    eprintln!(
        "state {} exit {:?}\nstdout: {}\nstderr: {}",
        done.status.state, done.status.exit_code, done.stdout, done.stderr
    );
    assert_eq!(done.status.exit_code, Some(0));
    assert!(done.stdout.to_lowercase().contains("ready"));
}

/// A bot's run allowed Claude Code reads and writes exactly Claude Code's
/// sign-in beyond its folder and home, and on macOS reaches the keychain
/// only when Claude Code keeps it there; a run without it gets neither.
#[test]
fn only_claude_codes_sign_in_is_added_to_what_a_run_writes() {
    let fx = fixture(true);
    let mut plan = plan(
        &fx.config,
        &fx.runs.paths,
        &request("claude"),
        "000000000000",
    )
    .unwrap();
    let writable = |plan: &Plan, sandbox| -> (Vec<String>, serde_json::Value) {
        let json: serde_json::Value =
            serde_json::from_str(&plan.request_json(None, sandbox)).unwrap();
        let paths = json["filesystem"]["readwritePaths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        (paths, json["seatbelt"]["keychainAccess"].clone())
    };
    let own = vec![
        plan.folder.to_string_lossy().into_owned(),
        plan.home.to_string_lossy().into_owned(),
    ];
    assert_eq!(writable(&plan, Sandbox::Bubblewrap).0, own);
    assert_eq!(
        writable(&plan, Sandbox::Seatbelt).1,
        serde_json::Value::Bool(false)
    );

    let dir = Path::new("/home/u/.claude").to_path_buf();
    let file = Path::new("/home/u/.claude.json").to_path_buf();
    plan.coding.agents = vec![crate::coding::AgentRun {
        agent: crate::coding::Agent::ClaudeCode,
        program: "/x/claude".into(),
        real: "/x/claude".into(),
        read: "/x".into(),
        entry: None,
        login: crate::coding::Login {
            dir: Some(dir.clone()),
            file: Some(file.clone()),
            keychain: false,
            secrets: Vec::new(),
            billing: Default::default(),
        },
    }];
    let mut expected = own.clone();
    expected.extend([
        "/home/u/.claude".to_string(),
        "/home/u/.claude.json".to_string(),
    ]);
    assert_eq!(writable(&plan, Sandbox::Bubblewrap).0, expected);
    assert_eq!(writable(&plan, Sandbox::Landlock).0, expected);
    let (seatbelt, keychain) = writable(&plan, Sandbox::Seatbelt);
    assert_eq!(seatbelt, expected);
    assert_eq!(keychain, serde_json::Value::Bool(false));
    if let Some(claude) = plan.coding.agents.first_mut() {
        claude.login.keychain = true;
    }
    assert_eq!(
        writable(&plan, Sandbox::Seatbelt).1,
        serde_json::Value::Bool(true)
    );
}

/// A run reads Claude Code's folder where it really is: Bubblewrap can't
/// start with a link given as a folder to bind (mise's
/// `installs/claude/latest`), which stopped every run of a bot allowed
/// Claude Code on a NUC. And never a folder that holds a shared one: then
/// only the program.
#[cfg(unix)]
#[test]
fn claude_codes_folder_is_read_where_it_really_is_and_never_too_broad() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("2.1.316");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("claude"), "").unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("latest")).unwrap();
    let program = dir.path().join("latest").join("claude");
    let fx = fixture(true);
    let real_program = program.canonicalize().unwrap();
    let read = crate::coding::grant(&real_program, &fx.config, &fx.runs.paths).unwrap();
    assert_eq!(read, real.canonicalize().unwrap());
    let mut plan = plan(
        &fx.config,
        &fx.runs.paths,
        &request("claude"),
        "000000000000",
    )
    .unwrap();
    plan.coding.agents = vec![crate::coding::AgentRun {
        agent: crate::coding::Agent::ClaudeCode,
        real: real_program,
        program,
        read,
        entry: None,
        login: Default::default(),
    }];
    let real = real.canonicalize().unwrap().to_string_lossy().into_owned();
    assert_eq!(plan.coding_readable(), [real]);

    // Beside the shared folder: its parent would hold the folder.
    let beside = fx.folder.parent().unwrap().join("claude");
    std::fs::write(&beside, "").unwrap();
    let beside = beside.canonicalize().unwrap();
    assert_eq!(
        crate::coding::grant(&beside, &fx.config, &fx.runs.paths),
        Some(beside.clone()),
        "only the program"
    );
}
