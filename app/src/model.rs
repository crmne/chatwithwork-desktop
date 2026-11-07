//! What the daemon reports, as the app reads it (CONTROL.md), and what the
//! tray shows for it.
//!
//! Fields are lenient: a newer or older daemon that adds or drops a field
//! still parses.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub connection: ConnectionStatus,
    pub server: Option<String>,
    pub device_id: Option<String>,
    pub paused: bool,
    pub roots: Vec<RootStatus>,
    /// The printers Chat with Work may use (`printers_add`).
    pub printers: Vec<PrinterStatus>,
    /// The latest tool call. Filled in by the app from the audit log and
    /// its events; the daemon's status doesn't carry it.
    pub last_access: Option<AuditEntry>,
    /// Counts audit events, so every new entry is a change the window sees.
    #[serde(skip_deserializing)]
    pub activity: u64,
    pub config_file: Option<PathBuf>,
    pub audit_file: Option<PathBuf>,
    /// The browser settings, and the browser attached (docs/browser.md).
    pub browser: BrowserStatus,
    /// Whether commands can run here: only on Windows, where it depends on
    /// the version of Windows (docs/commands.md); `None` elsewhere.
    pub commands: Option<CommandsHost>,
    /// Bots asking, about a folder lent to them, to run their commands
    /// unattended or to run Claude Code, waiting for the person here
    /// (CONTROL.md, `status`).
    pub unattended_asks: Vec<UnattendedAsk>,
}

/// A bot's request for unattended commands, named from this computer's own
/// settings: the folder's label and the loan's name for the bot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(default)]
pub struct UnattendedAsk {
    pub root: String,
    pub folder: String,
    pub bot: String,
    /// What it asks for: unattended commands (the default), or Claude
    /// Code with this computer's sign-in.
    pub kind: cww::unattended_asks::AskKind,
    pub name: String,
    pub asked_at: String,
    pub expires_at: String,
}

fn browser_asks_by_default() -> bool {
    true
}

