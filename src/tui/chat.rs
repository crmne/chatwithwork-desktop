//! Chats in the terminal, through the daemon.
//!
//! The TUI never holds a token: every call goes to the daemon over the
//! control socket (CONTROL.md, "Chats"), and the daemon asks Chat with Work
//! as this computer. The shapes here are the server's, as the daemon
//! relays them; unknown fields are ignored so a newer server still parses.
//! Every function blocks; the TUI calls them from worker threads.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::control::{Client, Closer, ControlRequest};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: u64,
    pub name: String,
    /// The Phosphor icon the web shows for it: `buildings` for HQ,
    /// `users-three` for All-Access, `folder-simple` for invite-only. Only
    /// in the list's `projects`, not on a chat's `project`.
    pub icon: Option<String>,
    /// The organization's HQ project.
    pub hq: bool,
    /// Everyone in the organization is in it.
    pub all_access: bool,
    /// Its page on the web.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChatSummary {
    pub number: u64,
    pub title: String,
    /// `idle`, `processing` or `error`.
    pub state: String,
    pub project: Option<Project>,
    /// Started by this person, rather than someone in a shared project.
    pub mine: bool,
    /// RFC 3339.
    pub updated_at: String,
    /// RFC 3339, when it was started; empty from an older server.
    pub created_at: String,
    pub url: String,
    /// The model the next question goes to; `None` from an older server.
    pub model: Option<ModelRef>,
    /// Its public link while it has one, only for the person who started
    /// it.
    pub share: Option<ShareLink>,
    pub web_search: bool,
    pub schedule: Option<Schedule>,
    /// What this person may do with it; `None` from an older server, which
    /// offers none of it here.
    pub can: Option<Can>,
}

impl ChatSummary {
    pub fn processing(&self) -> bool {
        self.state == "processing"
    }

    /// Whether the server says this chat allows `action`.
    pub fn can(&self, action: impl Fn(&Can) -> bool) -> bool {
        self.can.as_ref().is_some_and(action)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WebSearch {
    pub allowed: bool,
    pub rate: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub mine: bool,
    pub name: String,
    pub timing: String,
    pub url: Option<String>,
    pub owner: Option<String>,
}

impl Schedule {
    pub fn label(&self) -> String {
        if self.mine {
            format!("{} · {}", self.name, self.timing)
        } else {
            format!(
                "Scheduled by {}",
                self.owner.as_deref().unwrap_or("a teammate")
            )
        }
    }
}

/// A chat's model, as chats name it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ModelRef {
    #[serde(deserialize_with = "id_string")]
    pub id: String,
    pub name: String,
    /// Its maker's logo, as the picker's button shows it.
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
}

/// A logo or a file-type icon the web shows, as an image on the paired
/// server: `path` is always under `/assets/`, fingerprinted, so it never
/// changes and can be cached for good. Fetch it with [`Chats::asset`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Asset {
    pub path: String,
    /// A single-colour logo: drawn as is on light backgrounds, white on
    /// dark ones, as the web inverts it in dark mode.
    pub monochrome: bool,
}

/// An image, or nothing where the server sends none or something this
/// cww can't read: a logo never stops the rest from reading.
fn asset<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Asset>, D::Error> {
    Ok(serde_json::from_value::<Asset>(Value::deserialize(d)?)
        .ok()
        .filter(|a| a.path.starts_with("/assets/")))
}

/// [`asset`] for a list of images, one per name, `None` where there's none.
fn assets<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Option<Asset>>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value::<Asset>(item)
                    .ok()
                    .filter(|a| a.path.starts_with("/assets/"))
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// An image's bytes, as [`Chats::asset`] fetched them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetData {
    /// `image/svg+xml`, `image/png`, ...
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Dictation {
    pub cloud: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ShareLink {
    pub url: String,
    /// RFC 3339.
    pub expires_at: Option<String>,
}

/// What the server lets this person do with a chat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Can {
    pub retry: bool,
    pub branch: bool,
    pub rename: bool,
    pub delete: bool,
    pub share: bool,
}

/// The models a question can be asked with (`GET /local_agent/models`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Models {
    #[serde(deserialize_with = "id_string_opt")]
    pub default_model_id: Option<String>,
    pub models: Vec<Model>,
}

impl Models {
    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Model {
    #[serde(deserialize_with = "id_string")]
    pub id: String,
    pub name: String,
    pub provider: String,
    /// Its maker's logo, as the picker shows it.
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
    pub description: Option<String>,
    /// "About 3 credits per answer".
    pub rate: Option<String>,
    /// False when it can't be used right now, for `reason`.
    pub selectable: bool,
    pub free: bool,
    pub searches_web: bool,
    pub reason: Option<String>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            provider: String::new(),
            logo: None,
            description: None,
            rate: None,
            selectable: true,
            free: false,
            searches_web: false,
            reason: None,
        }
    }
}

/// An ID the server may send as a number or a string, kept as a string.
fn id_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(id_string_opt(d)?.unwrap_or_default())
}

fn id_string_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Named {
    pub name: String,
    /// The organization's logo; the server sends none yet (the web draws
    /// the name's first letter).
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
    /// The person's picture; the server sends none (draw initials).
    #[serde(deserialize_with = "asset")]
    pub avatar: Option<Asset>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChatList {
    /// Newest first.
    pub chats: Vec<ChatSummary>,
    pub projects: Vec<Project>,
    pub account: Named,
    pub user: Named,
    /// The organization's credits; `None` from an older server.
    pub credits: Option<Credits>,
    pub web_search: WebSearch,
    /// Why a new question can't be asked right now, as the composer says it.
    pub locked_reason: Option<String>,
    /// What the person pinned on the web, in pin order. Not streamed: as
    /// current as the last read.
    pub pins: Pins,
    /// The web's pages the sidebar and the settings open in the browser.
    pub links: Links,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Pins {
    /// Project ids.
    pub projects: Vec<u64>,
    /// Chat numbers.
    pub chats: Vec<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Links {
    pub new_chat: Option<String>,
    pub chats: Option<String>,
    pub projects: Option<String>,
    /// The tabs of Settings on the web, in its order, only those the
    /// person's role opens.
    pub settings: Vec<SettingsLink>,
}

/// A tab of Settings on the web.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SettingsLink {
    /// `account`, `people`, `usage`, `models`, `connectors`, ...
    pub tab: String,
    pub label: String,
    /// A Phosphor icon name.
    pub icon: Option<String>,
    pub url: String,
}

/// The meter under the person's name: the organization's balance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Credits {
    #[serde(deserialize_with = "whole")]
    pub left: u64,
    #[serde(deserialize_with = "whole")]
    pub capacity: u64,
    /// Under 20% left: the web's sidebar shows the meter then.
    pub running_low: bool,
}

/// A count the server may send as a whole or a decimal number.
fn whole<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0).round() as u64))
            .unwrap_or_default(),
        _ => 0,
    })
}

