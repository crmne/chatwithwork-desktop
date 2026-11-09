//! The local configuration file (`~/.config/cww/config.toml`).
//!
//! Everything that decides what the server may see lives here, on the user's
//! machine. The server can't change any of it.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::{Paths, write_private_file};

pub const DEFAULT_SERVER: &str = "https://chatwithwork.com";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The paired server. Absent until `cww login` succeeds.
    pub server: Option<ServerConfig>,
    /// Where secrets are kept: `keyring` (OS keychain) or `file` (0600 file).
    pub secret_store: Option<String>,
    /// Pause answering tool calls. Survives restarts.
    pub paused: bool,
    /// HTTP proxy to reach the server through, such as
    /// `http://user:password@proxy.example:3128`, or `none` for a direct
    /// connection. Overrides `HTTPS_PROXY` and the other proxy variables,
    /// which services don't inherit from a shell. See `crate::proxy`.
    pub proxy: Option<String>,
    /// Send a report to the Chat with Work team when the background agent
    /// crashes: where in its code, its version and the operating system,
    /// never files, chats, names or tokens. Off until set; the desktop app
    /// and `cww tui` ask once. `CWW_CRASH_REPORTS=1` or `0` overrides it.
    /// See `crate::crash_reports`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash_reports: Option<bool>,
    /// Folders shared with the server. Nothing outside them is readable.
    pub roots: Vec<Root>,
    /// Printers Chat with Work may use (docs/printing.md). None until the
    /// person adds one here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub printers: Vec<Printer>,
    pub deny: DenyConfig,
    pub limits: Limits,
    pub index: IndexConfig,
    pub sandbox: SandboxConfig,
    pub tui: TuiConfig,
    #[serde(skip_serializing_if = "DesktopConfig::is_default")]
    pub desktop: DesktopConfig,
    /// Chat with Work in the person's own browser, through the extension
    /// (docs/browser.md). Off until they turn it on here.
    #[serde(skip_serializing_if = "BrowserConfig::is_default")]
    pub browser: BrowserConfig,
    /// What this computer has for coding bots: the Claude Code program a
    /// bot's commands use, whether its sign-in is stored, and the
    /// repositories Desktop mirrors (docs/coding-computers.md). Only `cww
    /// coding`, this file and the control channel change it, never the
    /// server.
    #[serde(skip_serializing_if = "CodingConfig::is_default")]
    pub coding: CodingConfig,
    /// Shared folders, by ID, whose environment is in this computer's
    /// secret store (`cww roots env`, docs/secrets.md). Only their names:
    /// the lists themselves are never in this file.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folder_env: Vec<String>,
}

/// Coding bots' setup on this computer (docs/coding-computers.md).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CodingConfig {
    /// The `claude` program `cww coding setup` found and checked, by the
    /// path it was found at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<PathBuf>,
    /// The `codex` program, the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<PathBuf>,
    /// The `opencode` program, the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<PathBuf>,
    /// The `gemini` program, the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<PathBuf>,
    /// The `pi` program, the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi: Option<PathBuf>,
    /// What 0.13.1 and earlier wrote when they kept a token from `claude
    /// setup-token` in the secret store. Kept, so any save leaves it, until
    /// the next `cww coding` deletes that token
    /// ([`crate::coding::forget_legacy_token`]): runs now use Claude Code's
    /// own sign-in.
    #[serde(
        rename = "claude_signed_in",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub legacy_token: bool,
    /// Repositories Desktop keeps a mirror of, outside every shared folder,
    /// each with a checkout in one that fetches from it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<Repository>,
}

impl CodingConfig {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A repository a coding bot works on: its address, and its checkout,
/// `name` in the shared folder `root`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub root: String,
    pub name: String,
    pub url: String,
    /// A deploy key Desktop fetches the mirror with, instead of what `ssh`
    /// would offer: a private key outside every shared folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<PathBuf>,
    /// `owner/name` on GitHub, for a repository a bot's run named and this
    /// computer prepared by itself (`crate::coding::prepare`). A bot's
    /// `roots` names only these by it: the server knows them already. Its
    /// `url` is what this computer's Git login reached, never the server's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    /// Whether Desktop made this checkout, cloning it from the mirror in the
    /// sandbox, rather than keeping one that was there already. Only one it
    /// made is trusted for Claude Code and Codex (`crate::coding::trust`).
    /// Not recorded before 0.16.1: then the checkout's first reflog entry
    /// says whether it was cloned from the mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloned: Option<bool>,
}