/// What the daemon says of commands on Windows (CONTROL.md, `status`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CommandsHost {
    pub available: bool,
    /// Why not, when they can't.
    pub detail: Option<String>,
    /// The drives commands may need to see the top of, and whether they
    /// can.
    pub drives: Vec<DriveRoot>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct DriveRoot {
    pub root: PathBuf,
    pub prepared: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserStatus {
    pub enabled: bool,
    pub stopped: bool,
    pub changes: bool,
    #[serde(default = "browser_asks_by_default")]
    pub ask_before_actions: bool,
    pub sites: Vec<String>,
    pub all_sites: bool,
    pub blocked_sites: Vec<String>,
    pub bots: Vec<serde_json::Value>,
    pub connected: bool,
    /// The attached browser: `browser`, `version`, `stopped`.
    pub extension: Option<serde_json::Value>,
}

impl Default for BrowserStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            stopped: false,
            changes: false,
            ask_before_actions: true,
            sites: Vec::new(),
            all_sites: false,
            blocked_sites: Vec::new(),
            bots: Vec::new(),
            connected: false,
            extension: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ConnectionStatus {
    /// `not_paired`, `connecting`, `connected`, `offline` or `revoked`.
    pub connection: String,
    pub since: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RootStatus {
    pub id: String,
    pub label: String,
    pub available: bool,
    /// `pending`, `indexing`, `ready`, `error` or `disabled`.
    pub index: String,
    pub indexed_files: Option<u64>,
    pub local_path: Option<PathBuf>,
    /// The user allowed changes in it.
    pub writable: bool,
    /// The user allowed commands in it (`run`).
    pub allows_commands: bool,
    /// Those commands may reach the internet (and the local network).
    pub network: bool,
    /// How far they reach: `none`, `local` (this computer's own services) or
    /// `internet`, while commands are allowed.
    pub network_access: Option<String>,
    /// Exact local-network addresses (`ip:port`) they may reach besides.
    pub reach: Vec<String>,
    /// The Chat with Work bots it is lent to (`roots_bot`).
    pub bots: Vec<BotLoan>,
}

impl RootStatus {
    /// Whether its commands reach this computer's own services.
    pub fn local_services(&self) -> bool {
        matches!(self.network_access.as_deref(), Some("local" | "internet"))
    }
}

/// A printer Chat with Work may use: for the owner's chats, for the bots it's
/// lent to, with `unattended` those print without anyone approving a job.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PrinterStatus {
    pub id: String,
    pub queue: String,
    pub label: String,
    pub chats: bool,
    pub bots: Vec<BotLoan>,
}

/// A folder lent to a bot: its chats reach the folder as it allows, and
/// with `unattended` its commands there start without anyone approving them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BotLoan {
    pub id: String,
    pub name: String,
    pub unattended: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AuditEntry {
    pub ts: String,
    pub event: String,
    pub chat_id: Option<String>,
    pub tool: Option<String>,
    pub path: Option<String>,
    pub query: Option<String>,
    /// `allowed`, `denied` or `error`.
    pub decision: Option<String>,
    pub code: Option<String>,
    pub reason: Option<String>,
    pub bytes: Option<u64>,
    pub results: Option<u64>,
    pub detail: Option<String>,
    /// Where a move went.
    pub to: Option<String>,
    /// What a change did (`created`, `replaced`, `moved`, `trashed`, ...).
    pub effect: Option<String>,
    /// The call only checked a change.
    pub dry_run: Option<bool>,
    /// Bytes a change wrote.
    pub written: Option<u64>,
    /// Where the old version went in the trash, on this computer.
    pub trash: Option<String>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DenyList {
    pub builtin: Vec<String>,
    pub extra: Vec<String>,
    pub removed: Vec<String>,
    pub own_dirs: Vec<PathBuf>,
    pub allow_hardlinks: bool,
    pub config_file: Option<PathBuf>,
}

impl Status {
    pub fn is_paired(&self) -> bool {
        self.server.is_some()
    }

    /// Roots whose first index pass hasn't finished.
    pub fn indexing(&self) -> usize {
        self.roots
            .iter()
            .filter(|r| r.available && matches!(r.index.as_str(), "pending" | "indexing"))
            .count()
    }

    /// The server's host, for "Connected to chatwithwork.com".
    #[cfg(test)]
    pub fn server_host(&self) -> Option<String> {
        self.server.as_deref().map(host_of)
    }
}

#[cfg(test)]
pub fn host_of(origin: &str) -> String {
    origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string()
}

/// The overall state, which picks the tray icon and the first menu line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// The daemon isn't running.
    Stopped,
    NotPaired,
    Paused,
    Connecting,
    Online,
    Offline,
    Revoked,
}

impl Health {
    pub fn of(status: Option<&Status>) -> Self {
        let Some(status) = status else {
            return Self::Stopped;
        };
        if !status.is_paired() {
            return Self::NotPaired;
        }
        if status.paused {
            return Self::Paused;
        }
        match status.connection.connection.as_str() {
            "connected" => Self::Online,
            "connecting" => Self::Connecting,
            "revoked" => Self::Revoked,
            "not_paired" => Self::NotPaired,
            _ => Self::Offline,
        }
    }

    /// The status dot's meaning in the settings pages (`.state--<tone>`).
    pub fn tone(self) -> &'static str {
        match self {
            Self::Online => "positive",
            Self::Connecting => "live",
            Self::Paused | Self::NotPaired | Self::Offline => "attention",
            Self::Stopped | Self::Revoked => "negative",
        }
    }

    /// Chat with Work can use this computer right now.
    pub fn is_serving(self) -> bool {
        self == Self::Online
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Not running",
            Self::NotPaired => "Not connected to Chat with Work",
            Self::Paused => "Paused",
            Self::Connecting => "Connecting…",
            Self::Online => "Online",
            Self::Offline => "Offline, retrying",
            Self::Revoked => "Needs to reconnect",
        }
    }
}

/// The platform's words for things, so menus read as native ones do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    pub fn settings_item(self) -> &'static str {
        match self {
            Self::MacOs => "Settings…",
            Self::Windows | Self::Linux => "Settings",
        }
    }

    /// Where the tray item lives, in the platform's words.
    pub fn tray_place(self) -> &'static str {
        match self {
            Self::MacOs => "menu bar",
            Self::Windows => "notification area",
            Self::Linux => "system tray",
        }
    }

    pub fn quit_item(self) -> &'static str {
        match self {
            Self::MacOs => "Quit Chat with Work",
            Self::Windows => "Exit",
            Self::Linux => "Quit",
        }
    }
}