/// The list's header again, when something in it changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AccountUpdate {
    pub account: Named,
    pub user: Named,
    pub credits: Option<Credits>,
    pub web_search: WebSearch,
    pub locked_reason: Option<String>,
}

impl ChatList {
    /// Take a header the list's live updates sent.
    pub fn set_account(&mut self, update: AccountUpdate) {
        self.account = update.account;
        self.user = update.user;
        self.credits = update.credits;
        self.web_search = update.web_search;
        self.locked_reason = update.locked_reason;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Source {
    pub title: String,
    pub url: Option<String>,
    /// A file's type icon; `None` for a web page (the web asks Google for
    /// its favicon from the browser).
    #[serde(deserialize_with = "asset")]
    pub icon: Option<Asset>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Step {
    /// "Searched Drive for “budget”".
    pub summary: String,
    pub pending: bool,
    /// Names of the files it found, for the owner's own steps.
    pub files: Vec<String>,
    pub code: Option<CodeRun>,
    pub downloads: Vec<Attachment>,
    pub glyph: Option<String>,
    /// The tool's own view (an MCP App), when the step has one. The
    /// server doesn't send this yet; the desktop app keeps a place for it.
    pub app: Option<StepApp>,
    /// The answer stopped here for the person: a change to approve (see
    /// [`Transcript::approvals`]) or a question from the tool's server.
    pub waiting: bool,
    /// Its service's logo; `None` where the web shows a glyph instead.
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
}

/// A tool's view (MCP Apps): the service it belongs to and the `ui://`
/// resource to show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StepApp {
    /// "Slack", as the step's service is named.
    pub service: String,
    pub uri: String,
}

/// One thing in a conversation, as the web shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    User {
        id: u64,
        content: String,
        /// Who asked, when it wasn't this person (a project chat).
        #[serde(default)]
        author: Option<String>,
        /// Files sent with the question.
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    Assistant {
        id: u64,
        /// Markdown.
        content: String,
        #[serde(default)]
        sources: Vec<Source>,
    },
    /// The tool work between two things said, folded into one line.
    Activity {
        id: u64,
        /// "Searched Drive and Slack".
        title: String,
        /// "3 searches, read 2 files".
        #[serde(default)]
        details: Option<String>,
        /// What the running step reports doing.
        #[serde(default)]
        progress: Option<String>,
        #[serde(default)]
        services: Vec<String>,
        /// One per name in `services`: its logo, or `None` where the web
        /// shows a glyph (Phosphor's `laptop` for a computer,
        /// `plugs-connected` for a person's own MCP server).
        #[serde(default, deserialize_with = "assets")]
        logos: Vec<Option<Asset>>,
        #[serde(default)]
        glyphs: Vec<Option<String>>,
        #[serde(default)]
        pending: bool,
        #[serde(default)]
        steps: Vec<Step>,
    },
    /// A failure or running out of credits. `tone` is `negative` or `attention`.
    Notice { tone: String, text: String },
    #[serde(other)]
    Unknown,
}

/// A file sent with a question or produced by a tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Attachment {
    /// Same-server signed proxy path; never an arbitrary URL.
    pub path: Option<String>,
    pub filename: String,
    pub byte_size: u64,
    pub content_type: String,
    /// Its file type's icon.
    #[serde(deserialize_with = "asset")]
    pub icon: Option<Asset>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Transcript {
    pub chat: ChatSummary,
    pub locked_reason: Option<String>,
    pub entries: Vec<Entry>,
    /// Changes the answer stopped at, oldest first, until each is decided.
    pub approvals: Vec<Approval>,
    /// Questions the tools' servers asked, until each is answered.
    pub questions: Vec<Question>,
}

impl Transcript {
    pub fn downloads(&self) -> Vec<&Attachment> {
        self.entries
            .iter()
            .flat_map(|entry| match entry {
                Entry::User { attachments, .. } => attachments.iter().collect::<Vec<_>>(),
                Entry::Activity { steps, .. } => steps.iter().flat_map(|s| &s.downloads).collect(),
                _ => Vec::new(),
            })
            .filter(|file| file.path.is_some())
            .collect()
    }

    /// The first change this person can decide, if any.
    pub fn next_decision(&self) -> Option<&Approval> {
        self.approvals.iter().find(|a| a.decidable)
    }

    /// The first question this person can answer, if any.
    pub fn next_question(&self) -> Option<&Question> {
        self.questions.iter().find(|q| q.decidable)
    }
}

/// A question a tool's server asked while its tool runs (MCP elicitation),
/// as the web's question card asks it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Question {
    pub id: u64,
    /// "Notion".
    pub service: String,
    /// The service's logo; `None` where the web shows the
    /// `plugs-connected` glyph (a person's own MCP server).
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
    /// This person answers it; otherwise it waits for `waiting_for`.
    pub decidable: bool,
    pub message: String,
    /// `form` (fill in `fields`) or `url` (visit `url`, then say so).
    pub kind: String,
    pub fields: Vec<Field>,
    /// The page to visit, only `http` and `https`.
    pub url: Option<String>,
    pub host: Option<String>,
    /// What to keep in mind, to show with the form.
    pub note: Option<String>,
    pub waiting_for: Option<String>,
}

impl Question {
    pub fn is_url(&self) -> bool {
        self.kind == "url"
    }
}

/// One thing a question's form asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Field {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    /// `string`, `integer`, `number`, `boolean` or `array` (any of
    /// `choices`).
    #[serde(rename = "type")]
    pub kind: String,
    pub required: bool,
    pub choices: Option<Vec<String>>,
    pub default: Option<Value>,
}

impl Field {
    /// What the form calls it.
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.name
        } else {
            &self.title
        }
    }
}

/// A change the answer stopped at, as the web's approval card shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Approval {
    pub id: u64,
    /// "Linear".
    pub service: String,
    /// The service's logo, as the card's avatar shows it; `None` where
    /// the web shows the `plugs-connected` glyph.
    #[serde(deserialize_with = "asset")]
    pub logo: Option<Asset>,
    pub effect: String,
    /// This person decides it; otherwise it waits for `waiting_for`.
    pub decidable: bool,
    /// "Create issue in Linear".
    pub summary: String,
    pub details: Vec<Detail>,
    pub preview: Option<ChangePreview>,
    /// It can be approved for the rest of the chat.
    pub allow_for_rest_of_chat: bool,
    pub waiting_for: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub label: String,
    pub value: String,
}

/// The code and output the server is allowed to show this person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CodeRun {
    pub language: String,
    pub source: String,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