/// The browser: whether Chat with Work may use it, on which sites, and for
/// which bots. Only this file and the control channel change it, never the
/// server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    /// Offer the browser tools. The person's choice, on this computer.
    pub enabled: bool,
    /// Also offer the tools that act on pages: open, click, type, choose,
    /// close, and save files into a folder that allows changes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub changes: bool,
    /// Ask before browser actions. Only an explicit local opt-out changes this.
    pub ask_before_actions: bool,
    /// Stopped from the tray, `cww browser stop` or the extension: the
    /// tools stay listed and every call is refused until resumed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Sites the extension may open, read and act on: a host (`github.com`,
    /// with its subdomains, over HTTPS) or an origin
    /// (`http://localhost:3000`). See `crate::browsing::sites`.
    pub sites: Vec<String>,
    /// Use any HTTP(S) site except blocked_sites. Only an explicit local
    /// choice enables this; existing configurations stay allow-listed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub all_sites: bool,
    /// Always refused, in either mode. Bare hosts block every HTTP(S)
    /// port and subdomain; origins block exactly their scheme/host/port.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_sites: Vec<String>,
    /// Bots whose chats may use the browser, like folders lent to them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotLoan>,
    /// More extension IDs allowed to connect, besides the published one:
    /// a development build loaded unpacked with another key.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extension_ids: Vec<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            changes: false,
            ask_before_actions: true,
            stopped: false,
            sites: Vec::new(),
            all_sites: false,
            blocked_sites: Vec::new(),
            bots: Vec::new(),
            extension_ids: Vec::new(),
        }
    }
}

impl BrowserConfig {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn loan(&self, bot: &str) -> Option<&BotLoan> {
        self.bots.iter().find(|loan| loan.id == bot)
    }
}

/// The desktop app's own settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopConfig {
    /// Reduce motion (no fading in of streamed text): `true` or `false`
    /// over the system's accessibility setting, which applies when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_motion: Option<bool>,
}

impl DesktopConfig {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `cww tui`'s own settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Clicks and the wheel. Off leaves the mouse to the terminal, for
    /// selecting text without holding Shift.
    pub mouse: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self { mouse: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    /// Confine the daemon with Landlock (Linux) or Seatbelt (macOS), so the
    /// kernel refuses files outside the shared folders even if cww had a bug.
    pub enabled: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Origin of the Chat with Work server, e.g. `https://chatwithwork.com`.
    pub url: String,
    /// Device ID issued by the server at pairing.
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Root {
    /// Short, stable ID used in tool paths (`<id>:relative/path`).
    pub id: String,
    /// Human label shown to the server and the model.
    pub label: String,
    /// Absolute, canonical path. Never sent to the server.
    pub path: PathBuf,
    /// Follow symlinks that stay inside the root. Off by default. Links that
    /// leave the root are refused either way. Changes never follow them.
    #[serde(default)]
    pub follow_symlinks: bool,
    /// Allow changes: creating, editing, moving and deleting files here,
    /// each approved in Chat with Work, with old versions kept in the
    /// system trash. Off by default, and only ever turned on here or from
    /// this computer's apps, never by the server.
    #[serde(default)]
    pub writable: bool,
    /// Allow running commands here (`run`), each approved in Chat with
    /// Work, in a sandbox of its own that can change only this folder. Only
    /// where changes are allowed, and only ever turned on here or from this
    /// computer's apps, never by the server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub commands: bool,
    /// Let those commands reach the internet and the local network
    /// (installing packages, fetching dependencies), and this computer's own
    /// services with them. Off by default ("Internet" in the app).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// Let those commands reach this computer's own services: its loopback,
    /// its own addresses, and its containers' networks (`docker0` and the
    /// like), never the local network or the internet, and never the ports
    /// that would let a command out of its sandbox (`reach.rs`). On by
    /// default, so a folder that allowed commands before 0.16.0 gets it.
    #[serde(
        default = "local_services_default",
        skip_serializing_if = "Clone::clone"
    )]
    pub local_services: bool,
    /// Exact local-network addresses (`ip:port`) its commands may reach
    /// besides, from "Always allow" on one. Only ever changed here or from
    /// this computer's apps; never sent to the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reach: Vec<String>,
    /// Chat with Work bots this folder is lent to (PROTOCOL.md section 7.2.1).
    /// A call a bot's chat makes reaches only the folders lent to that bot,
    /// and a command it runs without a person approving it only where its
    /// loan says so. Only ever changed here or from this computer's apps,
    /// never by the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotLoan>,
    /// Characters a chat may read from this folder per rolling hour, in
    /// place of the hourly read limits in `[limits]`; all chats together may
    /// read twice it. It raises a limit, so it counts only while one of
    /// those is set: with none, reading has no hourly limit and neither does
    /// this folder. Between `read_chars_per_chat_hour` and
    /// [`MAX_READ_BUDGET`]; only ever changed here or from this computer's
    /// apps, never by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_budget: Option<usize>,
}

