use std::path::PathBuf;

// mimalloc returns freed memory to the system; see Cargo.toml.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;

use cww::bots::{self, Bots};
use cww::config::{Config, DEFAULT_SERVER};
use cww::control::{self, ControlRequest};
use cww::paths::Paths;
use cww::roots::{NewRoot, add_root, remove_root, set_writable};
use cww::{audit, auth, crash_reports, daemon, service};

/// Chat with Work Desktop.
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
        /// Also ask to manage your bots with `cww bots`.
        #[arg(long)]
        bots: bool,
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
    /// Lend this computer to a coding bot: a folder, Claude Code signed in
    /// for it, the repository and its toolchain, checked in the sandbox
    /// (docs/coding-computers.md).
    Coding {
        #[command(subcommand)]
        command: CodingCommand,
    },
    /// Let Chat with Work think with your own ChatGPT (Codex) or Claude
    /// sign-in on this computer, for your chats or a bot (docs/own-plans.md).
    Plans {
        #[command(subcommand)]
        command: Option<PlansCommand>,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage the printers Chat with Work may use.
    Printers {
        #[command(subcommand)]
        command: Option<PrintersCommand>,
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
    /// Let Chat with Work use your own browser, through the Chat with Work
    /// extension, on sites you allow.
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// List organization sign-ins saved on this computer.
    Organizations {
        #[arg(long)]
        json: bool,
    },
    /// Make and manage your bots, for you or an agent. Allowed separately
    /// from folders and chats, in `cww login --bots` or in Settings.
    Bots {
        /// Saved organization name or ID from `cww organizations`.
        #[arg(
            long,
            global = true,
            allow_hyphen_values = true,
            alias = "organization"
        )]
        org: Option<String>,
        /// Print JSON for a program to read. Failures print {"error": {...}}.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        command: BotsCommand,
    },
}

#[derive(Subcommand)]
enum BotsCommand {
    /// List the bots you can read.
    List,
    /// Show one bot.
    Show { id: String },
    /// List recipes, or show every choice one asks. KEY "blank" is from nothing.
    Recipes {
        key: Option<String>,
        /// See the choices with this tool in a slot: kind=tool.
        #[arg(long = "tool", value_parser = bots::pair)]
        tools: Vec<(String, String)>,
    },
    /// List or search what can start a bot, or show one app's events and fields.
    Triggers {
        /// Words to search for, or an app's key with --app.
        query: Option<String>,
        /// Show this app's events and fields.
        #[arg(long)]
        app: Option<String>,
    },
    /// Make a bot, from a recipe or from nothing. Spends nothing by itself.
    Create(Box<BotForm>),
    /// Change a bot. Only what's given changes.
    Update(Box<BotChange>),
    /// Stop a bot answering until it's resumed.
    Pause { id: String },
    /// Let a paused bot answer again.
    Resume { id: String },
    /// Delete a bot. Its chats stay.
    Delete {
        id: String,
        /// Don't ask to confirm.
        #[arg(long)]
        yes: bool,
    },
    /// Try a bot once. It changes nothing outside, and uses your credits.
    Run(Box<BotRun>),
}

#[derive(clap::Args, Default)]
struct BotFields {
    /// Start from this recipe (see `cww bots recipes`).
    #[arg(long)]
    recipe: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    /// What it should do.
    #[arg(long)]
    instructions: Option<String>,
    /// Plain settings in YAML, with flags over them. Never secrets.
    #[arg(long)]
    file: Option<PathBuf>,
    /// The tool for one of the recipe's slots: kind=tool, or kind=none.
    #[arg(long = "tool", value_parser = bots::pair)]
    tools: Vec<(String, String)>,
    /// Another service it uses, as you. It must be connected.
    #[arg(long = "integration")]
    integrations: Vec<String>,
    /// Let it run code in the sandbox.
    #[arg(long)]
    code: bool,
    /// When it tells you: when_needed, always, or never.
    #[arg(long)]
    notify: Option<String>,
    /// Its changes: ask, or automatic, the same choice as on New bot.
    #[arg(long)]
    writes: Option<String>,
    /// Share it in a project (owners and admins).
    #[arg(long)]
    shared: bool,
    /// The project's number, for --shared.
    #[arg(long)]
    project: Option<u64>,
    /// A shared bot's monthly credits.
    #[arg(long)]
    cap: Option<u64>,
    /// What starts it, an app from `cww bots triggers`.
    #[arg(long)]
    app: Option<String>,
    /// One of the app's events that starts it. Repeat for several.
    #[arg(long = "event")]
    events: Vec<String>,
    /// One of the app's fields: key=value.
    #[arg(long = "trigger-field", value_parser = bots::pair)]
    trigger_fields: Vec<(String, String)>,
    /// day, weekday, week, month, or custom.
    #[arg(long)]
    frequency: Option<String>,
    /// The time of day, like 08:30.
    #[arg(long)]
    at: Option<String>,
    /// For --frequency week.
    #[arg(long)]
    on_weekday: Option<String>,
    /// For --frequency month.
    #[arg(long)]
    on_day: Option<String>,
    /// Run once, at this date and time, like 2026-12-01T09:00.
    #[arg(long)]
    once: Option<String>,
    /// In words, like "every friday at 16:00".
    #[arg(long)]
    recurrence: Option<String>,
    #[arg(long)]
    time_zone: Option<String>,
}

impl BotFields {
    fn into_form(self) -> (bots::Form, Option<PathBuf>) {
        let file = self.file;
        let form = bots::Form {
            recipe: self.recipe,
            name: self.name,
            description: self.description,
            instructions: self.instructions,
            tools: self.tools,
            integrations: self.integrations,
            code: self.code,
            notify: self.notify,
            writes: self.writes,
            shared: self.shared,
            project: self.project,
            cap: self.cap,
            app: self.app,
            events: self.events,
            trigger_fields: self.trigger_fields,
            frequency: self.frequency,
            at: self.at,
            on_weekday: self.on_weekday,
            on_day: self.on_day,
            once: self.once,
            recurrence: self.recurrence,
            time_zone: self.time_zone,
        };
        (form, file)
    }
}

#[derive(clap::Args)]
struct BotForm {
    #[command(flatten)]
    fields: BotFields,
}

#[derive(clap::Args)]
struct BotRun {
    /// A bot that exists. Without it, tries the form the other flags give.
    id: Option<String>,
    #[command(flatten)]
    fields: BotFields,
    /// Try it on this recent event, not the newest.
    #[arg(long)]
    sample: Option<String>,
    /// Start it and print its ID without waiting.
    #[arg(long)]
    no_wait: bool,
    /// Follow a try already started, by its ID.
    #[arg(long, conflicts_with = "no_wait")]
    follow: Option<u64>,
    /// How long to wait, in seconds.
    #[arg(long, default_value_t = 300)]
    timeout: u64,
}