/// Everything the tray shows. Built from the status, so it can be tested
/// without a tray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub health: Health,
    /// First, disabled menu line.
    pub headline: String,
    /// Second, disabled menu line: indexing or the last access.
    pub detail: Option<String>,
    /// `Some(true)` offers Resume, `Some(false)` Pause, `None` neither.
    pub paused: Option<bool>,
    pub tooltip: String,
    /// Draw the icon faded: nothing is being served.
    pub dimmed: bool,
    /// `Some(true)` offers Resume Using the Browser, `Some(false)` Stop
    /// Using the Browser, `None` neither: the browser is off.
    pub browser_stopped: Option<bool>,
    /// What the person has to do for this computer to be connected, which
    /// the menu's first line offers and the app asks about.
    pub prompt: Option<ConnectionPrompt>,
}

/// A connection that won't come back without the person. A running app is
/// never silently not connected (docs/desktop-app.md, "Never silently not
/// connected"). Retrying through a blip isn't one of these: an offline
/// computer stays quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPrompt {
    /// Never paired, or disconnected.
    Connect,
    /// Paired, but its sign-in was refused or is gone.
    Reconnect,
    /// The background agent isn't running.
    Start,
}

impl ConnectionPrompt {
    pub fn of(status: Option<&Status>) -> Option<Self> {
        let Some(status) = status else {
            return Some(Self::Start);
        };
        match status.connection.connection.as_str() {
            _ if !status.is_paired() => Some(Self::Connect),
            "revoked" | "not_paired" => Some(Self::Reconnect),
            _ => None,
        }
    }

    /// The menu's first line, as an action.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connect => "Connect to Chat with Work…",
            Self::Reconnect => "Reconnect…",
            Self::Start => "Start the Background Agent…",
        }
    }

    /// How long this has to last before the app asks: the agent restarts
    /// itself whenever its folders change, and a status can be a moment old.
    pub fn patience(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Connect | Self::Reconnect => 5,
            Self::Start => 30,
        })
    }

    /// The settings page that does it. Reconnecting starts the sign-in at
    /// once, with the server this computer already uses; connecting first
    /// lets the person name a self-hosted one.
    pub fn page(self) -> &'static str {
        match self {
            Self::Reconnect => "reconnect",
            Self::Connect | Self::Start => "account",
        }
    }

    /// The question the app asks, in a dialog it draws.
    pub fn question(self) -> crate::computer::Question {
        let (title, message, yes) = match self {
            Self::Connect => (
                "Connect this computer to Chat with Work?",
                "Chat with Work can't use this computer until you connect it. You approve it in your browser.",
                "Connect…",
            ),
            Self::Reconnect => (
                "Reconnect this computer to Chat with Work?",
                "This computer has to sign in to Chat with Work again before it can be used. You approve it in your browser.",
                "Reconnect…",
            ),
            Self::Start => (
                "Start the background agent?",
                "The agent that connects this computer to Chat with Work isn't running.",
                "Open Settings…",
            ),
        };
        crate::computer::Question {
            title: title.into(),
            message: message.into(),
            yes: yes.into(),
        }
    }
}

impl TrayView {
    pub fn new(status: Option<&Status>) -> Self {
        let health = Health::of(status);
        let mut headline = health.label().to_string();
        if let Some(status) = status
            && health == Health::Online
        {
            headline = match status.roots.len() {
                0 => "Online, nothing shared".into(),
                1 => "Sharing 1 folder".into(),
                n => format!("Sharing {n} folders"),
            };
        }
        let detail = status.and_then(|s| {
            let indexing = s.indexing();
            if indexing > 0 {
                return Some(if indexing == 1 {
                    "Indexing 1 folder…".to_string()
                } else {
                    format!("Indexing {indexing} folders…")
                });
            }
            s.last_access
                .as_ref()
                .map(|a| format!("Last access {}", describe_access(a)))
        });
        let paused = match status {
            Some(s) if s.is_paired() => Some(s.paused),
            _ => None,
        };
        Self {
            health,
            tooltip: format!("Chat with Work Desktop: {}", health.label()),
            headline,
            detail,
            paused,
            dimmed: !health.is_serving(),
            prompt: ConnectionPrompt::of(status),
            browser_stopped: status
                .filter(|s| s.browser.enabled)
                .map(|s| s.browser.stopped),
        }
    }
}

/// "14:32, read Documents:plans/q3.md".
pub fn describe_access(entry: &AuditEntry) -> String {
    let when = crate::time::short(&entry.ts);
    let what = describe_tool(entry);
    format!("{when}, {what}")
}