/// The largest read budget a folder may have: characters per chat per
/// rolling hour.
pub const MAX_READ_BUDGET: usize = 20_000_000;

fn local_services_default() -> bool {
    true
}

impl Root {
    /// This folder's own read budget, if it has one and an hourly read
    /// limit is set, within its bounds: at least the per-chat budget every
    /// folder has (`limits`), at most [`MAX_READ_BUDGET`]. Without a limit
    /// to raise, a budget set for one would only cap this folder alone.
    pub fn read_budget(&self, limits: &Limits) -> Option<usize> {
        if !limits.reads_capped() {
            return None;
        }
        self.read_budget
            .map(|budget| budget.clamp(limits.least_read_budget(), MAX_READ_BUDGET))
    }

    /// Commands may run here: changes and commands are both allowed.
    pub fn runs_commands(&self) -> bool {
        self.writable && self.commands
    }

    /// How far its commands reach, by its settings alone (the sandbox this
    /// computer gets may hold less: `commands::network_access`).
    pub fn network_level(&self) -> crate::reach::Level {
        if !self.runs_commands() {
            return crate::reach::Level::None;
        }
        crate::reach::Level::of(self.local_services, self.network)
    }

    /// The loan of this folder to the bot with this ID, if it's lent to it.
    pub fn loan(&self, bot: &str) -> Option<&BotLoan> {
        self.bots.iter().find(|loan| loan.id == bot)
    }
}

/// A printer of this computer's system that Chat with Work may use. Only
/// ever changed here or from this computer's apps, never by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Printer {
    /// Short, stable ID used in tool calls.
    pub id: String,
    /// The system's name for it, as `lp -d` or the spooler takes it. Never
    /// sent to the server.
    pub queue: String,
    /// Name shown to the server and the model.
    pub label: String,
    /// The owner's chats may print here, each job approved in Chat with Work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chats: bool,
    /// Bots this printer is lent to. Each job asks, unless the loan is
    /// `unattended`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotLoan>,
}

impl Printer {
    /// The loan of this printer to the bot with this ID, if it has one.
    pub fn loan(&self, bot: &str) -> Option<&BotLoan> {
        self.bots.iter().find(|loan| loan.id == bot)
    }
}