#[derive(clap::Args)]
struct BotChange {
    id: String,
    /// Plain settings in YAML, with flags over them. Never secrets.
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    instructions: Option<String>,
    /// When it runs, in words, or a date and time for once.
    #[arg(long)]
    when: Option<String>,
    #[arg(long)]
    time_zone: Option<String>,
    /// A service of yours to add. It must be connected.
    #[arg(long = "add-integration")]
    add: Vec<String>,
    #[arg(long = "remove-integration")]
    remove: Vec<String>,
    /// When it tells you: when_needed, always, or never.
    #[arg(long)]
    notify: Option<String>,
    /// Your own connections' changes: ask, or automatic.
    #[arg(long)]
    changes: Option<String>,
    /// How it replies: draft, or automatic.
    #[arg(long)]
    reply_mode: Option<String>,
    /// A shared bot's monthly credits.
    #[arg(long)]
    cap: Option<u64>,
    /// One integration's changes: provider=off|ask|automatic.
    #[arg(long = "writes", value_parser = bots::pair)]
    writes: Vec<(String, String)>,
    /// One tool's policy: tool=off|ask|automatic.
    #[arg(long = "policy", value_parser = bots::pair)]
    policies: Vec<(String, String)>,
    /// Confirm a policy that lets a destructive tool run without asking.
    #[arg(long)]
    yes: bool,
}