/// What a tool call did, in a few words.
pub fn describe_tool(entry: &AuditEntry) -> String {
    let tool = entry.tool.as_deref().unwrap_or("request");
    let target = match tool {
        "search" => entry
            .query
            .as_deref()
            .map(|q| format!("“{}”", ellipsize(q, 40))),
        "roots" | "run_status" | "run_output" | "run_stop" => None,
        _ => entry.path.as_deref().map(|p| ellipsize(p, 48)),
    };
    if entry.dry_run == Some(true) {
        return match target {
            Some(target) => format!("checked a change to {target}"),
            None => "checked a change".into(),
        };
    }
    let verb = match (tool, entry.effect.as_deref()) {
        ("search", _) => "searched",
        ("read", _) => "read",
        ("list", _) => "listed",
        ("roots", _) => "listed the shared folders",
        (_, Some("created")) => "created",
        (_, Some("replaced")) => "replaced",
        (_, Some("appended")) => "added to",
        (_, Some("edited")) => "edited",
        (_, Some("created_folder")) => "made the folder",
        (_, Some("unchanged")) => "found the folder",
        (_, Some("moved")) => "moved",
        (_, Some("trashed")) => "moved to the trash",
        ("create" | "create_document", None) => "create",
        ("mkdir", None) => "make the folder",
        ("delete", None) => "delete",
        ("run", Some("ran")) => "ran a command in",
        ("run", None) => "a command in",
        ("run_stop", _) => "stopped a command",
        ("run_status" | "run_output", _) => "checked a command",
        (other, _) => other,
    };
    let mut text = match target {
        Some(target) => format!("{verb} {target}"),
        None => verb.to_string(),
    };
    if let Some(to) = &entry.to {
        text.push_str(&format!(" to {}", ellipsize(to, 48)));
    }
    text
}