/// A shared folder lent to one Chat with Work bot: the bot's ID on the
/// server, the name it had when it was lent (for the local user only), and
/// whether its commands here may run without a person approving each one.
///
/// Unknown keys are let through, unlike the rest of the file, for the
/// `claude_code` key 0.13.1 and earlier wrote: that yes gave a run one token,
/// not Claude Code's files, so it's dropped and never read as this one's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotLoan {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Commands the bot runs here start without anyone approving them in
    /// Chat with Work. Only where commands are allowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unattended: bool,
    /// The bot's commands here use this computer's own Claude Code sign-in:
    /// the person's yes to what that gives them (docs/coding-computers.md),
    /// `claude_login` in the file. Only for folders, only where commands are
    /// allowed.
    #[serde(
        default,
        rename = "claude_login",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub claude_code: bool,
    /// The same for Codex: `~/.codex`, `codex_login` in the file.
    #[serde(
        default,
        rename = "codex_login",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub codex: bool,
    /// The same for OpenCode: `~/.local/share/opencode`, `opencode_login`
    /// in the file.
    #[serde(
        default,
        rename = "opencode_login",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub opencode: bool,
    /// The same for Gemini CLI: `~/.gemini`, `gemini_login` in the file.
    #[serde(
        default,
        rename = "gemini_login",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub gemini: bool,
    /// The same for pi: `~/.pi/agent`, `pi_login` in the file.
    #[serde(
        default,
        rename = "pi_login",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub pi: bool,
    /// The GitHub owners whose repositories this bot's first runs may mirror
    /// and prepare here by themselves, with the person's Git login: the
    /// person's yes, on this computer, to what the server can name. Nothing
    /// else is ever fetched for a server's word.
    #[serde(
        default,
        rename = "repository_owners",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DenyConfig {
    /// Extra deny patterns, same syntax as the built-in list.
    pub extra: Vec<String>,
    /// Built-in patterns to drop. Only editable here, never from the server.
    pub remove: Vec<String>,
    /// Serve files with more than one hard link. Off by default, because a
    /// hard link inside a root can point at a file that lives outside it.
    pub allow_hardlinks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Tool calls per rolling minute, across all chats. Zero is no limit,
    /// the default.
    pub calls_per_minute: u32,
    /// Maximum characters one `read` call returns.
    pub read_chars_per_call: usize,
    /// Characters reads (`read`, `grep`, `glob`, `browser_read`, the diffs
    /// of dry runs) may return per chat in a rolling hour, from folders
    /// without a `read_budget` of their own. Zero is no limit, the default.
    pub read_chars_per_chat_hour: usize,
    /// Characters those reads may return across all chats in a rolling
    /// hour. Zero is no limit, the default.
    pub read_chars_per_hour: usize,
    /// Maximum search hits per call.
    pub search_hits: usize,
    /// Maximum characters per search snippet.
    pub snippet_chars: usize,
    /// Maximum entries per `list` page.
    pub list_page: usize,
    /// Largest file `read` will open, in bytes.
    pub max_file_bytes: u64,
    /// Largest file the indexer will extract, in bytes.
    pub max_index_file_bytes: u64,
    /// Largest WebSocket message accepted or sent, in bytes.
    pub max_message_bytes: usize,
    /// Time budget for the live grep fallback, in milliseconds.
    pub grep_timeout_ms: u64,
    /// Changes (create, write, edit, mkdir, move, delete) accepted per
    /// rolling minute, across all chats. Zero is no limit, the default.
    pub changes_per_minute: u32,
    /// Changes accepted per rolling day. Zero is no limit, the default.
    pub changes_per_day: u32,
    /// Bytes changes may write per rolling hour. Zero is no limit, the
    /// default.
    pub change_bytes_per_hour: u64,
    /// Largest file a change creates, edits or copies, in bytes.
    pub max_change_file_bytes: u64,
    /// Most files and folders one move or delete of a folder may carry.
    pub max_change_entries: usize,
    /// CPU time each process a command starts may use, in seconds, all its
    /// threads together. Zero is no limit, the default: a build or a test
    /// suite on many cores uses ten minutes of it in one, and the wall
    /// clock (`command_timeout_seconds`) still ends a run.
    pub command_cpu_seconds: u64,
    /// Memory a command may use, in megabytes: on Linux everything it
    /// starts together, on Windows each process. Half this computer's
    /// memory by default, at least 4 GB ([`default_command_memory_mb`]).
    pub command_memory_mb: u64,
    /// How long a command may run, in seconds, before it's stopped.
    pub command_timeout_seconds: u64,
    /// Processes a command may have at once, itself included, before it's
    /// stopped. Checked by the daemon on Linux; MXC can't hold it there.
    pub command_processes: u32,
    /// Commands running at once, across all chats. Zero is no limit; the
    /// default is this computer's CPU count ([`default_commands_running`]).
    pub commands_running: usize,
    /// Characters of each of a command's streams (output and errors) its
    /// result keeps, from the end.
    pub command_output_chars: usize,
    /// Bytes `read_bytes` may return per rolling hour, across all chats:
    /// files copied into a chat's files, apart from the read budgets. Zero
    /// is no limit, the default.
    pub copy_bytes_per_hour: u64,
    /// Pages all printers on this computer may print per rolling day,
    /// copies counted. Zero is unlimited.
    pub print_pages_per_day: u32,
}

impl Limits {
    /// Whether an hourly read limit is set.
    pub fn reads_capped(&self) -> bool {
        self.read_chars_per_chat_hour > 0 || self.read_chars_per_hour > 0
    }