#[derive(Subcommand)]
enum BrowserCommand {
    /// List the running daemon's browser tools and their JSON schemas.
    Tools,
    /// Call a browser tool through the running daemon, returning MCP JSON.
    Call {
        tool: String,
        /// JSON object from a file, or - for stdin. Defaults to {}.
        #[arg(long)]
        input: Option<PathBuf>,
        /// Acknowledge the person's approval for this specific action,
        /// after reviewing its dry run. Never enables sites or actions.
        #[arg(long)]
        approve: bool,
    },
    /// Show whether the browser is on, its sites and the connected browser.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Offer the browser tools, and register cww with Chrome, Chromium,
    /// Edge and Brave so the extension can reach it.
    Enable,
    /// Stop offering the browser tools, and remove the registration.
    Disable,
    /// Register cww with the browsers again (after installing a browser).
    Register,
    /// Allow a site: a host like github.com (with its subdomains, over
    /// HTTPS) or an origin like http://localhost:3000.
    Allow { site: String },
    /// Remove an entry from the allowed list. Use block to refuse it in either mode.
    Disallow { site: String },
    /// Allow every HTTP(S) site except the blocked sites, in the app's tab group.
    AllowAll,
    /// Use only the allowed sites again. Keeps both site lists.
    AllowListed,
    /// Block a site in both modes. A host covers subdomains and every HTTP(S) port.
    Block { site: String },
    /// Remove a site from the blocked list.
    Unblock { site: String },
    /// Let Chat with Work also act on pages: open, click, type, choose,
    /// close tabs and save files, each approved in Chat with Work.
    AllowChanges,
    /// Ask before each browser action again (the default).
    Ask,
    /// Run permitted browser actions without repeated approval, including submissions.
    AutomaticActions,
    /// Let Chat with Work only read pages again.
    DenyChanges,
    /// Stop Chat with Work using the browser now, until `resume`.
    Stop,
    /// Let Chat with Work use the browser again.
    Resume,
    /// Let a Chat with Work bot's chats use the browser, by the number in
    /// the bot's address in Chat with Work.
    #[command(name = "share", alias = "lend")]
    Lend {
        bot: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Stop sharing the browser with a bot.
    #[command(name = "unshare", alias = "stop-lending")]
    StopLending { bot: String },
}

#[derive(Subcommand)]
enum PrintersCommand {
    /// List this computer's printers, and which Chat with Work may use.
    List,
    /// Let your chats print on a printer, each job approved in Chat with
    /// Work. `--bots-only` keeps it for the bots you lend it to.
    Add {
        /// The system's name for it, as `cww printers` lists it.
        queue: String,
        /// What to call it in Chat with Work.
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        bots_only: bool,
    },
    /// Stop Chat with Work using a printer, for chats and bots alike.
    Remove { printer: String },
    /// Lend a printer to a Chat with Work bot, by the number in its address
    /// in Chat with Work. Each job asks you first, unless `--unattended`
    /// lets the bot print here without anyone approving it.
    Share {
        printer: String,
        bot: String,
        /// The bot's name, to recognize it here.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        unattended: bool,
    },
    /// Stop lending a printer to a bot.
    Unshare { printer: String, bot: String },
}

#[derive(Subcommand)]
enum PlansCommand {
    /// Let your chats (or, with --bot, a bot) think with this agent's
    /// sign-in here, or stop (--off).
    Allow {
        /// codex or claude.
        agent: String,
        /// The bot, by its ID in Chat with Work.
        #[arg(long)]
        bot: Option<String>,
        #[arg(long)]
        off: bool,
    },
}

#[derive(Subcommand)]
enum CodingCommand {
    /// Set this computer up for a coding bot, one step at a time. Asks
    /// before each thing it allows or runs; run again, it skips what's done.
    Setup {
        /// The bot, by name (part of it is enough when only one bot has
        /// it) or by its ID. Without it, setup lists your bots to pick from.
        #[arg(long)]
        bot: Option<String>,
        /// The bot's name, to recognize it here.
        #[arg(long)]
        name: Option<String>,
        /// The coding agent it uses: claude (the default), codex, opencode, gemini or pi.
        #[arg(long)]
        agent: Option<String>,
        /// The folder it works in (default ~/bots).
        #[arg(long)]
        folder: Option<PathBuf>,
        /// The repository's SSH or https address.
        #[arg(long)]
        repo: Option<String>,
        /// A deploy key to fetch it with (kept where it is, outside the
        /// shared folders), instead of the keys `ssh` would offer.
        #[arg(long)]
        ssh_key: Option<PathBuf>,
        /// Let the bot's commands there start without anyone approving each.
        #[arg(long)]
        unattended: bool,
        /// The command that runs the repository's tests.
        #[arg(long)]
        test_command: Option<String>,
        /// Answer yes to every question (never turns on --unattended).
        #[arg(long)]
        yes: bool,
        /// Don't install the repository's toolchain or dependencies.
        #[arg(long)]
        skip_install: bool,
        /// Don't run the tests and Claude Code in the sandbox at the end.
        #[arg(long)]
        skip_proof: bool,
    },
    /// What each bot's folders have, and the repositories' mirrors.
    Status {
        #[arg(long)]
        bot: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Let a bot's commands in a folder lent to it use this computer's
    /// Claude Code sign-in, or stop (`--off`).
    Allow {
        root: String,
        bot: String,
        #[arg(long)]
        off: bool,
        /// The coding agent: claude (the default), codex, opencode, gemini or pi.
        #[arg(long)]
        agent: Option<String>,
        /// Let this bot's first runs here prepare repositories of this GitHub
        /// owner by themselves, with your Git login.
        #[arg(long)]
        owner: Option<String>,
    },
    /// Fetch every repository's mirror now.
    Fetch,
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
    /// with Work (or run without asking in a chat set to Allow everything)
    /// and run in a sandbox that can change only that folder.
    AllowCommands {
        root: String,
        /// Let those commands reach the internet and the local network
        /// (installing packages), this computer's own services with them
        /// (not on Windows, whose sandbox can't give them).
        #[arg(long, visible_alias = "internet")]
        network: bool,
        /// Keep them from this computer's own services (a database in
        /// Docker, a development server), which they reach by default.
        #[arg(long, conflicts_with_all = ["local_services", "network"])]
        no_local_services: bool,
        /// Let them reach this computer's own services (the default).
        #[arg(long)]
        local_services: bool,
    },
    /// Let a folder's commands reach this computer's own services, or not:
    /// its loopback, its own addresses, and its containers' networks, never
    /// the local network or the internet. Windows' sandbox can't give them.
    LocalServices {
        root: String,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Let a folder's commands reach one exact address on the local network
    /// (`192.168.1.20:5432`), or stop with `--remove`.
    Reach {
        root: String,
        address: String,
        #[arg(long)]
        remove: bool,
    },
    /// Stop running commands in a shared folder.
    DenyCommands { root: String },
    /// Windows: let commands see the top of the drives they run on, which
    /// Node.js, npm and cmd's `dir` look at and Windows keeps from
    /// sandboxes. Sandboxed programs can then read the drive's own details,
    /// never what's in it. Asks for an administrator's approval.
    PrepareCommands,
    /// Share a folder with a Chat with Work bot, by the number in its
    /// address in Chat with Work: the bot's chats then reach this folder,
    /// as the folder allows. Each change and command asks you first, unless
    /// `--unattended` lets its commands here start without asking.
    #[command(name = "share", alias = "lend")]
    Lend {
        root: String,
        bot: String,
        /// The bot's name, to recognize it here.
        #[arg(long)]
        name: Option<String>,
        /// Let the bot's commands here start without anyone approving each
        /// one. Only where commands are allowed.
        #[arg(long)]
        unattended: bool,
    },
    /// Stop sharing a folder with a bot.
    #[command(name = "unshare", alias = "stop-lending")]
    StopLending { root: String, bot: String },
    /// List the bots asking, about a folder shared with them, to run their
    /// commands unattended or to run Claude Code.
    Requests,
    /// Allow a bot's request to run its commands unattended in a folder
    /// (or with --claude-code, to run Claude Code with this computer's
    /// sign-in there), through the daemon, which records it. It lasts
    /// until the folder's sharing changes.
    Allow {
        root: String,
        bot: String,
        #[arg(long)]
        claude_code: bool,
    },
    /// Turn down a bot's request about a folder.
    Decline {
        root: String,
        bot: String,
        #[arg(long)]
        claude_code: bool,
    },
    /// Set how many characters a chat may read from a shared folder in an
    /// hour, when `[limits]` sets an hourly read limit: from that limit to
    /// 20,000,000, for a folder worked in a lot, such as code. All chats
    /// together may read twice it. Reading has no hourly limit by default,
    /// and then this has no effect. `default` puts the folder back on the
    /// limit every folder shares.
    ReadBudget {
        root: String,
        /// Characters per chat per hour, or `default`.
        characters: String,
    },
    /// The environment every command in a folder gets: `NAME=value` lines,
    /// dotenv-style, kept in this computer's secret store. A value can be
    /// `$(command)`, run in the command's own sandbox when a run starts.
    /// Without a flag, lists the names.
    Env {
        root: String,
        /// Replace the list with standard input's.
        #[arg(long, conflicts_with = "clear")]
        stdin: bool,
        /// Forget the list.
        #[arg(long)]
        clear: bool,
    },
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
    // `cww status | head` closes the pipe early: end quietly, never a
    // crash report (crash_reports::install keeps this hook first).
    crash_reports::exit_quietly_on_broken_pipe();
    // Started by a thinking step's agent, in its sandbox, as the MCP server
    // of Chat with Work's tools (docs/own-plans.md): nothing else runs.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "plan-tools") {
        let Some(file) = args.get(2) else {
            eprintln!("usage: cww plan-tools <tools.json>");
            std::process::exit(2);
        };
        if let Err(e) = cww::plans::shim::serve(std::path::Path::new(file)) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    // Started by the browser as the extension's native messaging host.
    cww::browsing::host_if_invoked();
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
        Command::Browser { command } => browser(&paths, command)?,
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
            bots,
        } => login_json(
            &paths,
            &server,
            auth::LoginOptions {
                name,
                store: None,
                open_browser: !no_browser,
                without_chats: no_chats,
                with_bots: bots,
            },
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
            bots,
        } => {
            let options = auth::LoginOptions {
                name,
                store: None,
                open_browser: !no_browser,
                without_chats: no_chats,
                with_bots: bots,
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
        Command::Coding { command } => coding(&paths, command)?,
        Command::Plans { command, json } => plans(&paths, command, json)?,
        Command::Printers { command } => {
            printers(&paths, command.unwrap_or(PrintersCommand::List))?
        }
        Command::Bots { org, json, command } => {
            if let Err(error) = bots_command(&paths, org.as_deref(), json, command) {
                if json {
                    println!("{}", bots::error_json(&error));
                } else {
                    for line in bots::error_lines(&error) {
                        eprintln!("{line}");
                    }
                }
                std::process::exit(1);
            }
        }
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

fn printers(paths: &Paths, command: PrintersCommand) -> Result<()> {
    use cww::printing::{settings, system_printers};
    let mut config = Config::load(paths)?;
    let system = || system_printers().map_err(|e| anyhow::anyhow!(e.message));
    match command {
        PrintersCommand::List => {
            for printer in system()? {
                let allowed = config.printers.iter().find(|p| p.queue == printer.queue);
                let uses = allowed.map_or_else(
                    || "not used by Chat with Work".to_string(),
                    |p| {
                        let mut uses = Vec::new();
                        if p.chats {
                            uses.push("your chats".to_string());
                        }
                        uses.extend(p.bots.iter().map(|loan| {
                            format!(
                                "bot {}{}",
                                loan.id,
                                if loan.unattended { " (unattended)" } else { "" }
                            )
                        }));
                        format!("{}: {}", p.id, uses.join(", "))
                    },
                );
                println!("{} ({}), {uses}", printer.queue, printer.state);
            }
        }
        PrintersCommand::Add {
            queue,
            label,
            bots_only,
        } => {
            let printer = settings::add(
                &mut config,
                &system()?,
                &queue,
                label.as_deref(),
                !bots_only,
            )?;
            config.save(paths)?;
            println!(
                "Chat with Work can use {} as {}.",
                printer.label, printer.id
            );
            notify_daemon(paths, "");
        }
        PrintersCommand::Remove { printer } => {
            let printer = settings::remove(&mut config, &printer)?;
            config.save(paths)?;
            println!("Chat with Work can't use {} any more.", printer.label);
            notify_daemon(paths, "");
        }
        PrintersCommand::Share {
            printer,
            bot,
            name,
            unattended,
        } => {
            let printer = settings::set_bot(
                &mut config,
                &printer,
                &bot,
                name.as_deref(),
                true,
                unattended,
            )?;
            config.save(paths)?;
            println!(
                "{} is shared with bot {bot}. {}",
                printer.label,
                if unattended {
                    "It prints here without anyone approving each job."
                } else {
                    "Each job waits for you to approve it in Chat with Work."
                }
            );
            notify_daemon(paths, "");
        }
        PrintersCommand::Unshare { printer, bot } => {
            let printer = settings::set_bot(&mut config, &printer, &bot, None, false, false)?;
            config.save(paths)?;
            println!("{} is no longer shared with bot {bot}.", printer.label);
            notify_daemon(paths, "");
        }
    }
    Ok(())
}

fn coding(paths: &Paths, command: CodingCommand) -> Result<()> {
    use cww::coding::{self, repositories, setup};
    let mut config = Config::load(paths)?;
    if config.coding.legacy_token {
        // Runs use Claude Code's own sign-in now: the token an earlier
        // version kept goes. A store that refuses never stops the command
        // (`allow --off` above all); the note stays, so the next one tries
        // again. Its words go to stderr, keeping `--json` one document.
        match coding::forget_legacy_token(&mut config, paths).and_then(|()| config.save(paths)) {
            Ok(()) => eprintln!(
                "Deleted the Claude Code token Desktop kept: bots' runs now use this computer's \
                 own Claude Code sign-in. Revoke that token at claude.ai (Settings, Claude Code) \
                 if you no longer use it."
            ),
            Err(e) => {
                config.coding.legacy_token = true;
                eprintln!(
                    "Couldn't delete the Claude Code token Desktop kept ({e:#}); the next `cww \
                     coding` tries again."
                );
            }
        }
    }
    match command {
        CodingCommand::Setup {
            bot,
            name,
            agent,
            folder,
            repo,
            ssh_key,
            unattended,
            test_command,
            yes,
            skip_install,
            skip_proof,
        } => setup::setup(
            paths,
            setup::Options {
                bot,
                bot_name: name,
                agent: Some(coding_agent(agent.as_deref())?),
                folder,
                repo,
                ssh_key,
                unattended: unattended.then_some(true),
                test_command,
                yes,
                skip_install,
                skip_proof,
            },
        )?,
        CodingCommand::Status { bot, json } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&setup::status_json(
                        &config,
                        paths,
                        bot.as_deref()
                    ))?
                );
            } else {
                setup::summary(&config, paths, bot.as_deref());
            }
        }
        CodingCommand::Allow {
            root,
            bot,
            off,
            agent,
            owner,
        } => {
            let agent = coding_agent(agent.as_deref())?;
            let root_id = root.clone();
            let root = cww::roots::set_bot_agent(&mut config, &root, &bot, agent, !off)?;
            if let Some(owner) = owner {
                cww::roots::add_bot_owner(&mut config, &root_id, &bot, &owner)?;
            }
            config.save(paths)?;
            println!(
                "Bot {bot}'s commands in {} {} {}'s sign-in.",
                root.label,
                if off { "don't get" } else { "get" },
                agent.name()
            );
            notify_daemon(paths, "");
        }
        CodingCommand::Fetch => {
            for repo in &config.coding.repositories {
                let mirror = repositories::mirror_dir(paths, repo);
                if repositories::writable_by_commands(&mirror, &config.roots) {
                    println!(
                        "{} isn't fetched: its mirror is in a shared folder that allows changes",
                        repo.name
                    );
                    continue;
                }
                match repositories::fetch(&mirror, Some(repo)) {
                    Ok(()) => println!("Fetched {} ({}).", repo.name, repo.url),
                    Err(e) => println!("{} didn't fetch: {e:#}", repo.name),
                }
            }
        }
    }
    Ok(())
}

fn roots(paths: &Paths, command: RootsCommand) -> Result<()> {
    let mut config = Config::load(paths)?;
    match command {
        RootsCommand::Requests => {
            let Some(status) = control::request(&paths.socket_path(), ControlRequest::Status)?
            else {
                anyhow::bail!("the daemon is not running, so no request is waiting");
            };
            let asks = status["unattended_asks"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if asks.is_empty() {
                println!("No bot is asking anything of this computer.");
            }
            for ask in asks {
                println!("{}", describe_ask(&ask));
            }
        }
        RootsCommand::Allow {
            root,
            bot,
            claude_code,
        } => {
            let request = ControlRequest::RootsUnattendedAnswer {
                root,
                bot: bot.clone(),
                allow: true,
                kind: ask_kind(claude_code),
                asked_at: None,
            };
            if control::request(&paths.socket_path(), request)?.is_none() {
                anyhow::bail!("the daemon is not running, so no request is waiting");
            }
            if claude_code {
                println!("Bot {bot}'s runs there now use this computer's Claude Code sign-in.");
            } else {
                println!("Bot {bot}'s commands there now run without asking each time.");
            }
        }
        RootsCommand::Decline {
            root,
            bot,
            claude_code,
        } => {
            let request = ControlRequest::RootsUnattendedAnswer {
                root,
                bot: bot.clone(),
                allow: false,
                kind: ask_kind(claude_code),
                asked_at: None,
            };
            if control::request(&paths.socket_path(), request)?.is_none() {
                anyhow::bail!("the daemon is not running, so no request is waiting");
            }
            if claude_code {
                println!("Bot {bot}'s runs there don't get Claude Code's sign-in.");
            } else {
                println!("Bot {bot}'s commands there still wait for approval in Chat with Work.");
            }
        }
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
                    match (root.writable, root.runs_commands()) {
                        (true, true) => "commands",
                        (true, false) => "changes",
                        (false, _) => "read-only",
                    },
                    root.path.display()
                );
                if root.runs_commands() {
                    println!(
                        "{:<24} network: {}",
                        "",
                        cww::commands::network_access(root).words()
                    );
                    for address in &root.reach {
                        println!("{:<24} also reaches {address}", "");
                    }
                }
            }
        }
        RootsCommand::Remove { root } => {
            // Through the running agent, which records it in the audit log
            // as it does every removal, tells whoever follows the log, and
            // is then the only one writing it. By hand only when no agent
            // answers, or its organization keeps folders to the app.

            // The folder as this terminal names it: a relative path means
            // one thing here and another in the agent's working directory.
            let id = cww::roots::find(&config, &root)?.id.clone();
            let answer = match control::Client::connect(&paths.socket_path()) {
                Ok(Some(mut client)) => {
                    match client.call_raw(&ControlRequest::RootsRemove { root: id.clone() }) {
                        Ok(answer) => Some(answer),
                        // An agent that can't answer doesn't keep the folder
                        // shared: it's removed here, as with no agent, and an
                        // agent that gets to it later finds it gone.
                        Err(e) => {
                            eprintln!(
                                "warning: the daemon didn't answer ({e:#}); removing it here"
                            );
                            None
                        }
                    }
                }
                _ => None,
            };
            match answer {
                Some(answer) if answer["ok"] == true => println!(
                    "Stopped sharing {} ({}).",
                    str(&answer["root"]["path"]),
                    str(&answer["root"]["id"])
                ),
                Some(answer) if answer["code"] != "chat_only" => anyhow::bail!(
                    "{}",
                    answer["error"]
                        .as_str()
                        .unwrap_or("the daemon returned an error")
                ),
                _ => {
                    // Under the settings' lock the agent edits them under
                    // too, so an agent that didn't answer can't remove it
                    // at the same moment; it may have removed it already.
                    let removed = Config::edit(paths, |config| {
                        if !config.roots.iter().any(|r| r.id == id) {
                            return Ok(None);
                        }
                        let removed = remove_root(config, &id)?;
                        config.folder_env.retain(|each| each != &removed.id);
                        Ok(Some((removed, config.clone())))
                    })?;
                    let Some((removed, config)) = removed else {
                        println!("Stopped sharing {id}.");
                        eprintln!(
                            "warning: the daemon removed it without answering, so its audit \
                             entry may be missing"
                        );
                        return Ok(());
                    };
                    cww::commands::env::delete_logged(&config, paths, &removed.id);
                    audit_here(
                        paths,
                        "root_removed",
                        format!("{} ({})", removed.id, removed.label),
                    );
                    println!(
                        "Stopped sharing {} ({}).",
                        removed.path.display(),
                        removed.id
                    );
                    notify_daemon(paths, "");
                }
            }
        }
        RootsCommand::AllowChanges { root, i_know } => {
            allow_changes(paths, &mut config, &root, true, i_know)?
        }
        RootsCommand::DenyChanges { root } => {
            allow_changes(paths, &mut config, &root, false, false)?
        }
        RootsCommand::AllowCommands {
            root,
            network,
            no_local_services,
            local_services,
        } => {
            let local = if no_local_services {
                Some(false)
            } else if local_services {
                Some(true)
            } else {
                None
            };
            let root = cww::roots::set_commands_reaching(&mut config, &root, true, network, local)?;
            config.save(paths)?;
            println!(
                "Chat with Work can now run commands in {} ({}), network: {}. Each command is \
                 approved in Chat with Work, or runs without asking in a chat where you chose \
                 Allow everything, in a sandbox that can change only this folder.",
                root.label,
                root.path.display(),
                cww::commands::network_access(&root).words()
            );
            notify_daemon(paths, "");
        }
        RootsCommand::LocalServices { root, on } => {
            let root = Config::edit(paths, |config| {
                cww::roots::set_local_services(config, &root, on == "on")
            })?;
            println!(
                "Commands in {} ({}) now reach: {}.{}",
                root.label,
                root.path.display(),
                cww::commands::network_access(&root).words(),
                if cfg!(windows) && on == "on" {
                    " Windows can't give commands this computer's own services."
                } else {
                    ""
                }
            );
            notify_daemon(paths, "");
        }
        RootsCommand::Reach {
            root,
            address,
            remove,
        } => {
            let root = Config::edit(paths, |config| {
                cww::roots::set_reach(config, &root, &address, !remove)
            })?;
            if remove {
                println!("Commands in {} no longer reach {address}.", root.label);
            } else {
                println!(
                    "Commands in {} can reach {} on the local network now.",
                    root.label,
                    cww::reach::describe(
                        address.rsplit_once(':').map_or(&address, |(host, _)| host),
                        address
                            .rsplit_once(':')
                            .and_then(|(_, port)| port.parse().ok())
                            .unwrap_or(0)
                    )
                );
            }
            notify_daemon(paths, "");
        }
        RootsCommand::Lend {
            root,
            bot,
            name,
            unattended,
        } => {
            let root =
                cww::roots::set_bot(&mut config, &root, &bot, name.as_deref(), true, unattended)?;
            config.save(paths)?;
            println!(
                "{} ({}) is shared with bot {bot}. {}",
                root.label,
                root.path.display(),
                if unattended {
                    "Its commands here start without anyone approving each one."
                } else {
                    "Each of its commands here waits for you to approve it in Chat with Work."
                }
            );
            notify_daemon(paths, "");
        }
        RootsCommand::StopLending { root, bot } => {
            let root = cww::roots::set_bot(&mut config, &root, &bot, None, false, false)?;
            config.save(paths)?;
            println!(
                "{} ({}) is no longer shared with bot {bot} now.",
                root.label,
                root.path.display()
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
        RootsCommand::Env { root, stdin, clear } => {
            use cww::commands::env;
            let root = cww::roots::find(&config, &root)?.clone();
            if clear {
                env::clear(&mut config, paths, &root.id)?;
                notify_daemon(paths, "");
                println!("Forgot the environment of {} ({}).", root.label, root.id);
            } else if stdin {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut text)?;
                let variables = env::save(&mut config, paths, &root.id, &text)?;
                notify_daemon(paths, "");
                let names: Vec<&str> = variables.iter().map(|v| v.name.as_str()).collect();
                println!(
                    "{} ({}) has {} in its environment{}.",
                    root.label,
                    root.id,
                    if names.is_empty() {
                        "nothing".to_string()
                    } else {
                        names.join(", ")
                    },
                    if root.runs_commands() {
                        ""
                    } else {
                        ", for when it allows commands"
                    }
                );
            } else {
                // Read as a run reads it: a list gone from the store, or
                // with a mistake, is said, as a run would refuse with it.
                let variables =
                    env::for_run(&config, paths, &root.id).map_err(anyhow::Error::msg)?;
                if variables.is_empty() {
                    println!("{} ({}) has no environment.", root.label, root.id);
                }
                for variable in variables {
                    let command = variable
                        .parts
                        .iter()
                        .any(|p| matches!(p, env::Part::Command(_)));
                    println!(
                        "{}{}",
                        variable.name,
                        if command { " (from a command)" } else { "" }
                    );
                }
            }
        }
        RootsCommand::ReadBudget { root, characters } => {
            let budget = match characters.trim() {
                "default" => None,
                text => Some(text.replace(['_', ','], "").parse::<usize>().map_err(|_| {
                    anyhow::anyhow!("{text:?} isn't a number of characters or `default`")
                })?),
            };
            let root = cww::roots::set_read_budget(&mut config, &root, budget)?;
            config.save(paths)?;
            match root.read_budget {
                Some(_) if !config.limits.reads_capped() => println!(
                    "Saved for {} ({}), but reading has no hourly limit on this computer, so it \
                     counts only once [limits] in config.toml sets one.",
                    root.label,
                    root.path.display()
                ),
                Some(budget) => println!(
                    "A chat may now read {budget} characters an hour from {} ({}), and all chats \
                     together twice that.",
                    root.label,
                    root.path.display()
                ),
                None => println!(
                    "{} ({}) is back on the read budget every folder shares.",
                    root.label,
                    root.path.display()
                ),
            }
            notify_daemon(paths, "");
        }
        RootsCommand::PrepareCommands => prepare_commands(&config)?,
    }
    Ok(())
}