/// Shorten `text` to `max` characters, keeping the end of paths visible.
pub fn ellipsize(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = text.chars().skip(count - keep).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(value: serde_json::Value) -> Status {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn parses_what_the_daemon_sends() {
        let s = status(json!({
            "ok": true,
            "version": "0.1.0",
            "pid": 4242,
            "connection": { "connection": "connected", "since": "2026-09-25T08:00:00Z" },
            "server": "https://chatwithwork.com",
            "device_id": "42",
            "paused": false,
            "roots": [{ "id": "documents", "label": "Documents", "available": true,
                        "index": "ready", "indexed_files": 12, "local_path": "/home/u/Documents" }],
            "last_access": { "ts": "2026-09-25T09:12:03Z", "event": "tool", "tool": "read",
                             "path": "documents:plans/q3.md", "decision": "allowed" },
            "something_new": [1, 2, 3],
        }));
        assert!(s.is_paired());
        assert_eq!(s.server_host().as_deref(), Some("chatwithwork.com"));
        assert_eq!(s.roots[0].indexed_files, Some(12));
        assert_eq!(Health::of(Some(&s)), Health::Online);
    }

    #[test]
    fn a_connection_that_needs_the_person_prompts() {
        use ConnectionPrompt::{Connect, Reconnect, Start};
        assert_eq!(ConnectionPrompt::of(None), Some(Start));
        let unpaired = status(json!({ "connection": { "connection": "not_paired" } }));
        assert!(!unpaired.is_paired());
        assert_eq!(ConnectionPrompt::of(Some(&unpaired)), Some(Connect));
        for connection in ["revoked", "not_paired"] {
            let s = status(json!({
                "server": "https://chatwithwork.com",
                "connection": { "connection": connection }
            }));
            assert_eq!(
                ConnectionPrompt::of(Some(&s)),
                Some(Reconnect),
                "{connection}"
            );
        }
        let revoked_and_paused = status(json!({
            "server": "https://chatwithwork.com",
            "paused": true,
            "connection": { "connection": "revoked" }
        }));
        assert_eq!(
            ConnectionPrompt::of(Some(&revoked_and_paused)),
            Some(Reconnect)
        );
    }

    #[test]
    fn a_blip_stays_quiet() {
        for connection in ["connected", "connecting", "offline"] {
            let s = status(json!({
                "server": "https://chatwithwork.com",
                "connection": { "connection": connection }
            }));
            assert_eq!(ConnectionPrompt::of(Some(&s)), None, "{connection}");
        }
    }

    #[test]
    fn the_menu_never_says_not_connected_with_nothing_to_press() {
        let unpaired = status(json!({ "connection": { "connection": "not_paired" } }));
        let view = TrayView::new(Some(&unpaired));
        assert_eq!(view.health, Health::NotPaired);
        assert_eq!(
            view.prompt.map(ConnectionPrompt::label),
            Some("Connect to Chat with Work…")
        );
    }

    #[test]
    fn each_prompt_leads_to_pairing_or_the_agent() {
        assert_eq!(ConnectionPrompt::Reconnect.page(), "reconnect");
        assert_eq!(ConnectionPrompt::Connect.page(), "account");
        assert_eq!(ConnectionPrompt::Start.page(), "account");
        assert!(ConnectionPrompt::Start.patience() > ConnectionPrompt::Connect.patience());
    }

    #[test]
    fn health_follows_the_daemon() {
        assert_eq!(Health::of(None), Health::Stopped);
        let mut s = status(json!({ "connection": { "connection": "not_paired" } }));
        assert_eq!(Health::of(Some(&s)), Health::NotPaired);
        s.server = Some("https://chatwithwork.com".into());
        s.connection.connection = "offline".into();
        assert_eq!(Health::of(Some(&s)), Health::Offline);
        s.connection.connection = "revoked".into();
        assert_eq!(Health::of(Some(&s)), Health::Revoked);
        s.paused = true;
        assert_eq!(Health::of(Some(&s)), Health::Paused, "paused wins");
    }

    #[test]
    fn only_online_reads_as_positive() {
        assert_eq!(Health::Online.tone(), "positive");
        assert_eq!(Health::Connecting.tone(), "live");
        assert_eq!(Health::Paused.tone(), "attention");
        assert_eq!(Health::Stopped.tone(), "negative");
        assert_eq!(Health::Revoked.tone(), "negative");
    }

    #[test]
    fn the_tray_says_what_is_going_on() {
        let stopped = TrayView::new(None);
        assert_eq!(stopped.headline, "Not running");
        assert_eq!(stopped.paused, None, "nothing to pause");
        assert!(stopped.dimmed);

        let mut s = status(json!({
            "connection": { "connection": "connected" },
            "server": "https://chatwithwork.com",
            "roots": [
                { "id": "a", "label": "A", "available": true, "index": "indexing" },
                { "id": "b", "label": "B", "available": true, "index": "ready" },
            ],
        }));
        let view = TrayView::new(Some(&s));
        assert_eq!(view.headline, "Sharing 2 folders");
        assert_eq!(view.detail.as_deref(), Some("Indexing 1 folder…"));
        assert_eq!(view.paused, Some(false));
        assert!(!view.dimmed);

        s.roots[0].index = "ready".into();
        s.last_access = Some(AuditEntry {
            ts: "2026-09-25T09:12:03Z".into(),
            event: "tool".into(),
            tool: Some("search".into()),
            query: Some("falcon budget".into()),
            ..AuditEntry::default()
        });
        let view = TrayView::new(Some(&s));
        let detail = view.detail.unwrap();
        assert!(detail.starts_with("Last access "), "{detail}");
        assert!(detail.ends_with("searched “falcon budget”"), "{detail}");

        s.paused = true;
        let view = TrayView::new(Some(&s));
        assert_eq!(view.headline, "Paused");
        assert_eq!(view.paused, Some(true));
        assert!(view.dimmed);
    }

    #[test]
    fn tool_calls_read_as_sentences() {
        let entry = |tool: &str, path: Option<&str>| AuditEntry {
            tool: Some(tool.into()),
            path: path.map(Into::into),
            ..AuditEntry::default()
        };
        assert_eq!(
            describe_tool(&entry("read", Some("docs:a.md"))),
            "read docs:a.md"
        );
        assert_eq!(describe_tool(&entry("list", Some("docs:"))), "listed docs:");
        assert_eq!(
            describe_tool(&entry("roots", None)),
            "listed the shared folders"
        );
        let long = format!("docs:{}/end.md", "x".repeat(80));
        let described = describe_tool(&entry("read", Some(&long)));
        assert!(
            described.ends_with("/end.md") && described.contains('…'),
            "{described}"
        );
    }

    #[test]
    fn menus_use_each_platforms_words() {
        assert_eq!(Platform::MacOs.settings_item(), "Settings…");
        assert_eq!(Platform::Windows.quit_item(), "Exit");
        assert_eq!(Platform::MacOs.quit_item(), "Quit Chat with Work");
    }
}
