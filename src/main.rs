use std::path::PathBuf;

// mimalloc returns freed memory to the system; see Cargo.toml.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;

use cww::config::{Config, DEFAULT_SERVER};
use cww::control::{self, ControlRequest};
use cww::paths::Paths;
use cww::roots::{NewRoot, add_root, remove_root, set_writable};
use cww::{audit, auth, crash_reports, daemon, service};

/// Chat with Work Local Agent.
///
/// Lets Chat with Work read, and with your approval change files and run
/// commands, in folders you share. Without a command, opens the terminal UI.
#[derive(Parser)]
#[command(name = "cww", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Pair this computer with Chat with Work (OAuth device flow).
    Login {
        /// Server origin, for self-hosted installs.
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        /// Name shown in Settings (defaults to the hostname).
        #[arg(long)]
        name: Option<String>,
        /// Print the approval link without opening a browser.
        #[arg(long)]
        no_browser: bool,
        /// Don't start the background daemon after pairing.
        #[arg(long)]
        no_daemon: bool,
        /// Only share folders: don't ask to use your chats from `cww tui`.
        #[arg(long)]
        no_chats: bool,
        /// Sign in to another organization for chats, without changing shared folders.
        #[arg(long, conflicts_with = "no_chats")]
        organization: bool,
        /// Print JSON lines for a program to read (the desktop app): the
        /// code, then the outcome. Never asks anything.
        #[arg(long)]
        json: bool,
    },
    /// Forget the pairing on this computer. Revoke it in Settings too.
    Logout {
        /// Forget only this saved organization sign-in (its ID from login --json).
        #[arg(long, allow_hyphen_values = true)]
        organization: Option<String>,
    },
    /// Manage the shared folders.
    Roots {
        #[command(subcommand)]
        command: RootsCommand,
    },
    /// Show the daemon's connection and index state.
    Status {
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop answering tool calls until `cww resume`.
    Pause,
    /// Answer tool calls again.
    Resume,
    /// Make the daemon re-read the config file after you edit it.
    Reload,
    /// Show the local audit log.
    Log {
        /// Keep printing new entries.
        #[arg(short, long)]
        follow: bool,
        /// Entries to show first.
        #[arg(short = 'n', long, default_value_t = 20)]
        lines: usize,
        /// Print raw JSON lines.
        #[arg(long)]
        json: bool,
    },
    /// Run or register the background daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Diagnostics.
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
    /// Open the terminal UI (the default when run in a terminal).
    Tui {
        /// Saved organization name or ID from `cww organizations`.
        #[arg(long, allow_hyphen_values = true)]
        organization: Option<String>,
    },
    /// List organization sign-ins saved on this computer.
    Organizations {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RootsCommand {
    /// Share a folder.
    Add {
        path: PathBuf,
        /// Label shown to Chat with Work (defaults to the folder name).
        #[arg(long)]
        label: Option<String>,
        /// Allow `/`, your home folder, or system directories.
        #[arg(long)]
        i_know: bool,
        /// Follow symlinks that stay inside the folder.
        #[arg(long)]
        follow_symlinks: bool,
        /// Allow changes in it from the start (see `allow-changes`).
        #[arg(long)]
        allow_changes: bool,
    },
    /// List shared folders.
    List,
    /// Stop sharing a folder (by ID, label or path).
    Remove { root: String },
    /// Let Chat with Work change files in a shared folder: create, edit,
    /// move and delete, each approved in Chat with Work, with old versions
    /// and deleted files kept in the system trash.
    AllowChanges {
        root: String,
        /// Allow changes even in your home folder, a folder that holds it,
        /// a whole drive, or a system directory.
        #[arg(long)]
        i_know: bool,
    },
    /// Make a shared folder read-only again.
    DenyChanges { root: String },
    /// Let Chat with Work run commands in a shared folder that allows
    /// changes (`bin/rails test`, `npm run build`), each approved in Chat
    /// with Work and run in a sandbox that can change only that folder.
    AllowCommands {
        root: String,
        /// Let those commands use the network (installing packages).
        #[arg(long)]
        network: bool,
    },
    /// Stop running commands in a shared folder.
    DenyCommands { root: String },
}

#[derive(Subcommand)]
enum DebugCommand {
    /// Confine this process as the daemon would, then try to read FILES,
    /// and to write each `--write` file directly (bypassing cww's checks).
    Sandbox {
        files: Vec<PathBuf>,
        #[arg(long = "write")]
        writes: Vec<PathBuf>,
    },
    /// Confine this process as the daemon would, then make one change with
    /// a change tool, as the server would ask: TOOL and its JSON arguments.
    Change { tool: String, arguments: String },
    /// Confine this process for a keychain-held key, then write, read and
    /// delete a test secret in the OS keychain.
    Keychain,
    /// Panic on purpose, to check crash reports end to end. Sends one only
    /// where they're turned on (`CWW_CRASH_REPORTS=1`).
    Panic,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Run in the foreground.
    Run {
        /// Write the daemon's own log to this file instead of stderr.
        #[arg(long)]
        log_file: Option<PathBuf>,
        /// Don't confine the daemon with Landlock or Seatbelt.
        #[arg(long)]
        no_sandbox: bool,
        /// Run as the confined worker of a supervising `cww daemon run`.
        #[arg(long, hide = true)]
        worker: bool,
    },
    /// Ask the running daemon to stop.
    Stop,
    /// Install and start the per-user service (systemd --user or LaunchAgent).
    Install {
        /// Leave out systemd sandboxing options.
        #[arg(long)]
        no_hardening: bool,
    },
    /// Stop and remove the service.
    Uninstall {
        /// Also delete keys, config, index and logs.
        #[arg(long)]
        purge: bool,
    },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::from_env()?;
    // The daemon installs its own, as `daemon`, where `cww-agent` does too.
    let component = match &cli.command {
        None | Some(Command::Tui { .. }) => Some(crash_reports::Component::Tui),
        Some(Command::Daemon {
            command: DaemonCommand::Run { .. },
        }) => None,
        Some(_) => Some(crash_reports::Component::Cli),
    };
    if let Some(component) = component {
        crash_reports::install(component, &paths);
    }
    let Some(command) = cli.command else {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
            return cww::tui::run(paths);
        }
        // Piped into `head`, a broken pipe isn't worth an error.
        let _ = Cli::command().print_help();
        return Ok(());
    };
    match command {
        Command::Tui { organization } => {
            cww::tui::run_organization(paths, organization.as_deref())?
        }
        Command::Organizations { json } => {
            let profiles = auth::organizations::list(&paths)?;
            if json {
                println!("{}", serde_json::to_string(&profiles)?);
            } else if profiles.is_empty() {
                println!("No additional organizations. Sign in with cww login --organization.");
            } else {
                for profile in profiles {
                    println!(
                        "{}  {}  {}",
                        profile.id, profile.account.name, profile.server.url
                    );
                }
            }
        }
        Command::Login {
            server,
            name,
            no_browser,
            no_chats,
            no_daemon,
            json: true,
            organization,
        } => login_json(
            &paths,
            &server,
            name,
            !no_browser,
            no_chats,
            !no_daemon,
            organization,
        )?,
        Command::Login {
            server,
            name,
            no_browser,
            no_daemon,
            no_chats,
            json: false,
            organization,
        } => {
            let options = auth::LoginOptions {
                name,
                store: None,
                open_browser: !no_browser,
                without_chats: no_chats,
            };
            if organization {
                let profile =
                    auth::login_organization(&paths, &server, options, |line| println!("{line}"))?;
                println!(
                    "\nSigned in to {}. Shared folders are unchanged.",
                    profile.account.name
                );
            } else {
                let paired = auth::login(&paths, &server, options, |line| println!("{line}"))?;
                println!(
                    "\nPaired with {} as device {}.",
                    paired.url, paired.device_id
                );
            }
            if !organization && Config::load(&paths)?.roots.is_empty() {
                offer_documents(&paths)?;
            }
            proxy_hint(&paths);
            match daemon_after_login(&paths, !no_daemon) {
                AfterLogin::Reloaded => println!("The daemon picked up the pairing."),
                AfterLogin::Started => {
                    println!("Started the daemon. It runs in the background and at every login.");
                }
                AfterLogin::NotRunning => println!("{START_DAEMON_HINT}"),
                AfterLogin::Failed(e) if no_daemon => eprintln!("warning: {e}"),
                AfterLogin::Failed(e) => eprintln!("warning: {e}\n{START_DAEMON_HINT}"),
            }
        }
        Command::Logout {
            organization: Some(id),
        } => {
            if auth::organizations::forget(&paths, &id)? {
                println!(
                    "Signed out of this organization on this computer. Revoke the sign-in in Settings as well."
                );
            } else {
                anyhow::bail!("That organization is not signed in on this computer.");
            }
        }
        Command::Logout { organization: None } => {
            if auth::logout(&paths)? {
                println!("Forgot the pairing. Revoke this computer in Settings as well.");
            } else {
                println!("This computer wasn't paired.");
            }
            notify_daemon(&paths, "");
        }
        Command::Roots { command } => roots(&paths, command)?,
        Command::Status { json } => status(&paths, json)?,
        Command::Pause => pause(&paths, true)?,
        Command::Resume => pause(&paths, false)?,
        Command::Reload => match control::request(&paths.socket_path(), ControlRequest::Reload)? {
            Some(_) => println!("The daemon reloaded {}.", paths.config_file().display()),
            None => println!("The daemon is not running; it reads the config when it starts."),
        },
        Command::Log {
            follow,
            lines,
            json,
        } => log(&paths, lines, follow, json)?,
        Command::Debug {
            command: DebugCommand::Panic,
        } => panic!("cww debug panic: a test of crash reports"),
        Command::Debug {
            command: DebugCommand::Keychain,
        } => {
            let status = daemon::confine_for_check(&paths, Some(true))?;
            println!("sandbox: {}", serde_json::to_string(&status)?);
            let store = auth::secrets::SecretStore::Keyring;
            let name = "sandbox-probe";
            let result = store
                .set(name, "ok")
                .and_then(|()| store.get(name))
                .and_then(|value| store.delete(name).map(|()| value));
            match result {
                Ok(Some(value)) if value == "ok" => {
                    println!("keychain: write, read and delete work")
                }
                Ok(other) => println!("keychain: read back {other:?}"),
                Err(e) => println!("keychain: {e:#}"),
            }
        }
        Command::Debug {
            command: DebugCommand::Sandbox { files, writes },
        } => {
            let status = daemon::confine_for_check(&paths, None)?;
            println!("sandbox: {}", serde_json::to_string(&status)?);
            for file in files {
                match std::fs::read(&file) {
                    Ok(_) => println!("read {}", file.display()),
                    Err(e) => println!("refused {} ({e})", file.display()),
                }
            }
            for file in writes {
                match std::fs::write(&file, "probe") {
                    Ok(()) => println!("wrote {}", file.display()),
                    Err(e) => println!("refused writing {} ({e})", file.display()),
                }
            }
        }
        Command::Debug {
            command: DebugCommand::Change { tool, arguments },
        } => {
            // To stderr, so a refusal's reason (such as why the trash is
            // unavailable) shows next to it.
            cww::logging::init(None)?;
            let status = daemon::confine_for_check(&paths, None)?;
            println!("sandbox: {}", serde_json::to_string(&status)?);
            let arguments: Value = serde_json::from_str(&arguments)
                .map_err(|e| anyhow::anyhow!("parsing ARGUMENTS: {e}"))?;
            match daemon::change_for_check(&paths, &tool, arguments)? {
                Ok(result) => println!("changed {}", serde_json::to_string(&result)?),
                Err(e) => println!("refused {}: {}", e.code.as_str(), e.message),
            }
        }
        Command::Daemon { command } => match command {
            DaemonCommand::Run {
                log_file,
                no_sandbox,
                worker,
            } => daemon::run_foreground(
                paths,
                &daemon::Foreground {
                    log_file,
                    sandbox: !no_sandbox,
                    worker,
                },
            )?,
            DaemonCommand::Stop => {
                match control::request(&paths.socket_path(), ControlRequest::Shutdown)? {
                    Some(_) => println!("The daemon is stopping."),
                    None => println!("The daemon is not running."),
                }
            }
            DaemonCommand::Install { no_hardening } => {
                let file = service::install(&paths, &service::InstallOptions { no_hardening })?;
                println!("Installed and started {}.", file.display());
                proxy_hint(&paths);
            }
            DaemonCommand::Uninstall { purge } => {
                for path in service::uninstall(&paths, purge)? {
                    println!("Removed {}", path.display());
                }
                if !purge {
                    println!("Keys, config, index and logs were kept; use --purge to delete them.");
                }
            }
        },
    }
    Ok(())
}

fn roots(paths: &Paths, command: RootsCommand) -> Result<()> {
    let mut config = Config::load(paths)?;
    match command {
        RootsCommand::Add {
            path,
            label,
            i_know,
            follow_symlinks,
            allow_changes,
        } => {
            let root = add_root(
                &mut config,
                paths,
                NewRoot {
                    path: &path,
                    label,
                    i_know,
                    follow_symlinks,
                    writable: allow_changes,
                },
            )?;
            config.save(paths)?;
            println!(
                "Sharing {} as {} ({}){}.",
                root.path.display(),
                root.id,
                root.label,
                if root.writable {
                    ", with changes allowed"
                } else {
                    ""
                }
            );
            notify_daemon(paths, "");
        }
        RootsCommand::List => {
            if config.roots.is_empty() {
                println!("Nothing is shared. Add a folder with `cww roots add <path>`.");
            }
            for root in &config.roots {
                println!(
                    "{:<24} {:<24} {:<12} {}",
                    root.id,
                    root.label,
                    match (root.writable, root.runs_commands(), root.network) {
                        (true, true, true) => "commands+net",
                        (true, true, false) => "commands",
                        (true, false, _) => "changes",
                        (false, _, _) => "read-only",
                    },
                    root.path.display()
                );
            }
        }
        RootsCommand::Remove { root } => {
            let removed = remove_root(&mut config, &root)?;
            config.save(paths)?;
            println!(
                "Stopped sharing {} ({}).",
                removed.path.display(),
                removed.id
            );
            notify_daemon(paths, "");
        }
        RootsCommand::AllowChanges { root, i_know } => {
            allow_changes(paths, &mut config, &root, true, i_know)?
        }
        RootsCommand::DenyChanges { root } => {
            allow_changes(paths, &mut config, &root, false, false)?
        }
        RootsCommand::AllowCommands { root, network } => {
            let root = cww::roots::set_commands(&mut config, &root, true, network)?;
            config.save(paths)?;
            println!(
                "Chat with Work can now run commands in {} ({}), {}. Each command asks you first \
                 in Chat with Work and runs in a sandbox that can change only this folder.",
                root.label,
                root.path.display(),
                if root.network {
                    "with the network"
                } else {
                    "without the network"
                }
            );
            notify_daemon(paths, "");
        }
        RootsCommand::DenyCommands { root } => {
            let root = cww::roots::set_commands(&mut config, &root, false, false)?;
            config.save(paths)?;
            println!(
                "{} ({}) runs no commands now.",
                root.label,
                root.path.display()
            );
            notify_daemon(paths, "");
        }
    }
    Ok(())
}

/// `cww roots allow-changes` and `deny-changes`.
fn allow_changes(
    paths: &Paths,
    config: &mut Config,
    which: &str,
    writable: bool,
    i_know: bool,
) -> Result<()> {
    let root = set_writable(config, which, writable, i_know)?;
    config.save(paths)?;
    if writable {
        println!(
            "Chat with Work can now change files in {} ({}). Each change asks you first in \
             Chat with Work, and old versions and deleted files go to the system trash.",
            root.label,
            root.path.display()
        );
    } else {
        println!(
            "{} ({}) is read-only again.",
            root.label,
            root.path.display()
        );
    }
    notify_daemon(paths, "");
    Ok(())
}

fn status(paths: &Paths, json: bool) -> Result<()> {
    let Some(status) = control::request(&paths.socket_path(), ControlRequest::Status)? else {
        let config = Config::load(paths)?;
        if json {
            println!(
                "{}",
                serde_json::json!({ "running": false, "paired": config.server.is_some() })
            );
        } else {
            println!(
                "The daemon is not running. Start it with `cww daemon install` or `cww daemon run`."
            );
            match &config.server {
                Some(s) => println!("Paired with {} as device {}.", s.url, s.device_id),
                None => println!("Not paired. Run `cww login`."),
            }
        }
        return Ok(());
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    let conn = &status["connection"];
    println!(
        "Daemon:     running (pid {}, cww {})",
        status["pid"],
        str(&status["version"])
    );
    println!(
        "Server:     {}",
        status["server"].as_str().unwrap_or("not paired")
    );
    if status["paired"] == Value::Bool(true) {
        println!("Proxy:      {}", describe_proxy(&status["proxy"]));
    }
    println!(
        "Connection: {} since {}{}",
        str(&conn["connection"]).replace('_', " "),
        str(&conn["since"]),
        conn["last_error"]
            .as_str()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    );
    println!(
        "Tool calls: {}",
        if status["paused"] == Value::Bool(true) {
            "PAUSED"
        } else {
            "answered"
        }
    );
    println!("Roots:");
    let roots = status["roots"].as_array().cloned().unwrap_or_default();
    if roots.is_empty() {
        println!("  none; add one with `cww roots add <path>`");
    }
    for r in roots {
        println!(
            "  {:<20} {:<20} index {} ({} files){}{}  {}",
            str(&r["id"]),
            str(&r["label"]),
            str(&r["index"]),
            r["indexed_files"].as_u64().unwrap_or(0),
            if r["writable"] == Value::Bool(true) {
                ", changes allowed"
            } else {
                ""
            },
            if r["available"] == Value::Bool(false) {
                " UNAVAILABLE"
            } else {
                ""
            },
            str(&r["local_path"]),
        );
    }
    Ok(())
}

fn pause(paths: &Paths, paused: bool) -> Result<()> {
    let request = if paused {
        ControlRequest::Pause
    } else {
        ControlRequest::Resume
    };
    if control::request(&paths.socket_path(), request)?.is_none() {
        let mut config = Config::load(paths)?;
        config.paused = paused;
        config.save(paths)?;
        println!("The daemon is not running; saved for its next start.");
    }
    println!(
        "{}",
        if paused {
            "Paused: tool calls from Chat with Work are refused."
        } else {
            "Resumed: tool calls are answered again."
        }
    );
    Ok(())
}

const START_DAEMON_HINT: &str = "Start the daemon with `cww daemon install` (or `cww daemon run`).";

/// What became of the daemon after pairing.
#[derive(Debug)]
enum AfterLogin {
    /// It was running and re-read the config.
    Reloaded,
    /// It wasn't running, so its service was installed, which starts it.
    Started,
    /// It isn't running, and wasn't to be started.
    NotRunning,
    Failed(String),
}

impl AfterLogin {
    fn name(&self) -> &'static str {
        match self {
            Self::Reloaded => "reloaded",
            Self::Started => "started",
            Self::NotRunning => "not_running",
            Self::Failed(_) => "failed",
        }
    }
}

/// After pairing: a running daemon re-reads the config, and otherwise,
/// when `start`, the daemon is installed as a service, which starts it now
/// and at every login.
fn daemon_after_login(paths: &Paths, start: bool) -> AfterLogin {
    match control::request(&paths.socket_path(), ControlRequest::Reload) {
        Ok(Some(_)) => AfterLogin::Reloaded,
        Ok(None) if !start => AfterLogin::NotRunning,
        Ok(None) => match service::install(
            paths,
            &service::InstallOptions {
                no_hardening: false,
            },
        ) {
            Ok(_) => AfterLogin::Started,
            Err(e) => AfterLogin::Failed(format!("couldn't start the daemon: {e:#}")),
        },
        Err(e) => AfterLogin::Failed(format!("couldn't reach the daemon: {e:#}")),
    }
}

/// Warn when a proxy is set in this shell only; the service won't see it.
fn proxy_hint(paths: &Paths) {
    let setting = Config::load(paths).ok().and_then(|c| c.proxy);
    if let Some(hint) = cww::proxy::service_hint(setting.as_deref(), &paths.config_file()) {
        println!();
        println!("{hint}");
    }
}

/// `cww login --json`: one JSON object per line on stdout. First
/// `{"event":"code",...}` with what to show and open, then `{"event":"paired",...}`
/// or `{"event":"error",...}`. On success the daemon is started as by
/// `cww login` (reloaded if running, else installed unless `start` is
/// false), and `paired` says how that went.
fn login_json(
    paths: &Paths,
    server: &str,
    name: Option<String>,
    open: bool,
    without_chats: bool,
    start: bool,
    organization: bool,
) -> Result<()> {
    use std::sync::atomic::AtomicBool;

    let emit = |value: Value| println!("{value}");
    let options = auth::LoginOptions {
        name,
        store: None,
        open_browser: false,
        without_chats,
    };
    let pending = if organization {
        auth::start_organization_login(paths, server, options)
    } else {
        auth::start_login(paths, server, options)
    };
    let result = pending.and_then(|pending| {
        let opened = open && pending.browser_url().is_some_and(cww::browser::open);
        emit(serde_json::json!({
            "event": "code",
            "user_code": pending.user_code(),
            "verification_uri": pending.verification_uri(),
            "verification_uri_complete": pending.verification_uri_complete(),
            "browser_url": pending.browser_url(),
            "device_name": pending.device_name(),
            "fingerprint": pending.fingerprint(),
            "opened": opened,
        }));
        if organization {
            pending
                .wait_for_organization(&AtomicBool::new(false))
                .map(|profile| (profile.server.clone(), Some(profile)))
        } else {
            pending
                .wait(&AtomicBool::new(false))
                .map(|server| (server, None))
        }
    });
    match result {
        Ok((paired, profile)) => {
            // The pairing is saved either way: a daemon that couldn't pick it
            // up is reported, not an error.
            let daemon = daemon_after_login(paths, start);
            let mut event = serde_json::json!({
                "event": "paired",
                "server": paired.url,
                "device_id": paired.device_id,
                "daemon": daemon.name(),
            });
            if let Some(profile) = profile {
                event["organization"] = serde_json::to_value(profile)?;
            }
            if let AfterLogin::Failed(e) = &daemon {
                event["daemon_error"] = Value::String(e.clone());
            }
            emit(event);
            Ok(())
        }
        Err(e) => {
            emit(serde_json::json!({ "event": "error", "error": format!("{e:#}") }));
            std::process::exit(1);
        }
    }
}

/// Ask a running daemon to re-read the config.
fn notify_daemon(paths: &Paths, if_not_running: &str) {
    match control::request(&paths.socket_path(), ControlRequest::Reload) {
        Ok(Some(_)) => println!("The daemon picked up the change."),
        Ok(None) if !if_not_running.is_empty() => println!("{if_not_running}"),
        Ok(None) => {}
        Err(e) => eprintln!("warning: couldn't reach the daemon: {e:#}"),
    }
}

/// The daemon's `proxy` status field: never holds a password.
fn describe_proxy(proxy: &Value) -> String {
    match proxy["url"].as_str() {
        Some(url) => format!("{url} (from {})", str(&proxy["source"])),
        None => "none, connecting directly".into(),
    }
}

fn str(v: &Value) -> &str {
    v.as_str().unwrap_or("?")
}

/// Print the audit log. With `follow`, new entries come from the running
/// daemon as they happen; without a daemon, the file is tailed instead.
fn log(paths: &Paths, lines: usize, follow: bool, raw: bool) -> Result<()> {
    audit::print_log(&paths.audit_file(), lines, false, raw)?;
    if !follow {
        return Ok(());
    }
    let Some(client) = control::Client::connect(&paths.socket_path())? else {
        return audit::print_log(&paths.audit_file(), 0, true, raw);
    };
    for event in client.subscribe(&[control::Topic::Audit])? {
        let event = event?;
        if event["event"] != "audit" {
            continue;
        }
        let line = serde_json::to_string(&event["entry"])?;
        if raw {
            println!("{line}");
        } else {
            println!("{}", audit::format_line(&line));
        }
    }
    eprintln!("The daemon stopped.");
    Ok(())
}

/// After pairing, offer to share the Documents folder. Nothing is shared
/// without an explicit yes; without a terminal to ask on, nothing happens.
fn offer_documents(paths: &Paths) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let hint = "Share a folder with `cww roots add <path>`.";
    let Some(documents) = cww::paths::documents_dir().filter(|d| d.is_dir()) else {
        println!("Nothing is shared yet. {hint}");
        return Ok(());
    };
    if !std::io::stdin().is_terminal() {
        println!("Nothing is shared yet. {hint}");
        return Ok(());
    }
    println!();
    println!("Nothing is shared yet. Chat with Work can search your Documents folder:");
    println!("  {}", documents.display());
    println!("Secrets inside it (keys, .env files, password databases) stay private either way.");
    print!("Share it? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        println!("Not shared. {hint}");
        return Ok(());
    }
    roots(
        paths,
        RootsCommand::Add {
            path: documents,
            label: Some("Documents".into()),
            i_know: false,
            follow_symlinks: false,
            allow_changes: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_organization_ids_can_start_with_a_hyphen() {
        // Ed25519 public keys use URL-safe Base64, whose alphabet includes '-'.
        let id = "-aBcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
        for subcommand in ["logout", "tui"] {
            let cli = Cli::try_parse_from(["cww", subcommand, "--organization", id]).unwrap();
            let Some(Command::Logout { organization } | Command::Tui { organization }) =
                cli.command
            else {
                panic!("organization command");
            };
            assert_eq!(organization.as_deref(), Some(id));
        }
    }
}