/// `cww roots prepare-commands`: let commands see the top of the system's
/// drive and of the drives of the folders that allow them.
#[cfg(windows)]
fn prepare_commands(config: &Config) -> Result<()> {
    use cww::commands::windows;
    let folders: Vec<PathBuf> = config
        .roots
        .iter()
        .filter(|r| r.runs_commands())
        .map(|r| r.path.clone())
        .collect();
    let roots = windows::drive_roots(&windows::system_root(), &folders);
    let names = windows::drive_names;
    let missing: Vec<PathBuf> = roots
        .iter()
        .filter(|root| !windows::drive_prepared(root))
        .cloned()
        .collect();
    if missing.is_empty() {
        println!("Commands can already see the top of {}.", names(&roots));
        return Ok(());
    }
    println!(
        "Windows asks for an administrator to let sandboxed programs read the top of {} \
         (never what's in it)...",
        names(&missing)
    );
    let prepared = windows::prepare_drives(&missing, None).map_err(anyhow::Error::msg)?;
    println!(
        "Commands can now see the top of {}, so Node.js, npm and cmd's dir work there.",
        names(&prepared)
    );
    Ok(())
}

#[cfg(not(windows))]
fn prepare_commands(_config: &Config) -> Result<()> {
    anyhow::bail!("only Windows keeps the top of a drive from the commands' sandbox")
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
        match str(&conn["connection"]) {
            "revoked" => "signed out".to_string(),
            connection => connection.replace('_', " "),
        },
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
    // Windows only: whether this Windows can sandbox commands, and the
    // drives whose top they can't see yet (docs/commands.md).
    if let Some(commands) = status["commands"].as_object() {
        if commands.get("available") == Some(&Value::Bool(true)) {
            println!("Commands:   can run here, in a Windows process container");
            let unprepared: Vec<String> = commands
                .get("drives")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|d| d["prepared"] == Value::Bool(false))
                .map(|d| str(&d["root"]).trim_end_matches('\\').to_string())
                .collect();
            if !unprepared.is_empty() {
                println!(
                    "            Node.js and cmd can't see the top of {} yet: \
                     `cww roots prepare-commands` lets them",
                    unprepared.join(" and ")
                );
            }
        } else {
            println!(
                "Commands:   can't run here: {}",
                commands
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("no sandbox")
            );
        }
    }
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
        if let Some(access) = r["network_access"].as_str() {
            let level: Option<cww::reach::Level> =
                serde_json::from_value(Value::String(access.to_string())).ok();
            println!(
                "    commands allowed, network: {}",
                level.map_or(access, |l| l.words())
            );
            for address in r["reach"].as_array().into_iter().flatten() {
                println!("    also reaches {}", str(address));
            }
        }
        for loan in r["bots"].as_array().cloned().unwrap_or_default() {
            println!(
                "    shared with bot {}{}{}",
                str(&loan["id"]),
                Some(str(&loan["name"]))
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default(),
                if loan["unattended"] == Value::Bool(true) {
                    ", its commands unattended"
                } else {
                    ""
                }
            );
            let asking = status["unattended_asks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|ask| ask["root"] == r["id"] && ask["bot"] == loan["id"]);
            for ask in asking {
                println!("      {}", describe_ask(ask));
            }
        }
    }
    Ok(())
}