impl CodeRun {
    pub fn markdown(&self) -> String {
        let mut text = fenced(&self.language, &self.source);
        for (label, output) in [
            ("Output", &self.stdout),
            ("Errors", &self.stderr),
            ("Error", &self.error),
        ] {
            if let Some(output) = output.as_ref().filter(|s| !s.is_empty()) {
                text.push_str(&format!("\n\n**{label}**\n\n{}", fenced("text", output)));
            }
        }
        if let Some(code) = self.exit_code {
            text.push_str(&format!("\n\nExit code: {code}"));
        }
        text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChangePreview {
    pub action: String,
    pub rows: Vec<Detail>,
    pub body: Option<PreviewBody>,
    pub notes: Vec<PreviewNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewBody {
    Text {
        text: String,
    },
    Diff {
        lines: Vec<DiffLine>,
    },
    Replace {
        old_text: String,
        new_text: String,
    },
    Document {
        markdown: String,
    },
    Sheets {
        sheets: Vec<PreviewSheet>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DiffLine {
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PreviewSheet {
    pub name: String,
    pub header: bool,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PreviewNote {
    pub tone: String,
    pub text: String,
}

impl Approval {
    pub fn fields(&self) -> &[Detail] {
        self.preview.as_ref().map_or(&self.details, |p| &p.rows)
    }
}

impl PreviewBody {
    pub fn markdown(&self) -> String {
        match self {
            Self::Text { text } => fenced("text", text),
            Self::Diff { lines } => fenced("diff", &lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")),
            Self::Replace { old_text, new_text } => format!("**Replace**\n\n{}\n\n**With**\n\n{}", fenced("text", old_text), fenced("text", new_text)),
            Self::Document { markdown } => markdown.clone(),
            Self::Sheets { sheets } => sheets.iter().map(|sheet| {
                let columns = sheet.rows.iter().map(Vec::len).max().unwrap_or(0);
                let mut text = format!("### {}\n\n", markdown_cell(&sheet.name));
                if columns == 0 { return text; }
                let row = |cells: &[String]| format!("| {} |\n", (0..columns).map(|i| markdown_cell(cells.get(i).map_or("", String::as_str))).collect::<Vec<_>>().join(" | "));
                let skip = if sheet.header {
                    text.push_str(&row(&sheet.rows[0]));
                    1
                } else {
                    text.push_str(&row(&(1..=columns).map(|i| format!("Column {i}")).collect::<Vec<_>>()));
                    0
                };
                text.push_str(&format!("| {} |\n", vec!["---"; columns].join(" | ")));
                for cells in &sheet.rows[skip..] { text.push_str(&row(cells)); }
                text
            }).collect::<Vec<_>>().join("\n"),
            Self::Unknown => "Preview unavailable in this version. Review this change on the web before approving.".into(),
        }
    }
}

/// Preserve literal output, even if it contains Markdown fences or links.
fn fenced(language: &str, text: &str) -> String {
    let fence = "`".repeat(
        text.split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    let language: String = language
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("{fence}{language}\n{text}\n{fence}")
}

fn markdown_cell(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
        .replace(['\n', '\r'], " ")
}

impl Attachment {
    /// A suggested basename for a native Save dialog, never a server-chosen directory.
    pub fn save_name(&self) -> &str {
        self.filename
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .unwrap_or("download")
    }
}

/// A file uploaded for a question.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Uploaded {
    pub signed_id: String,
    pub filename: String,
    pub byte_size: u64,
    pub content_type: String,
    /// Its file type's icon.
    #[serde(deserialize_with = "asset")]
    pub icon: Option<Asset>,
}

/// A chat's new public link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Shared {
    pub url: String,
    /// RFC 3339.
    pub expires_at: Option<String>,
    pub chat: ChatSummary,
}

/// A question to ask with [`Chats::ask`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    /// The chat to ask in; `None` starts one.
    pub chat: Option<u64>,
    pub text: String,
    /// A project for a new chat.
    pub project: Option<u64>,
    /// A model ID from [`Chats::models`]; `None` keeps the chat's, or the
    /// default for a new chat.
    pub model: Option<String>,
    pub web_search: Option<bool>,
    /// Files from [`Chats::upload`], by `signed_id`.
    pub attachments: Vec<String>,
}

/// The largest file a question can carry.
pub const MAX_UPLOAD_BYTES: u64 = crate::chats::MAX_UPLOAD_BYTES as u64;

/// Where asking for chat access leaves things.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AccessRequest {
    pub granted: bool,
    pub requested: bool,
    pub approve_url: Option<String>,
}

/// Why a chat call didn't work, with the code to act on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Failure {
    /// `daemon_stopped`, `daemon_outdated`, `not_paired`, `revoked`, `unreachable`,
    /// `unsupported`, `chat_access_required`, `locked`, `chat_busy`,
    /// `not_found`, `rate_limited`, `invalid`, `forbidden`, ...
    pub code: String,
    pub message: String,
    pub approve_url: Option<String>,
    /// The owner was already asked and hasn't answered.
    pub requested: bool,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            ..Self::default()
        }
    }

    fn from_response(response: &Value) -> Self {
        // A daemon from before chats doesn't know the request at all.
        let error = response["error"].as_str().unwrap_or_default();
        if response["code"].is_null() && error.starts_with("bad request: unknown variant") {
            return Self::new(
                "daemon_outdated",
                "The daemon running now is older than this cww and can't relay chats. \
                 Restart it with this version of cww.",
            );
        }
        Self {
            code: response["code"].as_str().unwrap_or("error").into(),
            message: response["error"]
                .as_str()
                .unwrap_or("The daemon returned an error")
                .into(),
            approve_url: response["approve_url"].as_str().map(str::to_string),
            requested: response["requested"].as_bool().unwrap_or(false),
        }
    }
}

/// Something that happened in a followed chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Live {
    /// Answer text as it's written.
    Chunk {
        message_id: u64,
        seq: Option<u64>,
        text: String,
    },
    /// What the running tool step is doing.
    Progress(String),
    /// Anything else: read the chat again.
    Changed,
    /// The server follows the chat for us (again): catch up.
    Watching,
    /// The server won't let this computer follow it.
    Refused,
    /// The daemon's connection can't follow chats at all (it speaks bare
    /// MCP): nothing to retry until it reconnects.
    Unsupported,
    /// No connection to the server right now; the daemon keeps trying.
    Offline,
}

impl Live {
    /// What one event of the daemon's stream says about the followed chat.
    /// Missed events (`lagged`) mean reading the chat again; anything else,
    /// like a heartbeat, is nothing new but a chance to stop.
    fn from_event(event: &Value) -> Option<Self> {
        match event["event"].as_str() {
            Some("chat") => Self::from_update(&event["update"]),
            Some("lagged") => Some(Self::Changed),
            _ => None,
        }
    }