    /// The smallest read budget a folder may have.
    pub fn least_read_budget(&self) -> usize {
        self.read_chars_per_chat_hour.clamp(1, MAX_READ_BUDGET)
    }

    /// Older files contain every old default because `Config::save` writes
    /// them out whole. Move those exact values, from every release, to the
    /// current defaults, which set no time-based limit; preserve other
    /// local values.
    pub fn raise_old_defaults(&mut self) {
        let new = Self::default();
        if matches!(self.calls_per_minute, 120 | 240) {
            self.calls_per_minute = new.calls_per_minute;
        }
        if self.changes_per_minute == 30 {
            self.changes_per_minute = new.changes_per_minute;
        }
        if self.changes_per_day == 500 {
            self.changes_per_day = new.changes_per_day;
        }
        if self.change_bytes_per_hour == 50 * 1024 * 1024 {
            self.change_bytes_per_hour = new.change_bytes_per_hour;
        }
        if matches!(
            self.read_chars_per_chat_hour,
            256_000 | 1_000_000 | 5_000_000
        ) {
            self.read_chars_per_chat_hour = new.read_chars_per_chat_hour;
        }
        if matches!(self.read_chars_per_hour, 2_000_000 | 10_000_000) {
            self.read_chars_per_hour = new.read_chars_per_hour;
        }
        if self.copy_bytes_per_hour == 100 * 1024 * 1024 {
            self.copy_bytes_per_hour = new.copy_bytes_per_hour;
        }
        if self.commands_running == 4 {
            self.commands_running = new.commands_running;
        }
        if self.command_cpu_seconds == 600 {
            self.command_cpu_seconds = new.command_cpu_seconds;
        }
        if self.command_memory_mb == 4_096 {
            self.command_memory_mb = new.command_memory_mb;
        }
    }
}

/// Memory a command may use by default: half this computer's, and at least
/// 4 GB, read once. A confined daemon can't read it, so [`crate::sandbox`]
/// reads it before confining.
pub fn default_command_memory_mb() -> u64 {
    static DEFAULT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *DEFAULT.get_or_init(|| half_of(crate::dictation::memory_bytes()))
}

/// Half of `bytes` of memory, in megabytes, and never under 4 GB.
fn half_of(bytes: Option<u64>) -> u64 {
    bytes.map_or(4_096, |bytes| (bytes / 2 / (1024 * 1024)).max(4_096))
}

