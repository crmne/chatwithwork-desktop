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
}

/// The browser: whether Chat with Work may use it, on which sites, and for
/// which bots. Only this file and the control channel change it, never the
/// server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    /// Offer the browser tools. The person's choice, on this computer.
    pub enabled: bool,
    /// Stopped from the tray, `cww browser stop` or the extension: the
    /// tools stay listed and every call is refused until resumed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Sites the extension may open, read and act on: a host (`github.com`,
    /// with its subdomains, over HTTPS) or an origin
    /// (`http://localhost:3000`). See `crate::browsing::sites`.
    pub sites: Vec<String>,
    /// Bots whose chats may use the browser, like folders lent to them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotLoan>,
    /// More extension IDs allowed to connect, besides the published one:
    /// a development build loaded unpacked with another key.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extension_ids: Vec<String>,
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
    /// Let those commands use the network (installing packages, fetching
    /// dependencies). Off by default: commands run without a network.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// Chat with Work bots this folder is lent to (PROTOCOL.md section 7.2.1).
    /// A call a bot's chat makes reaches only the folders lent to that bot,
    /// and a command it runs without a person approving it only where its
    /// loan says so. Only ever changed here or from this computer's apps,
    /// never by the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<BotLoan>,
}

impl Root {
    /// Commands may run here: changes and commands are both allowed.
    pub fn runs_commands(&self) -> bool {
        self.writable && self.commands
    }

    /// The loan of this folder to the bot with this ID, if it's lent to it.
    pub fn loan(&self, bot: &str) -> Option<&BotLoan> {
        self.bots.iter().find(|loan| loan.id == bot)
    }
}

/// A shared folder lent to one Chat with Work bot: the bot's ID on the
/// server, the name it had when it was lent (for the local user only), and
/// whether its commands here may run without a person approving each one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BotLoan {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Commands the bot runs here start without anyone approving them in
    /// Chat with Work. Only where commands are allowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unattended: bool,
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
    /// Tool calls accepted per rolling minute, across all chats.
    pub calls_per_minute: u32,
    /// Maximum characters one `read` call returns.
    pub read_chars_per_call: usize,
    /// Characters `read` may return per chat in a rolling hour.
    pub read_chars_per_chat_hour: usize,
    /// Characters `read` may return across all chats in a rolling hour.
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
    /// rolling minute, across all chats.
    pub changes_per_minute: u32,
    /// Changes accepted per rolling day.
    pub changes_per_day: u32,
    /// Bytes changes may write per rolling hour (new contents and copies).
    pub change_bytes_per_hour: u64,
    /// Largest file a change creates, edits or copies, in bytes.
    pub max_change_file_bytes: u64,
    /// Most files and folders one move or delete of a folder may carry.
    pub max_change_entries: usize,
    /// CPU time each process a command starts may use, in seconds.
    pub command_cpu_seconds: u64,
    /// Memory each process a command starts may allocate, in megabytes.
    pub command_memory_mb: u64,
    /// How long a command may run, in seconds, before it's stopped.
    pub command_timeout_seconds: u64,
    /// Processes a command may have at once, itself included, before it's
    /// stopped. Checked by the daemon on Linux; MXC can't hold it there.
    pub command_processes: u32,
    /// Commands running at once, across all chats.
    pub commands_running: usize,
    /// Characters of each of a command's streams (output and errors) its
    /// result keeps, from the end.
    pub command_output_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            calls_per_minute: 120,
            read_chars_per_call: 32_000,
            read_chars_per_chat_hour: 1_000_000,
            read_chars_per_hour: 2_000_000,
            search_hits: 20,
            snippet_chars: 300,
            list_page: 200,
            max_file_bytes: 64 * 1024 * 1024,
            max_index_file_bytes: 32 * 1024 * 1024,
            max_message_bytes: 256 * 1024,
            grep_timeout_ms: 5_000,
            changes_per_minute: 30,
            changes_per_day: 500,
            change_bytes_per_hour: 50 * 1024 * 1024,
            max_change_file_bytes: 10 * 1024 * 1024,
            max_change_entries: 1_000,
            command_cpu_seconds: 600,
            command_memory_mb: 4_096,
            command_timeout_seconds: 1_800,
            command_processes: 256,
            commands_running: 4,
            command_output_chars: 20_000,
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
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
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

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("allow_everything = true").is_err());
    }
}