    /// `None` only for a chunk it can't read; anything unknown means "read
    /// the chat again".
    fn from_update(update: &Value) -> Option<Self> {
        Some(match update["type"].as_str().unwrap_or_default() {
            "chunk" => Self::Chunk {
                message_id: update["message_id"].as_u64()?,
                seq: match update.get("seq") {
                    None | Some(Value::Null) => None,
                    Some(seq) => Some(seq.as_u64()?),
                },
                text: update["text"].as_str()?.to_string(),
            },
            "progress" => Self::Progress(update["text"].as_str()?.to_string()),
            "watching" => Self::Watching,
            "refused" => Self::Refused,
            "unsupported" => Self::Unsupported,
            "offline" => Self::Offline,
            _ => Self::Changed,
        })
    }
}

/// A change to the chat list, as the server keeps it current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListLive {
    /// A chat started, or changed: put it in by `number`, newest first.
    Chat(ChatSummary),
    /// A chat deleted, or out of sight now: drop it, close it if open.
    Removed(u64),
    /// Every project again.
    Projects(Vec<Project>),
    /// The list's header again.
    Account(AccountUpdate),
    /// The server keeps the list current (again): list the chats once.
    Watching,
    /// The server won't keep it current for this computer.
    Refused,
    /// The daemon's connection can't (bare MCP).
    Unsupported,
    /// No connection to the server; the daemon keeps trying.
    Offline,
    /// Changes were missed: list the chats again.
    Missed,
}

impl ListLive {
    /// What one event of the daemon's stream says about the list. `None`
    /// for heartbeats and anything this cww doesn't know.
    fn from_event(event: &Value) -> Option<Self> {
        match event["event"].as_str() {
            Some("chats") => Self::from_update(&event["update"]),
            Some("lagged") => Some(Self::Missed),
            _ => None,
        }
    }

    fn from_update(update: &Value) -> Option<Self> {
        Some(match update["event"].as_str()? {
            "chat" => Self::Chat(serde_json::from_value(update["chat"].clone()).ok()?),
            "removed" => Self::Removed(update["number"].as_u64()?),
            "projects" => Self::Projects(serde_json::from_value(update["projects"].clone()).ok()?),
            "account" => Self::Account(serde_json::from_value(update.clone()).ok()?),
            "watching" => Self::Watching,
            "refused" => Self::Refused,
            "unsupported" => Self::Unsupported,
            "offline" => Self::Offline,
            _ => return None,
        })
    }
}

/// The daemon, as the chat pane talks to it.
#[derive(Debug, Clone)]
pub struct Chats {
    socket: PathBuf,
}

impl Chats {
    pub fn new(socket: &Path) -> Self {
        Self {
            socket: socket.to_path_buf(),
        }
    }

    pub fn list(&self) -> Result<ChatList, Failure> {
        parse(self.call(&ControlRequest::Chats)?)
    }

    pub fn workspace(&self, project: Option<u64>) -> Result<crate::workspace::NewChat, Failure> {
        parse(self.call(&ControlRequest::Workspace { project })?)
    }

    pub fn service_pause(
        &self,
        service: &crate::workspace::Service,
        paused: bool,
    ) -> Result<crate::workspace::NewChat, Failure> {
        parse(self.call(&ControlRequest::ServicePause {
            kind: service.kind,
            key: service.key.clone(),
            paused,
        })?)
    }

    pub fn instructions(
        &self,
        project: Option<u64>,
    ) -> Result<crate::workspace::InstructionPage, Failure> {
        parse(self.call(&ControlRequest::Instructions { project })?)
    }

    pub fn memory_preference(&self, enabled: bool) -> Result<crate::workspace::Memory, Failure> {
        parse(self.call(&ControlRequest::MemoryPreference { enabled })?)
    }

    pub fn memory_forget(&self, id: u64) -> Result<crate::workspace::Memory, Failure> {
        parse(self.call(&ControlRequest::MemoryForget { id })?)
    }

    pub fn instruction_count(&self, text: String) -> Result<usize, Failure> {
        parse(self.call(&ControlRequest::InstructionCount { text })?["tokens"].take())
    }

    pub fn computer_remove(&self, id: u64) -> Result<crate::workspace::NewChat, Failure> {
        parse(self.call(&ControlRequest::ComputerRemove { id })?)
    }

    pub fn connectors(
        &self,
        query: String,
        category: Option<String>,
    ) -> Result<crate::workspace::Catalog, Failure> {
        parse(self.call(&ControlRequest::Connectors { query, category })?)
    }

    pub fn connector_handoff(
        &self,
        key: String,
        access: String,
        connection: Option<String>,
    ) -> Result<String, Failure> {
        parse(
            self.call(&ControlRequest::ConnectorHandoff {
                key,
                access,
                connection,
            })?["path"]
                .take(),
        )
    }

    pub fn instruction_update(
        &self,
        project: Option<u64>,
        layer: crate::workspace::Layer,
        change: crate::workspace::InstructionChange,
    ) -> Result<crate::workspace::SavedInstruction, Failure> {
        parse(self.call(&ControlRequest::InstructionUpdate {
            project,
            layer,
            change,
        })?)
    }

    pub fn show(&self, chat: u64) -> Result<Transcript, Failure> {
        parse(self.call(&ControlRequest::Chat {
            chat: chat.to_string(),
        })?)
    }

    /// Ask in `chat`, or start a new chat. Answers with the chat.
    pub fn send(&self, chat: Option<u64>, text: &str) -> Result<ChatSummary, Failure> {
        self.ask(&Ask {
            chat,
            text: text.to_string(),
            ..Ask::default()
        })
    }