/// The agent a `--agent` names: Claude Code when it names none.
/// `cww plans`: what this computer has for thinking with an agent's own
/// sign-in, and the yes for each.
fn plans(paths: &Paths, command: Option<PlansCommand>, json: bool) -> Result<()> {
    let Some(PlansCommand::Allow { agent, bot, off }) = command else {
        let config = Config::load(paths)?;
        let looks = cww::plans::look(&config, paths, None, cww::commands::think::sandbox().err());
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "plans": looks,
                    "bots": {
                        "codex": config.plans.codex.bots,
                        "claude_code": config.plans.claude_code.bots,
                    },
                }))?
            );
            return Ok(());
        }
        for look in looks {
            let agent = cww::plans::agent(&look.agent).expect("a plan's agent");
            let state = match (look.installed, look.signed_in) {
                (false, _) => "not installed".to_string(),
                (true, false) => format!("not signed in: {}", agent.sign_in_hint()),
                (true, true) => match look.billing {
                    cww::coding::Billing::Plan => "signed in, your plan pays".into(),
                    cww::coding::Billing::ApiKey => "signed in with an API key".into(),
                    cww::coding::Billing::Unknown => "signed in".into(),
                },
            };
            let bots = match agent {
                cww::coding::Agent::Codex => &config.plans.codex.bots,
                _ => &config.plans.claude_code.bots,
            };
            println!(
                "{} ({} plan): {state}",
                look.name,
                cww::plans::plan_name(agent)
            );
            if let Some(why) = &look.unavailable {
                println!("  can't think here: {why}");
            }
            println!(
                "  your chats: {}",
                if look.chats { "allowed" } else { "not allowed" }
            );
            if !bots.is_empty() {
                println!("  bots: {}", bots.join(", "));
            }
        }
        return Ok(());
    };
    let which = cww::plans::agent(&agent)
        .ok_or_else(|| anyhow::anyhow!("{agent:?} isn't codex or claude"))?;
    if let Some(bot) = &bot
        && !cww::plans::valid_bot(bot)
    {
        anyhow::bail!("{bot:?} isn't a bot's ID");
    }
    let whom = cww::plans::for_whom(bot.as_deref());
    let request = ControlRequest::PlansAllow {
        agent: which.key().into(),
        bot: bot.clone(),
        allow: !off,
    };
    if control::request(&paths.socket_path(), request)?.is_none() {
        // No daemon: the file itself, audited here as the daemon would.
        let mut config = Config::load(paths)?;
        if cww::plans::set(&mut config, which, bot.as_deref(), !off) {
            config.save(paths)?;
            let mut entry = audit::AuditEntry::event(if off {
                "plan_withdrawn"
            } else {
                "plan_allowed"
            });
            entry.detail = Some(format!("{} for {whom}", which.key()));
            audit::AuditLog::open(&paths.audit_file())?.append(&entry);
        }
    }
    let who = match &bot {
        Some(bot) => format!("Bot {bot}"),
        None => "Your chats".into(),
    };
    println!(
        "{who} {} use your {} plan on this computer.",
        if off { "may not" } else { "may" },
        cww::plans::plan_name(which)
    );
    Ok(())
}