/// Commands that may run at once by default: one per CPU this computer
/// has, or 4 where it can't say.
pub fn default_commands_running() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            calls_per_minute: 0,
            read_chars_per_call: 32_000,
            read_chars_per_chat_hour: 0,
            read_chars_per_hour: 0,
            search_hits: 20,
            snippet_chars: 300,
            list_page: 200,
            max_file_bytes: 64 * 1024 * 1024,
            max_index_file_bytes: 32 * 1024 * 1024,
            max_message_bytes: 256 * 1024,
            grep_timeout_ms: 5_000,
            changes_per_minute: 0,
            changes_per_day: 0,
            change_bytes_per_hour: 0,
            max_change_file_bytes: 10 * 1024 * 1024,
            max_change_entries: 1_000,
            command_cpu_seconds: 0,
            command_memory_mb: default_command_memory_mb(),
            command_timeout_seconds: 1_800,
            command_processes: 256,
            commands_running: default_commands_running(),
            command_output_chars: 20_000,
            copy_bytes_per_hour: 0,
            print_pages_per_day: 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    /// Build and use the full-text index. Search falls back to live grep
    /// without it.
    pub enabled: bool,
    /// Watch roots for changes. Without it, roots are rescanned periodically.
    pub watch: bool,
    /// Full rescan interval in seconds, to catch events the watcher missed.
    pub rescan_secs: u64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            watch: true,
            rescan_secs: 30 * 60,
        }
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.config_file();
        match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map(|mut config| {
                    config.limits.raise_old_defaults();
                    config
                })
                .with_context(|| format!("parsing {}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    /// Load, change and save the file while holding a lock every process
    /// that edits it this way takes, so two edits never save over each other:
    /// the daemon's, and a repository a run prepared (`crate::coding::prepare`).
    pub fn edit<T>(paths: &Paths, edit: impl FnOnce(&mut Config) -> Result<T>) -> Result<T> {
        let lock_path = paths.config_file().with_extension("toml.lock");
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .context("opening the settings' lock")?;
        lock.lock().context("waiting for the settings' lock")?;
        let mut config = Config::load(paths)?;
        let edited = edit(&mut config)?;
        config.save(paths)?;
        Ok(edited)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing config")?;
        let header = "# Chat with Work Desktop (cww) configuration.\n\
                      # The server can't change this file. See README.md for every option.\n\n";
        write_private_file(&paths.config_file(), format!("{header}{text}").as_bytes())
    }

    pub fn root(&self, id: &str) -> Option<&Root> {
        self.roots.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        assert_eq!(Config::load(&paths).unwrap(), Config::default());

        let mut config = Config::default();
        config.roots.push(Root {
            bots: Vec::new(),
            id: "abc123".into(),
            label: "Docs".into(),
            path: "/tmp/docs".into(),
            follow_symlinks: false,
            writable: true,
            commands: false,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: None,
        });
        config.deny.extra.push("*.secret".into());
        config.proxy = Some("http://proxy.example:3128".into());
        config.save(&paths).unwrap();
        assert_eq!(Config::load(&paths).unwrap(), config);
    }

    #[test]
    fn folders_are_read_only_unless_they_say_otherwise() {
        let config: Config =
            toml::from_str("[[roots]]\nid = \"docs\"\nlabel = \"Docs\"\npath = \"/tmp/docs\"\n")
                .unwrap();
        assert!(!config.roots[0].writable);
    }

    /// A yes from 0.13.1 and earlier covered a token, not Claude Code's own
    /// files: it never carries over, and the person is asked again.
    #[test]
    fn an_old_yes_to_a_claude_code_token_isnt_a_yes_to_its_files() {
        let loan = |key: &str| -> BotLoan {
            toml::from_str(&format!("id = \"42\"\n{key} = true\n")).unwrap()
        };
        assert!(!loan("claude_code").claude_code);
        assert!(loan("claude_login").claude_code);
        let saved = toml::to_string(&loan("claude_login")).unwrap();
        assert!(saved.contains("claude_login = true"), "{saved}");
        assert!(!saved.contains("claude_code"), "{saved}");
        let dropped = toml::to_string(&loan("claude_code")).unwrap();
        assert!(!dropped.contains("claude"), "{dropped}");
    }

    /// 0.13.1 and earlier ran at most 4 commands at once and held each
    /// process to 600 CPU seconds: a config that still says exactly those
    /// moves to the new defaults, the CPU count and no CPU limit.
    #[test]
    fn old_command_limits_move_up_and_custom_ones_stay() {
        let mut limits = Limits {
            commands_running: 4,
            command_cpu_seconds: 600,
            ..Limits::default()
        };
        limits.raise_old_defaults();
        assert_eq!(limits.commands_running, default_commands_running());
        assert_eq!(limits.command_cpu_seconds, 0);
        let mut custom = Limits {
            commands_running: 2,
            command_cpu_seconds: 120,
            command_memory_mb: 6_000,
            ..Limits::default()
        };
        custom.raise_old_defaults();
        assert_eq!(custom.commands_running, 2);
        assert_eq!(custom.command_cpu_seconds, 120);
        assert_eq!(custom.command_memory_mb, 6_000);
        assert!(Limits::default().commands_running >= 1);
    }

    /// 0.15.0 and earlier held each command to 4 GB: a config that still
    /// says exactly that moves to half this computer's memory.
    #[test]
    fn the_old_memory_limit_moves_to_half_this_computers_memory() {
        let mut limits = Limits {
            command_memory_mb: 4_096,
            ..Limits::default()
        };
        limits.raise_old_defaults();
        assert_eq!(limits.command_memory_mb, default_command_memory_mb());
        assert!(default_command_memory_mb() >= 4_096);
        let gib = 1024 * 1024 * 1024;
        assert_eq!(half_of(Some(64 * gib)), 32_768);
        assert_eq!(half_of(Some(16 * gib)), 8_192);
        assert_eq!(half_of(Some(4 * gib)), 4_096);
        assert_eq!(half_of(None), 4_096);
    }

    /// 0.13.1 and earlier noted a stored Claude Code token: such a config
    /// still loads, and the note goes only once the token is deleted.
    #[test]
    fn an_old_claude_code_token_note_loads_and_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            paths.config_file(),
            "[coding]\nclaude = \"/usr/bin/claude\"\nclaude_signed_in = true\n",
        )
        .unwrap();
        let mut config = Config::load(&paths).unwrap();
        assert!(config.coding.legacy_token);
        config.coding.legacy_token = false;
        config.save(&paths).unwrap();
        let text = std::fs::read_to_string(paths.config_file()).unwrap();
        assert!(!text.contains("claude_signed_in"), "{text}");
        assert!(text.contains("/usr/bin/claude"), "{text}");

        // Another save before then keeps it, so the token still goes.
        std::fs::write(paths.config_file(), "[coding]\nclaude_signed_in = true\n").unwrap();
        Config::load(&paths).unwrap().save(&paths).unwrap();
        assert!(Config::load(&paths).unwrap().coding.legacy_token);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("allow_everything = true").is_err());
    }

    #[test]
    fn old_default_limits_move_up_and_custom_ones_stay() {
        let mut limits = Limits {
            calls_per_minute: 120,
            read_chars_per_chat_hour: 1_000_000,
            read_chars_per_hour: 3_000_000,
            copy_bytes_per_hour: 7,
            ..Limits::default()
        };
        limits.raise_old_defaults();
        assert_eq!(limits.calls_per_minute, 0);
        assert_eq!(limits.read_chars_per_chat_hour, 0);
        assert_eq!(limits.read_chars_per_hour, 3_000_000);
        assert_eq!(limits.copy_bytes_per_hour, 7);
    }

    /// Every release wrote its defaults into the file with the first save,
    /// so each has to lose them, the first release's small one included.
    #[test]
    fn every_former_default_read_limit_is_lifted_from_a_saved_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        for (chat, all) in [
            (256_000, 2_000_000),
            (1_000_000, 2_000_000),
            (5_000_000, 10_000_000),
            (256_000, 10_000_000),
        ] {
            let saved = Config {
                limits: Limits {
                    read_chars_per_chat_hour: chat,
                    read_chars_per_hour: all,
                    copy_bytes_per_hour: 100 * 1024 * 1024,
                    ..Limits::default()
                },
                ..Config::default()
            };
            saved.save(&paths).unwrap();
            let text = std::fs::read_to_string(paths.config_file()).unwrap();
            assert!(
                text.contains(&format!("read_chars_per_chat_hour = {chat}")),
                "the file spells them out: {text}"
            );
            assert_eq!(Config::load(&paths).unwrap().limits, Limits::default());
        }
    }

    #[test]
    fn a_folders_budget_counts_only_beside_an_hourly_limit() {
        let root = Root {
            bots: Vec::new(),
            id: "code".into(),
            label: "Code".into(),
            path: "/tmp/code".into(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: Some(10_000_000),
        };
        assert_eq!(root.read_budget(&Limits::default()), None);
        for capped in [
            Limits {
                read_chars_per_chat_hour: 1_000,
                ..Limits::default()
            },
            Limits {
                read_chars_per_hour: 1_000,
                ..Limits::default()
            },
        ] {
            assert_eq!(root.read_budget(&capped), Some(10_000_000));
        }
    }

    #[test]
    fn existing_installs_drop_the_former_default_change_quotas() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        for calls in [120, 240] {
            let config = Config {
                limits: Limits {
                    calls_per_minute: calls,
                    changes_per_minute: 30,
                    changes_per_day: 500,
                    change_bytes_per_hour: 50 * 1024 * 1024,
                    ..Limits::default()
                },
                ..Config::default()
            };
            config.save(&paths).unwrap();
            let loaded = Config::load(&paths).unwrap();
            assert_eq!(loaded.limits, Limits::default());
            loaded.save(&paths).unwrap();
            assert_eq!(Config::load(&paths).unwrap(), loaded);
        }
        let custom = Config {
            limits: Limits {
                calls_per_minute: 7,
                changes_per_minute: 3,
                changes_per_day: 20,
                change_bytes_per_hour: 100,
                ..Limits::default()
            },
            ..Config::default()
        };
        custom.save(&paths).unwrap();
        assert_eq!(Config::load(&paths).unwrap(), custom);
    }
}