    /// Ask with a model, a project or files. `model_unavailable` means the
    /// model can't be used (any more).
    pub fn ask(&self, ask: &Ask) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatSend {
            chat: ask.chat.map(|c| c.to_string()),
            text: ask.text.clone(),
            project: ask.project,
            model: ask.model.clone(),
            web_search: ask.web_search,
            attachments: ask.attachments.clone(),
        })?;
        parse(response["chat"].clone())
    }

    /// The models to pick from. `unsupported` means a server without the
    /// choice, `daemon_outdated` a daemon that can't relay it.
    pub fn models(&self) -> Result<Models, Failure> {
        parse(self.call(&ControlRequest::Models)?)
    }

    pub fn dictation(&self) -> Result<Dictation, Failure> {
        parse(self.call(&ControlRequest::Dictation)?)
    }

    pub fn dictate(&self, data: &[u8], content_type: &str) -> Result<String, Failure> {
        use base64::Engine;
        if data.len() > crate::chats::MAX_AUDIO_BYTES {
            return Err(Failure::new("too_large", "Record at most five minutes"));
        }
        let response = self.call_slow(&ControlRequest::Dictate {
            data: base64::engine::general_purpose::STANDARD.encode(data),
            content_type: content_type.into(),
        })?;
        response["text"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Failure::new("invalid", "The transcription response was incomplete"))
    }

    /// Fetch bytes only; the client chooses where to save, outside the daemon.
    pub fn download(&self, path: &str) -> Result<AssetData, Failure> {
        use base64::Engine;
        crate::chats::download_path(path).map_err(|e| Failure::new("invalid", e.to_string()))?;
        let response = self.call_slow(&ControlRequest::ChatDownload { path: path.into() })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(response["data"].as_str().unwrap_or_default())
            .map_err(|_| Failure::new("invalid", "The download was incomplete"))?;
        Ok(AssetData {
            bytes,
            content_type: response["content_type"].as_str().unwrap_or_default().into(),
        })
    }

    /// Save only to the destination the person chose. The TUI never overwrites;
    /// the desktop picker obtains overwrite confirmation before this call.
    pub fn save_download(
        &self,
        path: &str,
        destination: &Path,
        overwrite: bool,
    ) -> Result<(), Failure> {
        use std::io::Write;
        let data = self.download(path)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut file = options.open(destination).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Failure::new("exists", "That file already exists. Choose another name.")
            } else {
                Failure::new(
                    "save",
                    "Could not create the file at the chosen destination.",
                )
            }
        })?;
        file.write_all(&data.bytes)
            .map_err(|_| Failure::new("save", "Could not finish saving the file."))
    }

    /// Approve a change the answer stopped at. Answers with the chat.
    pub fn approve(
        &self,
        chat: u64,
        tool_call: u64,
        for_rest_of_chat: bool,
    ) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatApprove {
            chat: chat.to_string(),
            tool_call: tool_call.to_string(),
            for_rest_of_chat,
        })?;
        parse(response["chat"].clone())
    }

    /// Deny a change, optionally saying what to do instead.
    pub fn deny(
        &self,
        chat: u64,
        tool_call: u64,
        reason: Option<&str>,
    ) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatDeny {
            chat: chat.to_string(),
            tool_call: tool_call.to_string(),
            reason: reason.map(str::to_string),
        })?;
        parse(response["chat"].clone())
    }

    /// Read a file and upload it for a question, through the daemon (which
    /// can't read outside shared folders itself). Files over
    /// [`MAX_UPLOAD_BYTES`] fail with `attachment_refused` before anything
    /// is sent.
    pub fn upload(&self, path: &Path) -> Result<Uploaded, Failure> {
        use base64::Engine;
        let refused = |message: String| Failure::new("attachment_refused", message);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let size = std::fs::metadata(path)
            .map_err(|e| refused(format!("Can't read {}: {e}", path.display())))?
            .len();
        if size > MAX_UPLOAD_BYTES {
            return Err(refused(format!(
                "{name} is larger than {} MB",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        let data = std::fs::read(path)
            .map_err(|e| refused(format!("Can't read {}: {e}", path.display())))?;
        let response = self.call_slow(&ControlRequest::ChatUpload {
            filename: name.clone(),
            content_type: Some(content_type(&name).into()),
            data: base64::engine::general_purpose::STANDARD.encode(&data),
        })?;
        parse(response)
    }

    /// Answer `message` again, or the latest question. Answers with the
    /// chat; `chat_busy` while an answer is written.
    pub fn retry(&self, chat: u64, message: Option<u64>) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatRetry {
            chat: chat.to_string(),
            message: message.map(|m| m.to_string()),
        })?;
        parse(response["chat"].clone())
    }

    /// A new chat with the conversation up to `message`, or all of it.
    /// Answers with the new chat.
    pub fn branch(&self, chat: u64, message: Option<u64>) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatBranch {
            chat: chat.to_string(),
            message: message.map(|m| m.to_string()),
        })?;
        parse(response["chat"].clone())
    }

    pub fn rename(&self, chat: u64, title: &str) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatRename {
            chat: chat.to_string(),
            title: title.to_string(),
        })?;
        parse(response["chat"].clone())
    }

    pub fn delete(&self, chat: u64) -> Result<(), Failure> {
        self.call(&ControlRequest::ChatDelete {
            chat: chat.to_string(),
        })
        .map(|_| ())
    }

    /// Make the chat's public link, or give the one it has 30 more days.
    pub fn share(&self, chat: u64) -> Result<Shared, Failure> {
        parse(self.call(&ControlRequest::ChatShare {
            chat: chat.to_string(),
        })?)
    }

    /// Stop sharing: the link stops working. Answers with the chat.
    pub fn unshare(&self, chat: u64) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatUnshare {
            chat: chat.to_string(),
        })?;
        parse(response["chat"].clone())
    }

    /// Answer a question from a tool's server: the form's values by field
    /// name, or `None` once the person has been to the page it asked them
    /// to open. Answers with the chat; `invalid` says what's missing.
    pub fn answer(
        &self,
        chat: u64,
        tool_call: u64,
        input: Option<Value>,
    ) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatAnswer {
            chat: chat.to_string(),
            tool_call: tool_call.to_string(),
            input,
        })?;
        parse(response["chat"].clone())
    }

    /// Decline a question from a tool's server. Answers with the chat.
    pub fn decline(&self, chat: u64, tool_call: u64) -> Result<ChatSummary, Failure> {
        let response = self.call(&ControlRequest::ChatDecline {
            chat: chat.to_string(),
            tool_call: tool_call.to_string(),
        })?;
        parse(response["chat"].clone())
    }

    pub fn cancel(&self, chat: u64) -> Result<(), Failure> {
        self.call(&ControlRequest::ChatCancel {
            chat: chat.to_string(),
        })
        .map(|_| ())
    }

    pub fn request_access(&self) -> Result<AccessRequest, Failure> {
        parse(self.call(&ControlRequest::ChatAccess)?)
    }

    /// An image the web shows (an [`Asset`]'s `path`), fetched from the
    /// paired server by the daemon: only paths under `/assets/`, images of
    /// at most 1 MiB, never through a redirect. Fingerprinted paths never
    /// change, so keep what this returns. `not_found` when the server has
    /// no such image, `invalid` for a path that isn't one.
    pub fn asset(&self, path: &str) -> Result<AssetData, Failure> {
        use base64::Engine;
        crate::chats::asset_path(path)
            .map_err(|refusal| Failure::new(&refusal.code, refusal.message))?;
        let response = self.call(&ControlRequest::Asset {
            path: path.to_string(),
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(response["data"].as_str().unwrap_or_default())
            .map_err(|_| {
                Failure::new(
                    "unsupported",
                    "The daemon sent an image this cww can't read.",
                )
            })?;
        Ok(AssetData {
            content_type: response["content_type"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            bytes,
        })
    }

    /// Follow `chat` until `on_live` answers false or the daemon goes
    /// away. `on_live` also hears `None` now and then, when nothing
    /// happened, so it can stop; `on_start` gets a way to stop it sooner
    /// from another thread.
    pub fn follow(
        &self,
        chat: u64,
        on_start: impl FnOnce(Option<Closer>),
        mut on_live: impl FnMut(Option<Live>) -> bool,
    ) -> Result<(), Failure> {
        let client = self.connect()?;
        let events = client
            .follow_chat(&chat.to_string())
            .map_err(|e| Failure::new("daemon_stopped", format!("{e:#}")))?
            .map_err(|response| Failure::from_response(&response))?;
        on_start(events.closer());
        for event in events {
            let Ok(event) = event else { break };
            if !on_live(Live::from_event(&event)) {
                break;
            }
        }
        Ok(())
    }

    /// Keep the chat list current until `on_live` answers false or the
    /// daemon goes away, as [`Chats::follow`] does for one chat. List the
    /// chats when it says [`ListLive::Watching`]; a server from before live
    /// lists never does, and the list is only as current as its last read.
    pub fn follow_list(
        &self,
        on_start: impl FnOnce(Option<Closer>),
        mut on_live: impl FnMut(Option<ListLive>) -> bool,
    ) -> Result<(), Failure> {
        let client = self.connect()?;
        let events = client
            .follow_chats()
            .map_err(|e| Failure::new("daemon_stopped", format!("{e:#}")))?
            .map_err(|response| Failure::from_response(&response))?;
        on_start(events.closer());
        for event in events {
            let Ok(event) = event else { break };
            if !on_live(ListLive::from_event(&event)) {
                break;
            }
        }
        Ok(())
    }

    fn connect(&self) -> Result<Client, Failure> {
        match Client::connect(&self.socket) {
            Ok(Some(client)) => Ok(client),
            Ok(None) => Err(Failure::new("daemon_stopped", "The daemon isn't running.")),
            Err(e) => Err(Failure::new("daemon_stopped", format!("{e:#}"))),
        }
    }

    fn call(&self, request: &ControlRequest) -> Result<Value, Failure> {
        self.call_with(request, Client::call_raw)
    }

    /// A call that may take minutes, like an upload.
    fn call_slow(&self, request: &ControlRequest) -> Result<Value, Failure> {
        self.call_with(request, Client::call_slow)
    }

    fn call_with(
        &self,
        request: &ControlRequest,
        call: impl FnOnce(&mut Client, &ControlRequest) -> anyhow::Result<Value>,
    ) -> Result<Value, Failure> {
        let response = call(&mut self.connect()?, request)
            .map_err(|e| Failure::new("daemon_stopped", format!("{e:#}")))?;
        if response["ok"] == Value::Bool(true) {
            Ok(response)
        } else {
            Err(Failure::from_response(&response))
        }
    }
}

/// The type to upload a file as, from its extension. The server checks it
/// again.
pub fn content_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "odt" => "application/vnd.oasis.opendocument.text",
        "rtf" => "application/rtf",
        _ => "application/octet-stream",
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Failure> {
    serde_json::from_value(value).map_err(|e| {
        Failure::new(
            "unsupported",
            format!("Chat with Work sent something this cww doesn't understand ({e}). Update cww."),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_daemon_from_before_chats_reads_as_outdated() {
        let failure = Failure::from_response(&json!({
            "ok": false,
            "error": "bad request: unknown variant `chats`, expected one of `hello`, `status`",
        }));
        assert_eq!(failure.code, "daemon_outdated");
        assert!(failure.message.contains("older than this cww"));
    }

    #[test]
    fn parses_the_servers_transcript() {
        let transcript: Transcript = serde_json::from_value(json!({
            "chat": { "number": 7, "title": "Budget", "state": "processing", "project": null,
                      "mine": true, "updated_at": "2026-09-25T08:14:03Z", "url": "https://x/1/chats/7" },
            "locked_reason": null,
            "entries": [
                { "kind": "user", "id": 1, "content": "Q3?" },
                { "kind": "activity", "id": 2, "title": "Searching Drive", "services": ["Drive"], "pending": true,
                  "steps": [{ "summary": "Searching Drive for “q3”", "pending": true }] },
                { "kind": "assistant", "id": 3, "content": "**€40k**", "sources": [{ "title": "Q3.pdf", "url": null }] },
                { "kind": "notice", "tone": "negative", "text": "Model is rate limited." },
                { "kind": "hologram", "id": 9 }
            ]
        }))
        .unwrap();
        assert!(transcript.chat.processing());
        assert_eq!(transcript.entries.len(), 5);
        assert!(
            matches!(&transcript.entries[1], Entry::Activity { pending: true, steps, .. } if steps.len() == 1)
        );
        assert_eq!(transcript.entries[4], Entry::Unknown);
    }

    #[test]
    fn parses_models_approvals_and_what_a_chat_allows() {
        let transcript: Transcript = serde_json::from_value(json!({
            "chat": { "number": 7, "title": "Issue", "state": "idle",
                      "model": { "id": 3, "name": "Claude Sonnet" },
                      "can": { "retry": true, "branch": false } },
            "entries": [
                { "kind": "user", "id": 1, "content": "File it",
                  "attachments": [{ "filename": "log.txt", "byte_size": 12, "content_type": "text/plain" }] },
                { "kind": "activity", "id": 2, "title": "Creating an issue", "pending": false,
                  "steps": [{ "summary": "Create issue in Linear", "pending": false, "waiting": true }] }
            ],
            "approvals": [
                { "id": 12, "service": "Linear", "effect": "write", "decidable": true,
                  "summary": "Create issue in Linear", "details": [{ "label": "Title", "value": "Fix login" }],
                  "allow_for_rest_of_chat": true, "approve_path": "/x", "deny_path": "/y" },
                { "id": 13, "service": "Linear", "effect": "write", "decidable": false, "waiting_for": "Ada" }
            ]
        }))
        .unwrap();
        assert_eq!(
            transcript.chat.model,
            Some(ModelRef {
                id: "3".into(),
                name: "Claude Sonnet".into(),
                logo: None,
            })
        );
        assert!(transcript.chat.can(|c| c.retry));
        assert!(!transcript.chat.can(|c| c.branch));
        assert!(
            matches!(&transcript.entries[0], Entry::User { attachments, .. } if attachments[0].filename == "log.txt")
        );
        let next = transcript.next_decision().unwrap();
        assert_eq!(next.id, 12);
        assert_eq!(next.details[0].value, "Fix login");
        assert_eq!(transcript.approvals[1].waiting_for.as_deref(), Some("Ada"));
        assert_eq!(transcript.next_question(), None);

        // An older server: no model, nothing it allows.
        let old: ChatSummary = serde_json::from_value(json!({ "number": 1 })).unwrap();
        assert_eq!((old.model, old.can), (None, None));

        let models: Models = serde_json::from_value(json!({
            "default_model_id": "gpt-5",
            "models": [{ "id": "gpt-5", "name": "GPT-5", "provider": "openai" }]
        }))
        .unwrap();
        assert!(
            models.get("gpt-5").unwrap().selectable,
            "selectable unless said"
        );
    }

    #[test]
    fn parses_questions_from_a_tools_server() {
        let transcript: Transcript = serde_json::from_value(json!({
            "chat": { "number": 42, "share": { "url": "https://x/shared/abc", "expires_at": "2026-11-01T09:00:00Z" } },
            "questions": [
                { "id": 33, "service": "Linear", "decidable": false, "waiting_for": "Ada Lovelace" },
                { "id": 32, "service": "Notion", "decidable": true,
                  "message": "Which environment should the report cover?", "kind": "form",
                  "fields": [
                    { "name": "environment", "title": "Environment", "description": null, "type": "string",
                      "required": true, "choices": ["staging", "production"], "default": null },
                    { "name": "days", "title": "", "type": "integer", "required": false, "choices": null, "default": 7 }
                  ],
                  "url": null, "host": null, "note": "Only Notion sees your answer.",
                  "answer_path": "/x", "decline_path": "/x" }
            ]
        }))
        .unwrap();
        assert_eq!(
            transcript.chat.share.as_ref().map(|s| s.url.as_str()),
            Some("https://x/shared/abc")
        );
        let question = transcript.next_question().unwrap();
        assert_eq!(question.id, 32);
        assert!(!question.is_url());
        assert_eq!(question.fields[0].kind, "string");
        assert_eq!(
            question.fields[0].choices.as_deref(),
            Some(&["staging".to_string(), "production".to_string()][..])
        );
        assert_eq!(question.fields[1].label(), "days");
        assert_eq!(question.fields[1].default, Some(json!(7)));
        assert_eq!(
            question.note.as_deref(),
            Some("Only Notion sees your answer.")
        );
    }

    #[test]
    fn uploads_get_a_type_from_their_extension() {
        assert_eq!(content_type("Q3.PDF"), "application/pdf");
        assert_eq!(content_type("notes"), "application/octet-stream");
    }

    #[test]
    fn reads_failures_and_live_updates() {
        let failure = Failure::from_response(&json!({
            "ok": false, "error": "Allow it in Settings", "code": "chat_access_required",
            "requested": true, "approve_url": "https://x/settings"
        }));
        assert_eq!(failure.code, "chat_access_required");
        assert!(failure.requested);
        assert_eq!(
            Live::from_update(&json!({ "type": "chunk", "message_id": 5, "text": "Hi" })),
            Some(Live::Chunk {
                message_id: 5,
                seq: None,
                text: "Hi".into()
            })
        );
        assert_eq!(
            Live::from_update(&json!({ "type": "chunk", "message_id": 5, "seq": 7, "text": "Hi" })),
            Some(Live::Chunk {
                message_id: 5,
                seq: Some(7),
                text: "Hi".into()
            })
        );
        assert_eq!(
            Live::from_update(
                &json!({ "type": "chunk", "message_id": 5, "seq": -1, "text": "Hi" })
            ),
            None
        );
        assert_eq!(
            Live::from_update(&json!({ "type": "anything" })),
            Some(Live::Changed)
        );
        assert_eq!(Live::from_update(&json!({})), Some(Live::Changed));
        assert_eq!(Live::from_update(&json!({ "type": "chunk" })), None);
        assert_eq!(
            Live::from_update(&json!({ "type": "unsupported" })),
            Some(Live::Unsupported)
        );
    }

    #[test]
    fn parses_the_webs_logos_and_icons_without_failing_on_them() {
        let logo = |path: &str, mono: bool| {
            Some(Asset {
                path: path.into(),
                monochrome: mono,
            })
        };
        let transcript: Transcript = parse(json!({
            "chat": { "number": 42, "model": { "id": 12, "name": "Gemini", "logo": { "path": "/assets/providers/gemini-1.svg", "monochrome": false } } },
            "entries": [
                { "kind": "user", "id": 1, "content": "Hi", "attachments": [
                    { "filename": "Q3.pdf", "byte_size": 4, "content_type": "application/pdf",
                      "icon": { "path": "/assets/mimetypes/application-pdf-a.svg", "monochrome": false } }
                ] },
                { "kind": "activity", "id": 2, "title": "Searched Drive", "services": ["Drive", "Carmine's MacBook", "Odd"],
                  "logos": [ { "path": "/assets/providers/google_drive-7.svg", "monochrome": false }, null, { "path": "https://evil.example/x.svg" } ],
                  "steps": [ { "summary": "Searched", "logo": { "path": "/assets/providers/google_drive-7.svg", "monochrome": false } }, { "summary": "Read", "logo": null } ] },
                { "kind": "assistant", "id": 3, "content": "It's €40k", "sources": [
                    { "title": "Q3 plan.pdf", "url": "https://drive.google.com/x", "icon": { "path": "/assets/mimetypes/application-pdf-a.svg", "monochrome": false } },
                    { "title": "Review", "url": "https://news.example/r", "icon": null },
                    { "title": "Odd", "icon": "a string, not an image" }
                ] }
            ],
            "approvals": [ { "id": 31, "service": "Slack", "logo": { "path": "/assets/providers/slack-0.svg", "monochrome": false }, "decidable": false, "waiting_for": "Ada" } ],
            "questions": [ { "id": 32, "service": "Mine", "logo": null, "decidable": false, "waiting_for": "Ada" } ]
        }))
        .unwrap();
        assert_eq!(
            transcript.chat.model.unwrap().logo,
            logo("/assets/providers/gemini-1.svg", false)
        );
        let Entry::User { attachments, .. } = &transcript.entries[0] else {
            panic!("a question");
        };
        assert_eq!(
            attachments[0].icon,
            logo("/assets/mimetypes/application-pdf-a.svg", false)
        );
        let Entry::Activity { logos, steps, .. } = &transcript.entries[1] else {
            panic!("an activity");
        };
        assert_eq!(
            logos,
            &[
                logo("/assets/providers/google_drive-7.svg", false),
                None,
                None
            ],
            "one per service; off the server, none"
        );
        assert!(steps[0].logo.is_some() && steps[1].logo.is_none());
        let Entry::Assistant { sources, .. } = &transcript.entries[2] else {
            panic!("an answer");
        };
        assert!(sources[0].icon.is_some());
        assert_eq!(
            (sources[1].icon.clone(), sources[2].icon.clone()),
            (None, None)
        );
        assert_eq!(
            transcript.approvals[0].logo,
            logo("/assets/providers/slack-0.svg", false)
        );
        assert_eq!(transcript.questions[0].logo, None);

        let models: Models = parse(json!({ "models": [ { "id": 15, "name": "GPT", "logo": { "path": "/assets/providers/openai-9.svg", "monochrome": true } } ] })).unwrap();
        assert_eq!(
            models.models[0].logo,
            logo("/assets/providers/openai-9.svg", true)
        );
        let uploaded: Uploaded = parse(json!({ "signed_id": "s", "filename": "a.txt", "byte_size": 1, "content_type": "text/plain",
            "icon": { "path": "/assets/mimetypes/unknown.svg", "monochrome": false } })).unwrap();
        assert_eq!(uploaded.icon, logo("/assets/mimetypes/unknown.svg", false));
    }

    #[test]
    fn parses_what_the_web_shows_around_the_list() {
        let list: ChatList = parse(json!({
            "account": { "name": "Plenty UG", "logo": null },
            "user": { "name": "Carmine Paolino", "avatar": null },
            "credits": { "left": 1250, "capacity": 5000, "running_low": false },
            "locked_reason": null,
            "chats": [{ "number": 42, "title": "Q3 budget", "state": "idle", "project": { "id": 7, "name": "Launch" } }],
            "projects": [
                { "id": 3, "name": "HQ", "icon": "buildings", "hq": true, "all_access": false, "url": "https://x/projects/1" },
                { "id": 7, "name": "Launch", "icon": "folder-simple", "hq": false, "all_access": false, "url": "https://x/projects/4" }
            ],
            "pins": { "projects": [7], "chats": [42, 17] },
            "links": {
                "new_chat": "https://x/chats/new",
                "chats": "https://x/chats",
                "projects": "https://x/projects",
                "settings": [
                    { "tab": "account", "label": "Account", "icon": "user-circle", "url": "https://x/settings?tab=account" },
                    { "tab": "usage", "label": "Usage", "icon": "chart-bar", "url": "https://x/settings?tab=usage" }
                ]
            }
        }))
        .unwrap();
        assert_eq!(list.user.name, "Carmine Paolino");
        assert_eq!(list.credits.as_ref().unwrap().capacity, 5000);
        assert!(list.projects[0].hq);
        assert_eq!(list.projects[1].icon.as_deref(), Some("folder-simple"));
        assert_eq!(list.chats[0].project.as_ref().unwrap().icon, None);
        assert_eq!(list.pins.chats, [42, 17]);
        assert_eq!(list.pins.projects, [7]);
        assert_eq!(list.links.settings[1].label, "Usage");
        assert_eq!(list.links.projects.as_deref(), Some("https://x/projects"));

        // An older server's list has none of it.
        let old: ChatList =
            parse(json!({ "chats": [], "projects": [{ "id": 1, "name": "P" }] })).unwrap();
        assert_eq!(
            (old.credits, old.pins, old.links),
            (None, Pins::default(), Links::default())
        );
    }

    #[test]
    fn reads_the_lists_live_changes() {
        let read =
            |update: Value| ListLive::from_event(&json!({ "event": "chats", "update": update }));
        let Some(ListLive::Chat(chat)) = read(json!({
            "event": "chat",
            "chat": { "number": 42, "title": "Q3 budget", "state": "processing", "updated_at": "2026-10-02T09:05:00Z", "created_at": "x" }
        })) else {
            panic!("a chat");
        };
        assert_eq!((chat.number, chat.processing()), (42, true));
        assert_eq!(
            read(json!({ "event": "removed", "number": 42 })),
            Some(ListLive::Removed(42))
        );
        assert_eq!(
            read(
                json!({ "event": "projects", "projects": [{ "id": 7, "name": "Launch", "icon": "folder-simple" }] })
            ),
            Some(ListLive::Projects(vec![Project {
                id: 7,
                name: "Launch".into(),
                icon: Some("folder-simple".into()),
                ..Project::default()
            }]))
        );
        let Some(ListLive::Account(account)) = read(json!({
            "event": "account",
            "account": { "name": "Plenty UG", "logo": null },
            "user": { "name": "Carmine", "avatar": null },
            "credits": { "left": 1180.5, "capacity": 5000, "running_low": false },
            "locked_reason": null
        })) else {
            panic!("the account");
        };
        assert_eq!(account.user.name, "Carmine");
        assert_eq!(account.credits.unwrap().left, 1181);
        for (note, live) in [
            ("watching", ListLive::Watching),
            ("refused", ListLive::Refused),
            ("offline", ListLive::Offline),
            ("unsupported", ListLive::Unsupported),
        ] {
            assert_eq!(read(json!({ "event": note })), Some(live));
        }
        assert_eq!(read(json!({ "event": "something_new" })), None);
        assert_eq!(read(json!({ "event": "removed" })), None, "no number");
        assert_eq!(ListLive::from_event(&json!({ "event": "heartbeat" })), None);
        assert_eq!(
            ListLive::from_event(&json!({ "event": "lagged", "missed": 3 })),
            Some(ListLive::Missed)
        );
    }

    #[test]
    fn missed_events_mean_reading_the_chat_again() {
        assert_eq!(
            Live::from_event(&json!({ "event": "lagged", "missed": 3 })),
            Some(Live::Changed)
        );
        assert_eq!(
            Live::from_event(&json!({ "event": "chat", "update": { "type": "refused" } })),
            Some(Live::Refused)
        );
        assert_eq!(Live::from_event(&json!({ "event": "heartbeat" })), None);
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn code_output_keeps_literal_fences_and_failures() {
        let code: CodeRun = serde_json::from_value(json!({"language":"python", "source":"print('```')", "stdout":"```\n[not a link](https://example.com)", "stderr":"failed", "exit_code":1, "error":"Timed out"})).unwrap();
        let text = code.markdown();
        assert!(text.starts_with("````python\n"));
        assert!(text.contains("````text\n```\n[not a link]"));
        assert!(text.contains("Exit code: 1"));
        assert!(text.contains("Timed out"));
    }

    #[test]
    fn change_previews_keep_each_body_and_escape_sheet_cells() {
        for (body, expected) in [
            (json!({"kind":"text","text":"hello"}), "hello"),
            (
                json!({"kind":"diff","lines":[{"kind":"removed","text":"-old"},{"kind":"added","text":"+new"}]}),
                "-old\n+new",
            ),
            (
                json!({"kind":"replace","old_text":"before","new_text":"after"}),
                "**With**",
            ),
            (json!({"kind":"document","markdown":"# Plan"}), "# Plan"),
            (
                json!({"kind":"sheets","sheets":[{"name":"Budget","header":true,"rows":[["Item","Total"],["[link]|<tag>","42"]]}]}),
                "\\[link\\]\\|&lt;tag&gt;",
            ),
            (
                json!({"kind":"sheets","sheets":[{"name":"Data","rows":[["42"],["17","20"]]}]}),
                "Column 2",
            ),
            (
                json!({"kind":"sheets","sheets":[{"name":"Empty","header":true,"rows":[]}]}),
                "### Empty",
            ),
            (json!({"kind":"future"}), "Review this change on the web"),
        ] {
            let preview: PreviewBody = serde_json::from_value(body).unwrap();
            assert!(
                preview.markdown().contains(expected),
                "{}",
                preview.markdown()
            );
        }
    }

    #[test]
    fn download_names_never_choose_a_directory() {
        for (name, expected) in [
            ("/tmp/budget.csv", "budget.csv"),
            ("C:\\Users\\budget.csv", "budget.csv"),
            ("..", "download"),
            ("", "download"),
        ] {
            assert_eq!(
                Attachment {
                    filename: name.into(),
                    ..Default::default()
                }
                .save_name(),
                expected
            );
        }
    }
}