fn coding_agent(name: Option<&str>) -> anyhow::Result<cww::coding::Agent> {
    use cww::coding::Agent;
    match name {
        None | Some("claude") | Some("claude_code") | Some("claude-code") => Ok(Agent::ClaudeCode),
        Some(other) => Agent::ALL
            .into_iter()
            .find(|agent| agent.program() == other || agent.key() == other)
            .ok_or_else(|| {
                let names: Vec<&str> = Agent::ALL.iter().map(|a| a.program()).collect();
                anyhow::anyhow!("{other:?} isn't a coding agent: use {}", names.join(", "))
            }),
    }
}

fn ask_kind(claude_code: bool) -> cww::unattended_asks::AskKind {
    if claude_code {
        cww::unattended_asks::AskKind::ClaudeCode
    } else {
        cww::unattended_asks::AskKind::Unattended
    }
}

/// A bot's request, and how to answer it.
fn describe_ask(ask: &Value) -> String {
    let (root, bot) = (str(&ask["root"]), str(&ask["bot"]));
    let name = Some(str(&ask["name"]))
        .filter(|n| !n.trim().is_empty())
        .map_or_else(|| format!("Bot {bot}"), str::to_string);
    if ask["kind"] == "plan" {
        let flag = if bot.is_empty() {
            String::new()
        } else {
            format!(" --bot {bot}")
        };
        let agent = if root == "claude_code" {
            "claude"
        } else {
            root
        };
        let whom = if bot.is_empty() {
            "your chats".to_string()
        } else {
            name.clone()
        };
        return format!(
            "Chat with Work asks to use your {} sign-in on this computer for {whom}; the request \
             waits until {}. `cww plans allow {agent}{flag}` allows it, `cww plans allow \
             {agent}{flag} --off` doesn't.",
            str(&ask["folder"]),
            str(&ask["expires_at"])
        );
    }
    if ask["kind"] == "claude_code" {
        return format!(
            "{name} asks to run Claude Code in {} with this computer's Claude Code sign-in; the \
             request waits until {}. `cww roots allow {root} {bot} --claude-code` allows it, \
             `cww roots decline {root} {bot} --claude-code` doesn't.",
            str(&ask["folder"]),
            str(&ask["expires_at"])
        );
    }
    format!(
        "{name} asks to run commands in {} without asking each time; the request waits \
         until {}. `cww roots allow {root} {bot}` allows it until you change the folder's \
         sharing, `cww roots decline {root} {bot}` doesn't.",
        str(&ask["folder"]),
        str(&ask["expires_at"])
    )
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

/// `cww bots`: each command is one request, printed in words or as the
/// server's JSON, which is the stable shape agents read.
fn bots_command(
    paths: &Paths,
    organization: Option<&str>,
    json: bool,
    command: BotsCommand,
) -> Result<()> {
    let client = Bots::connect(paths, organization)?;
    let print = |answer: Value, render: fn(&Value) -> Vec<String>| -> Result<()> {
        if json {
            println!("{}", serde_json::to_string_pretty(&answer)?);
        } else {
            for line in render(&answer) {
                println!("{line}");
            }
        }
        Ok(())
    };
    match command {
        BotsCommand::List => print(client.list()?, bots::render_list),
        BotsCommand::Show { id } => print(client.show(&id)?, bots::render_bot),
        BotsCommand::Recipes { key: None, .. } => print(client.recipes()?, bots::render_recipes),
        BotsCommand::Recipes {
            key: Some(key),
            tools,
        } => print(client.recipe(&key, &tools)?, bots::render_recipe),
        BotsCommand::Triggers { app: Some(app), .. } => {
            print(client.trigger(&app)?, bots::render_trigger)
        }
        BotsCommand::Triggers { query, app: None } => {
            print(client.triggers(query.as_deref())?, bots::render_triggers)
        }
        BotsCommand::Create(form) => {
            let (form, file) = form.fields.into_form();
            print(
                client.create(&form.body(file.as_deref())?)?,
                bots::render_bot,
            )
        }
        BotsCommand::Update(change) => {
            let id = change.id.clone();
            let file = change.file.clone();
            let change = bots::Change {
                name: change.name,
                description: change.description,
                instructions: change.instructions,
                when: change.when,
                time_zone: change.time_zone,
                add: change.add,
                remove: change.remove,
                notify: change.notify,
                changes: change.changes,
                reply_mode: change.reply_mode,
                cap: change.cap,
                writes: change.writes,
                policies: change.policies,
                allow_destructive: change.yes,
            };
            print(
                client.update(&id, &change.body(file.as_deref())?)?,
                bots::render_bot,
            )
        }
        BotsCommand::Pause { id } => print(client.pause(&id)?, bots::render_bot),
        BotsCommand::Resume { id } => print(client.resume(&id)?, bots::render_bot),
        BotsCommand::Delete { id, yes } => {
            let name = client.show(&id)?["bot"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            bots::confirm(&format!("Delete {name}? Its chats stay."), yes)?;
            print(client.delete(&id)?, |answer| {
                vec![format!(
                    "Deleted {}.",
                    answer["deleted"]["name"].as_str().unwrap_or_default()
                )]
            })
        }
        BotsCommand::Run(run) => bots_run(&client, *run, json),
    }
}

fn bots_run(client: &Bots, run: BotRun, json: bool) -> Result<()> {
    let print = |answer: &Value| -> Result<()> {
        if json {
            println!("{}", serde_json::to_string_pretty(answer)?);
        } else {
            for line in bots::render_trial(answer) {
                println!("{line}");
            }
        }
        Ok(())
    };
    let limit = std::time::Duration::from_secs(run.timeout);
    if let Some(id) = run.follow {
        return print(&client.wait_for_trial(id, limit)?);
    }
    let (form, file) = run.fields.into_form();
    let mut body = match run.id {
        Some(id) => serde_json::json!({ "bot_id": bots::bot_id(&id)? }),
        None => form.body(file.as_deref())?,
    };
    if let Some(sample) = run.sample {
        body["event"] = sample.into();
    }
    // Before it starts, and on stderr, so --json stays one document.
    eprintln!("This try uses your credits, like a chat. It changes nothing outside.");
    let started = client.start_trial(&body)?;
    if run.no_wait {
        return print(&started);
    }
    let id = started["trial"]["id"].as_u64().unwrap_or_default();
    print(&client.wait_for_trial(id, limit)?)
}

/// `cww login --json`: one JSON object per line on stdout. First
/// `{"event":"code",...}` with what to show and open, then `{"event":"paired",...}`
/// or `{"event":"error",...}`. On success the daemon is started as by
/// `cww login` (reloaded if running, else installed unless `start` is
/// false), and `paired` says how that went.
fn login_json(
    paths: &Paths,
    server: &str,
    mut options: auth::LoginOptions,
    start: bool,
    organization: bool,
) -> Result<()> {
    use std::sync::atomic::AtomicBool;

    let emit = |value: Value| println!("{value}");
    // Opened below, after the code is printed.
    let open = std::mem::take(&mut options.open_browser);
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
/// `cww browser ...`: edit `config.toml`, then tell the daemon.
fn browser(paths: &Paths, command: BrowserCommand) -> Result<()> {
    use cww::browsing::{bridge, install, settings};
    let mut config = Config::load(paths)?;
    let register = |config: &Config| -> Result<()> {
        let exe = host_exe()?;
        let done = install::register(paths, &exe, &bridge::origins(&config.browser.extension_ids))?;
        if done.is_empty() {
            println!(
                "No Chrome, Chromium, Edge or Brave found for this user. Install one and run                  `cww browser register`."
            );
        }
        for registered in done {
            println!(
                "Registered with {} ({}).",
                registered.browser, registered.location
            );
        }
        Ok(())
    };
    match command {
        BrowserCommand::Tools => {
            println!("{}", cww::browsing::local::tools(&paths.socket_path())?);
            return Ok(());
        }
        BrowserCommand::Call {
            tool,
            input,
            approve,
        } => {
            let args = cww::browsing::local::arguments(input.as_deref())?;
            let result = cww::browsing::local::call(&paths.socket_path(), tool, args, approve)?;
            println!("{result}");
            if result["isError"] == true {
                anyhow::bail!("browser tool refused or failed; see the JSON result");
            }
            return Ok(());
        }
        BrowserCommand::Status { json } => {
            let live = control::request(&paths.socket_path(), ControlRequest::Browser)
                .ok()
                .flatten();
            let status = live
                .map(|v| v["browser"].clone())
                .unwrap_or_else(|| settings::status(&config.browser, None));
            if json {
                println!("{status}");
                return Ok(());
            }
            println!(
                "Browser: {}{}{}",
                if config.browser.enabled { "on" } else { "off" },
                if config.browser.changes {
                    ", reads and acts"
                } else {
                    ", reads only"
                },
                if config.browser.stopped {
                    ", stopped"
                } else {
                    ""
                }
            );
            match status["extension"].as_object() {
                Some(ext) => println!(
                    "Connected: {} with extension {}{}",
                    ext.get("browser")
                        .and_then(Value::as_str)
                        .unwrap_or("a browser"),
                    ext.get("version").and_then(Value::as_str).unwrap_or("?"),
                    if ext.get("stopped") == Some(&Value::Bool(true)) {
                        " (stopped in the extension)"
                    } else {
                        ""
                    }
                ),
                None => println!("Connected: no browser"),
            }
            if config.browser.all_sites {
                println!("Sites: all except blocked sites");
            } else if config.browser.sites.is_empty() {
                println!("Sites: none yet (`cww browser allow github.com`)");
            } else {
                println!("Sites: {}", config.browser.sites.join(", "));
            }
            if !config.browser.blocked_sites.is_empty() {
                println!("Blocked sites: {}", config.browser.blocked_sites.join(", "));
            }
            for loan in &config.browser.bots {
                println!("Lent to bot {} {}", loan.id, loan.name);
            }
            return Ok(());
        }
        BrowserCommand::Enable => {
            register(&config)?;
            config.browser.enabled = true;
            config.save(paths)?;
            println!(
                "Chat with Work can now use the tabs in the \"Chat with Work\" group of your \
                 browser, on the sites you allow, once the Chat with Work extension is installed."
            );
        }
        BrowserCommand::Disable => {
            install::unregister(paths)?;
            config.browser.enabled = false;
            config.save(paths)?;
            println!("Chat with Work doesn't use your browser now.");
        }
        BrowserCommand::Register => {
            register(&config)?;
            return Ok(());
        }
        BrowserCommand::Allow { site } => {
            let site = settings::allow_site(&mut config, &site)?;
            config.save(paths)?;
            println!("Chat with Work may use {site} in its tab group.");
        }
        BrowserCommand::Disallow { site } => {
            let site = settings::disallow_site(&mut config, &site)?;
            config.save(paths)?;
            println!(
                "{site}: removed from the allowed list. Use block to refuse it in both modes."
            );
        }
        BrowserCommand::Ask | BrowserCommand::AutomaticActions => {
            config.browser.ask_before_actions = matches!(command, BrowserCommand::Ask);
            config.save(paths)?;
            println!(
                "Browser actions: {}.",
                if config.browser.ask_before_actions {
                    "ask before acting"
                } else {
                    "run without repeated approval on permitted sites"
                }
            );
        }
        BrowserCommand::AllowAll | BrowserCommand::AllowListed => {
            config.browser.all_sites = matches!(command, BrowserCommand::AllowAll);
            config.save(paths)?;
            println!(
                "Browser sites: {}.",
                if config.browser.all_sites {
                    "all except blocked sites"
                } else {
                    "only listed sites"
                }
            );
        }
        BrowserCommand::Block { site } => {
            let site = settings::block_site(&mut config, &site, true)?;
            config.save(paths)?;
            println!("{site}: blocked.");
        }
        BrowserCommand::Unblock { site } => {
            let site = settings::block_site(&mut config, &site, false)?;
            config.save(paths)?;
            println!("{site}: block removed.");
        }
        BrowserCommand::AllowChanges => {
            config.browser.changes = true;
            config.save(paths)?;
            println!(
                "Chat with Work can now also open pages, click, type and choose in its tab \
                 group, on the sites you allow. Your Ask before browser actions setting still applies."
            );
        }
        BrowserCommand::DenyChanges => {
            config.browser.changes = false;
            config.save(paths)?;
            println!("Chat with Work only reads pages in your browser now.");
        }
        BrowserCommand::Stop => {
            config.browser.stopped = true;
            config.save(paths)?;
            println!(
                "Chat with Work has stopped using your browser. `cww browser resume` lets it again."
            );
        }
        BrowserCommand::Resume => {
            config.browser.stopped = false;
            config.save(paths)?;
            println!("Chat with Work may use your browser again.");
        }
        BrowserCommand::Lend { bot, name } => {
            settings::set_bot(&mut config, &bot, name.as_deref(), true)?;
            config.save(paths)?;
            println!(
                "Bot {bot}'s chats may use your browser, on the same sites. Each change still \
                 waits for approval in Chat with Work."
            );
        }
        BrowserCommand::StopLending { bot } => {
            settings::set_bot(&mut config, &bot, None, false)?;
            config.save(paths)?;
            println!("Bot {bot} can't use your browser now.");
        }
    }
    notify_daemon(paths, "");
    Ok(())
}

/// The program the browsers start: this `cww`, or on Windows `cww-agent`
/// beside it, which opens no console window.
fn host_exe() -> Result<PathBuf> {
    let exe = service::stable_exe()?;
    if cfg!(windows) {
        let agent = exe.with_file_name("cww-agent.exe");
        if agent.is_file() {
            return Ok(agent);
        }
    }
    Ok(exe)
}

/// Records a change `cww` made to the config itself, with no agent to make
/// it, in the audit log, as the agent records the same change made through
/// it. A log that can't be written is said, not fatal: the change is
/// already saved.
fn audit_here(paths: &Paths, event: &str, detail: String) {
    let mut entry = audit::AuditEntry::event(event);
    entry.detail = Some(detail);
    if let Err(e) = audit::append_from_outside(&paths.audit_file(), &entry) {
        eprintln!("warning: couldn't write the audit log: {e:#}");
    }
}

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
    fn browser_calls_read_json_from_stdin_and_require_explicit_approval() {
        let cli = Cli::try_parse_from(["cww", "browser", "call", "browser_read", "--input", "-"])
            .unwrap();
        let Some(Command::Browser {
            command:
                BrowserCommand::Call {
                    tool,
                    input,
                    approve,
                },
        }) = cli.command
        else {
            panic!("browser call");
        };
        assert_eq!(tool, "browser_read");
        assert_eq!(input, Some(PathBuf::from("-")));
        assert!(!approve);
        let cli =
            Cli::try_parse_from(["cww", "browser", "call", "browser_click", "--approve"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Browser {
                command: BrowserCommand::Call { approve: true, .. }
            })
        ));
    }

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
