//! Coding bots on this computer (docs/coding-computers.md): the coding
//! agents' sign-ins a bot's commands get ([`Agent`]: Claude Code, Codex,
//! OpenCode, Gemini CLI and pi),
//! the repositories Desktop mirrors for them, and what a bot's `roots` says
//! is ready.
//!
//! A sign-in is the agent's own, where it keeps it on this computer
//! ([`Login`]): a bot whose loan allows that agent gets those paths in its
//! run's sandbox, to read and write as the agent does when it refreshes its
//! tokens, and nothing else of the home folder. Any token Desktop can read
//! there is taken out of the run's output ([`Redactor`]).
//! Mirrors live in Desktop's data folder, which no command writes, so
//! fetching one with the person's credentials runs nothing a bot wrote.

pub mod detect;
pub mod prepare;
pub mod repositories;
pub mod setup;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::secrets::SecretStore;
use crate::config::{CodingConfig, Config, Root};
use crate::paths::Paths;

/// A coding agent a bot's runs can use with this computer's sign-in. Each
/// has its own switch on a loan, its own program and its own folder in the
/// home folder; what the sandbox does with them is the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    ClaudeCode,
    Codex,
    OpenCode,
    Gemini,
    Pi,
}

impl Agent {
    pub const ALL: [Self; 5] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::OpenCode,
        Self::Gemini,
        Self::Pi,
    ];

    /// Its name on the wire: `claude_code`, `codex`, `opencode`, `gemini`,
    /// `pi`.
    pub fn key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Pi => "pi",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|agent| agent.key() == key)
    }

    /// Why it can't run in a bot's sandbox on this system, if it can't, and
    /// Desktop doesn't loosen the sandbox for it. On macOS, Codex stops at
    /// "Failed to synchronize managed preferences" in Seatbelt, and OpenCode
    /// at its own server, which must listen on a local port Seatbelt doesn't
    /// give a run.
    pub fn unavailable(self) -> Option<&'static str> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        match self {
            Self::Codex => Some("Codex can't run in this computer's sandbox on macOS yet"),
            Self::OpenCode => Some("OpenCode can't run in this computer's sandbox on macOS yet"),
            _ => None,
        }
    }

    /// What people call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini CLI",
            Self::Pi => "pi",
        }
    }

    /// The program, as a person types it.
    pub fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Pi => "pi",
        }
    }

    /// Its folder in the home folder, as a path relative to it with `/`
    /// between folders: OpenCode's is `.local/share/opencode` everywhere,
    /// Windows too.
    pub fn config_dir(self) -> &'static str {
        match self {
            Self::ClaudeCode => CONFIG_DIR,
            Self::Codex => CODEX_DIR,
            Self::OpenCode => OPENCODE_DIR,
            Self::Gemini => GEMINI_DIR,
            Self::Pi => PI_DIR,
        }
    }

    /// That folder in the home folder `home`.
    pub fn config_dir_in(self, home: &Path) -> PathBuf {
        self.config_dir()
            .split('/')
            .fold(home.to_path_buf(), |path, part| path.join(part))
    }

    /// The variable that names its sign-in for it on Windows, which can't
    /// link one into a run's home without Developer Mode, and its value for
    /// the folder `dir`: the folder itself, or for OpenCode the database in
    /// it, which holds its credentials.
    pub fn config_variable(self, dir: &Path) -> (&'static str, PathBuf) {
        match self {
            Self::ClaudeCode => ("CLAUDE_CONFIG_DIR", dir.to_path_buf()),
            Self::Codex => ("CODEX_HOME", dir.to_path_buf()),
            Self::OpenCode => ("OPENCODE_DB", dir.join(OPENCODE_DB_FILE)),
            // The folder that holds its `.gemini`, which it reads as its home.
            Self::Gemini => (
                "GEMINI_CLI_HOME",
                dir.parent()
                    .map_or_else(|| dir.to_path_buf(), Path::to_path_buf),
            ),
            Self::Pi => ("PI_CODING_AGENT_DIR", dir.to_path_buf()),
        }
    }

    /// What a run's output says where its token was.
    pub fn redacted(self) -> &'static str {
        match self {
            Self::ClaudeCode => REDACTED,
            Self::Codex => CODEX_REDACTED,
            Self::OpenCode => OPENCODE_REDACTED,
            Self::Gemini => GEMINI_REDACTED,
            Self::Pi => PI_REDACTED,
        }
    }

    /// Whether `loan` allows it.
    pub fn allowed(self, loan: &crate::config::BotLoan) -> bool {
        match self {
            Self::ClaudeCode => loan.claude_code,
            Self::Codex => loan.codex,
            Self::OpenCode => loan.opencode,
            Self::Gemini => loan.gemini,
            Self::Pi => loan.pi,
        }
    }

    /// Let `loan` use it, or stop.
    pub fn allow(self, loan: &mut crate::config::BotLoan, allowed: bool) {
        match self {
            Self::ClaudeCode => loan.claude_code = allowed,
            Self::Codex => loan.codex = allowed,
            Self::OpenCode => loan.opencode = allowed,
            Self::Gemini => loan.gemini = allowed,
            Self::Pi => loan.pi = allowed,
        }
    }

    /// How to sign in, for the person.
    pub fn sign_in_hint(self) -> &'static str {
        match self {
            Self::ClaudeCode => "run `claude` once and sign in",
            Self::Codex => "run `codex login`",
            Self::OpenCode => "run `opencode auth login`",
            Self::Gemini => "run `gemini` once and sign in",
            Self::Pi => "run `pi`, then /login",
        }
    }

    /// What a run allowed it gets in its environment: no self-updates, as
    /// a run can't write its program's folder.
    pub fn environment(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::ClaudeCode => &[("DISABLE_AUTOUPDATER", "1")],
            Self::Codex => &[],
            Self::OpenCode => &[("OPENCODE_DISABLE_AUTOUPDATE", "1")],
            // Its own sandbox can't start inside this one, which already
            // holds it to the folder.
            Self::Gemini => &[("GEMINI_SANDBOX", "false")],
            Self::Pi => &[("PI_SKIP_VERSION_CHECK", "1")],
        }
    }

    /// The variables that give it an API key, which a folder's own
    /// environment (`cww roots env`) can set, and which then bill instead.
    pub fn api_key_variables(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCode => &["ANTHROPIC_API_KEY"],
            Self::Codex => &["OPENAI_API_KEY"],
            Self::OpenCode => &[
                "OPENCODE_API_KEY",
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "OPENROUTER_API_KEY",
            ],
            Self::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::Pi => &[
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "GEMINI_API_KEY",
                "OPENROUTER_API_KEY",
            ],
        }
    }

    /// One word from it, run as a bot's run starts it: proves it's found,
    /// signed in, and reaches its provider from the sandbox.
    pub fn ready_probe(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude -p 'Reply with the word ready.' --max-turns 1",
            Self::Codex => {
                "codex exec --sandbox danger-full-access --skip-git-repo-check 'Reply with the word ready.'"
            }
            Self::OpenCode => "opencode run --standalone --auto 'Reply with the word ready.'",
            Self::Gemini => {
                "gemini -p 'Reply with the word ready.' --approval-mode yolo --skip-trust"
            }
            Self::Pi => "pi -p --no-session 'Reply with the word ready.'",
        }
    }
}

/// Every one of Desktop's agents with a loan's switch cleared: what a folder
/// that stops commands does to its loans.
pub fn disallow_all(loan: &mut crate::config::BotLoan) {
    Agent::ALL
        .into_iter()
        .for_each(|agent| agent.allow(loan, false));
}

/// Who pays for a run of an agent, as its sign-in says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    /// The person's subscription: Claude or ChatGPT.
    Plan,
    /// An API key, billed by use.
    ApiKey,
    /// It couldn't tell (a locked keychain, say).
    #[default]
    Unknown,
}

/// Codex's folder in the home folder: its settings, sessions and sign-in
/// (`auth.json`).
pub const CODEX_DIR: &str = ".codex";
/// Where Codex keeps its tokens, in [`CODEX_DIR`].
pub const CODEX_AUTH_FILE: &str = "auth.json";
/// What a run's output says where a Codex token was.
pub const CODEX_REDACTED: &str = "[codex-token]";

/// OpenCode's data folder in the home folder (`xdg-basedir`'s, on every
/// system): its database, whose `credential` table holds its sign-ins and
/// keys, with its sessions, logs and snapshots.
pub const OPENCODE_DIR: &str = ".local/share/opencode";
/// OpenCode's database, in [`OPENCODE_DIR`].
pub const OPENCODE_DB_FILE: &str = "opencode.db";
/// What a run's output says where an OpenCode token was.
pub const OPENCODE_REDACTED: &str = "[opencode-token]";

/// Gemini CLI's folder in the home folder: its settings, history and
/// sign-in (`oauth_creds.json`, or an API key in its `.env`).
pub const GEMINI_DIR: &str = ".gemini";
/// Where Gemini CLI keeps a Google sign-in's tokens, in [`GEMINI_DIR`].
pub const GEMINI_OAUTH_FILE: &str = "oauth_creds.json";
/// What a run's output says where a Gemini CLI token was.
pub const GEMINI_REDACTED: &str = "[gemini-token]";
/// pi's folder in the home folder: its settings, sessions, extensions and
/// sign-ins (`auth.json`).
pub const PI_DIR: &str = ".pi/agent";
/// Where pi keeps its sign-ins and keys, in [`PI_DIR`].
pub const PI_AUTH_FILE: &str = "auth.json";
/// What a run's output says where a pi token was.
pub const PI_REDACTED: &str = "[pi-token]";

/// Claude Code's folder in the home folder: its settings, sessions and, on
/// Linux and Windows, its sign-in (`.credentials.json`).
pub const CONFIG_DIR: &str = ".claude";
/// Claude Code's file in the home folder: its account and preferences.
pub const CONFIG_FILE: &str = ".claude.json";
/// Where Claude Code keeps its tokens on Linux and Windows, in [`CONFIG_DIR`].
pub const CREDENTIALS_FILE: &str = ".credentials.json";
/// The login keychain item Claude Code keeps its tokens in on macOS.
pub const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// Most repositories a bot's `roots` names in one folder.
pub const MAX_REPOSITORIES_LISTED: usize = 20;
/// What a run's output says where a token was.
pub const REDACTED: &str = "[claude-code-token]";
/// The secret store's name for the token `cww coding sign-in` kept, before
/// runs used Claude Code's own sign-in. Only ever deleted now.
pub const LEGACY_TOKEN_NAME: &str = "claude-code-token";

/// A value that never prints: not in `Debug`, logs or errors.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// An agent's sign-in on this computer, where the agent itself keeps it for
/// whoever runs it here. Claude Code: `~/.claude` and `~/.claude.json` in
/// the home folder, and on macOS the login keychain's
/// `Claude Code-credentials`. Codex: `~/.codex`, with its tokens in
/// `auth.json`. Only those standard places: never another folder
/// `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or an account switcher points at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Login {
    /// `~/.claude` or `~/.codex`, where it really is, if it's there.
    pub dir: Option<PathBuf>,
    /// `~/.claude.json`, where it really is, if it's there.
    pub file: Option<PathBuf>,
    /// macOS: the keychain holds the tokens, so the run's sandbox reaches
    /// the keychain.
    pub keychain: bool,
    /// The tokens Desktop could read, to take out of a run's output.
    pub secrets: Vec<Secret>,
    /// Whose bill a run goes on, as the sign-in says.
    pub billing: Billing,
    /// What the agent's own settings, outside the folder a run gets, tell a
    /// run's environment: OpenCode's chosen model, and nothing else of them.
    pub env: Vec<(String, String)>,
}

impl Login {
    /// `agent`'s sign-in in the home folder `home`, as it is now.
    pub fn find_for(agent: Agent, home: &Path) -> Self {
        match agent {
            Agent::ClaudeCode => Self::find(home),
            Agent::Codex => Self::find_codex(home),
            Agent::OpenCode => Self::find_opencode(home),
            Agent::Gemini => Self::find_gemini(home),
            Agent::Pi => Self::find_pi(home),
        }
    }

    /// An agent's folder in `home`, where it really is: never one a link
    /// makes something else (`~/.ssh`), or broad (the home folder, a drive).
    fn own_dir(agent: Agent, home: &Path) -> Option<PathBuf> {
        let wanted = agent.config_dir().rsplit('/').next().unwrap_or_default();
        crate::roots::canonical(&agent.config_dir_in(home))
            .ok()
            .filter(|p| p.is_dir())
            .filter(|p| p.file_name().is_some_and(|name| is_named(name, wanted)))
            .filter(|p| matches!(crate::roots::too_broad(p), Ok(None)))
    }

    /// Gemini CLI's: its folder, with a Google sign-in's tokens in
    /// `oauth_creds.json`, or an API key in its `.env`. One kept in the
    /// system's keychain (`GEMINI_FORCE_ENCRYPTED_FILE_STORAGE`) reads as not
    /// signed in here.
    fn find_gemini(home: &Path) -> Self {
        let dir = Self::own_dir(Agent::Gemini, home);
        let oauth = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(GEMINI_OAUTH_FILE)))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_default();
        let mut secrets = long_strings(&oauth);
        let keys: Vec<Secret> = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(".env")))
            .map(|text| env_values(&text, Agent::Gemini.api_key_variables()))
            .unwrap_or_default();
        let billing = if !secrets.is_empty() {
            Billing::Plan
        } else if !keys.is_empty() {
            Billing::ApiKey
        } else {
            Billing::Unknown
        };
        secrets.extend(keys);
        Self {
            dir,
            file: None,
            keychain: false,
            secrets,
            billing,
            env: Vec::new(),
        }
    }

    /// pi's: its folder, whose `auth.json` holds a sign-in per provider
    /// (`{"type":"oauth",...}`) or a key (`{"type":"api_key","key":...}`).
    fn find_pi(home: &Path) -> Self {
        let dir = Self::own_dir(Agent::Pi, home);
        let auth = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(PI_AUTH_FILE)))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_default();
        let entries: Vec<&Value> = auth
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .collect();
        let billing = if entries.iter().any(|e| e["type"] == "oauth") {
            Billing::Plan
        } else if entries.iter().any(|e| e["type"] == "api_key") {
            Billing::ApiKey
        } else {
            Billing::Unknown
        };
        Self {
            dir,
            file: None,
            keychain: false,
            secrets: long_strings(&auth),
            billing,
            env: Vec::new(),
        }
    }

    /// OpenCode's: its data folder, whose database holds its credentials.
    /// Its config folder (`~/.config/opencode`, with its plugins and the
    /// background service's password) is never lent: a run uses its own.
    fn find_opencode(home: &Path) -> Self {
        let dir = Self::own_dir(Agent::OpenCode, home);
        let credentials = dir
            .as_ref()
            .map(|d| opencode_credentials(d))
            .unwrap_or_default();
        let billing = if credentials.iter().any(|c| c["type"] == "oauth") {
            Billing::Plan
        } else if credentials.iter().any(|c| c["type"] == "key") {
            Billing::ApiKey
        } else {
            Billing::Unknown
        };
        let secrets = long_strings(&Value::Array(credentials));
        let env = opencode_model(home)
            .map(|model| {
                vec![(
                    "OPENCODE_CONFIG_CONTENT".to_string(),
                    json!({ "model": model }).to_string(),
                )]
            })
            .unwrap_or_default();
        Self {
            dir,
            file: None,
            keychain: false,
            secrets,
            billing,
            env,
        }
    }

    /// Codex's: its folder, whose `auth.json` holds its tokens or an API key.
    fn find_codex(home: &Path) -> Self {
        let dir = crate::roots::canonical(&home.join(CODEX_DIR))
            .ok()
            .filter(|p| p.is_dir())
            .filter(|p| p.file_name().is_some_and(|name| is_named(name, CODEX_DIR)))
            .filter(|p| matches!(crate::roots::too_broad(p), Ok(None)));
        let auth = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(CODEX_AUTH_FILE)))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_default();
        Self {
            dir,
            file: None,
            keychain: false,
            secrets: codex_secrets(&auth),
            billing: codex_billing(&auth),
            env: Vec::new(),
        }
    }

    /// The sign-in in the home folder `home`, as it is now. The keychain is
    /// asked only for the person's own home, whose keychain it is.
    pub fn find(home: &Path) -> Self {
        let real = |name: &str| {
            crate::roots::canonical(&home.join(name))
                .ok()
                .filter(|p| p.exists())
        };
        // Never a folder a link makes broad (the home folder, a drive), or
        // one that isn't Claude Code's (a link to `~/.ssh`): a run writes
        // all of it. A link to a folder named `.claude` elsewhere (dotfiles)
        // is fine.
        let dir = real(CONFIG_DIR)
            .filter(|p| p.is_dir())
            .filter(|p| p.file_name().is_some_and(|name| is_named(name, CONFIG_DIR)))
            .filter(|p| matches!(crate::roots::too_broad(p), Ok(None)));
        // Only the file itself, never another a link leads to.
        let file = real(CONFIG_FILE).filter(|p| {
            p.is_file()
                && crate::roots::canonical(home)
                    .is_ok_and(|home| p.as_path() == home.join(CONFIG_FILE))
        });
        let credentials = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(CREDENTIALS_FILE)));
        let mut billing = credentials
            .as_deref()
            .map_or(Billing::Unknown, claude_billing);
        let mut secrets = credentials.map(|text| tokens_in(&text)).unwrap_or_default();
        let keychain = cfg!(target_os = "macos")
            && crate::paths::home_dir()
                .ok()
                .and_then(|h| crate::roots::canonical(&h).ok())
                == crate::roots::canonical(home).ok()
            && keychain_item(false).is_some();
        if keychain && let Some(text) = keychain_item(true) {
            secrets.extend(tokens_in(&text));
            if billing == Billing::Unknown {
                billing = claude_billing(&text);
            }
        }
        Self {
            dir,
            file,
            keychain,
            secrets,
            billing,
            env: Vec::new(),
        }
    }

    /// Whether the agent is signed in here: tokens in its folder, or in
    /// the keychain.
    pub fn signed_in(&self) -> bool {
        self.keychain || !self.secrets.is_empty()
    }

    /// The paths a run reads and writes for it.
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.dir.iter().chain(self.file.iter())
    }

    /// Without any path that holds Desktop's own folders or a shared
    /// folder, which a run must never write through it.
    fn narrowed(mut self, config: &Config, paths: &Paths) -> Self {
        let holds = |outer: &PathBuf| {
            let inner = |p: &Path| crate::roots::canonical(p).is_ok_and(|p| p.starts_with(outer));
            paths.all_dirs().iter().any(|own| inner(own))
                || config.roots.iter().any(|r| inner(&r.path))
        };
        if self.dir.as_ref().is_some_and(holds) {
            self.dir = None;
        }
        self
    }
}

/// Whether a folder is named as the agent's: on Windows, whose file names
/// ignore case, `.Claude` is that folder too.
fn is_named(name: &std::ffi::OsStr, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}

/// Claude Code's credentials say a subscription (`pro`, `max`, `team`...) by
/// its `subscriptionType`; a sign-in with a Console account has none and
/// bills the API.
fn claude_billing(text: &str) -> Billing {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Billing::Unknown;
    };
    match value["claudeAiOauth"]["subscriptionType"].as_str() {
        Some(kind) if !kind.is_empty() => Billing::Plan,
        _ if value["claudeAiOauth"].is_object() => Billing::ApiKey,
        _ => Billing::Unknown,
    }
}

/// Codex's `auth.json`: an `OPENAI_API_KEY`, or a ChatGPT sign-in's `tokens`.
fn codex_billing(auth: &Value) -> Billing {
    if auth["OPENAI_API_KEY"]
        .as_str()
        .is_some_and(|k| !k.is_empty())
    {
        Billing::ApiKey
    } else if auth["tokens"].is_object() {
        Billing::Plan
    } else {
        Billing::Unknown
    }
}

/// Every token in Codex's `auth.json`: its API key, and its sign-in's
/// tokens, whatever they're called. Its account's id is no secret.
fn codex_secrets(auth: &Value) -> Vec<Secret> {
    let key = auth["OPENAI_API_KEY"].as_str();
    let tokens = auth["tokens"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.as_str() != "account_id")
        .filter_map(|(_, value)| value.as_str());
    key.into_iter()
        .chain(tokens)
        .filter(|token| token.len() >= 20)
        .map(|token| Secret::new(token.to_string()))
        .collect()
}

/// Most of OpenCode's database Desktop reads for its credentials, per file.
const MAX_OPENCODE_DB_BYTES: u64 = 256 * 1024 * 1024;

/// Each credential's value in OpenCode's database and its write-ahead log:
/// the JSON it stores (`{"type":"key","key":...}`, `{"type":"oauth",
/// "access":...,"refresh":...}`, `{"type":"external",...}`), found in the
/// files' bytes, where SQLite keeps a row's text whole. Without opening it
/// as a database: a run allowed OpenCode can write it, so it's only read,
/// a regular file, at most [`MAX_OPENCODE_DB_BYTES`] of each. An old value a
/// page still holds is found too, which only takes more out of output.
fn opencode_credentials(dir: &Path) -> Vec<Value> {
    const KINDS: [&[u8]; 3] = [
        br#""type":"key""#,
        br#""type":"oauth""#,
        br#""type":"external""#,
    ];
    let mut found = Vec::new();
    for name in [
        OPENCODE_DB_FILE.to_string(),
        format!("{OPENCODE_DB_FILE}-wal"),
    ] {
        let Some(bytes) = read_regular(&dir.join(name), MAX_OPENCODE_DB_BYTES) else {
            continue;
        };
        let mut at = 0;
        while let Some(offset) = bytes[at..].windows(7).position(|w| w == br#""type":"#) {
            let mark = at + offset;
            at = mark + 1;
            if !KINDS.iter().any(|kind| bytes[mark..].starts_with(kind)) {
                continue;
            }
            // The object it's in: the nearest `{` before it that starts one
            // whose own `type` is this, a credential's value being at most a
            // few KiB.
            let floor = mark.saturating_sub(16 * 1024);
            let value = (floor..mark)
                .rev()
                .filter(|&i| bytes[i] == b'{')
                .take(16)
                .find_map(|start| {
                    let value = serde_json::Deserializer::from_slice(&bytes[start..])
                        .into_iter::<Value>()
                        .next()?
                        .ok()?;
                    matches!(value["type"].as_str(), Some("key" | "oauth" | "external"))
                        .then_some(value)
                });
            if let Some(value) = value
                && !found.contains(&value)
            {
                found.push(value);
            }
        }
    }
    found
}

/// The model the person chose in OpenCode's own config
/// (`~/.config/opencode/opencode.json` or `.jsonc`, its top-level `model`),
/// which a run uses as their own `opencode run` would. Only that: the
/// config folder holds plugins and the background service's password, and
/// is never lent.
fn opencode_model(home: &Path) -> Option<String> {
    let dir = home.join(".config").join("opencode");
    ["opencode.json", "opencode.jsonc", "config.json"]
        .iter()
        .filter_map(|name| read_small(&dir.join(name)))
        .find_map(|text| {
            let value: Value = serde_json::from_str(&without_comments(&text)).ok()?;
            let model = value["model"].as_str()?.to_string();
            let valid = !model.is_empty()
                && model.len() <= 200
                && model.contains('/')
                && model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/._-:@#+".contains(&b));
            valid.then_some(model)
        })
}

/// JSON with comments (`//`, `/* */`) and trailing commas made plain JSON,
/// leaving strings as they are.
fn without_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            out.push(b);
            if b == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 1;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
            out.push(b);
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            out.push(b);
        }
        i += 1;
    }
    without_trailing_commas(&out)
}

/// `bytes`, JSON but for commas before a `}` or `]`, without them.
fn without_trailing_commas(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\\' && i + 1 < bytes.len() {
                out.extend_from_slice(&bytes[i..i + 2]);
                i += 2;
                continue;
            }
            in_string = b != b'"';
        } else if b == b'"' {
            in_string = true;
        } else if b == b',' {
            let next = bytes[i + 1..].iter().find(|c| !c.is_ascii_whitespace());
            if matches!(next, Some(b'}') | Some(b']')) {
                i += 1;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Every string of at least 20 characters in `value`, once each: a sign-in
/// file's tokens and keys, whatever they're called. What else is that long
/// (an account's id, a provider's name) is only taken out of output too.
fn long_strings(value: &Value) -> Vec<Secret> {
    let mut found: Vec<Secret> = Vec::new();
    strings_in(value, &mut |text| {
        if text.len() >= 20 && !found.iter().any(|s| s.expose() == text) {
            found.push(Secret::new(text.to_string()));
        }
    });
    found
}

/// The values a `.env` file gives the variables `names`.
fn env_values(text: &str, names: &[&str]) -> Vec<Secret> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches("export ").trim_start();
            let (name, value) = line.split_once('=')?;
            names.contains(&name.trim()).then(|| {
                value
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string()
            })
        })
        .filter(|value| value.len() >= 20)
        .map(Secret::new)
        .collect()
}

/// Every string in `value`, wherever it is.
fn strings_in(value: &Value, each: &mut impl FnMut(&str)) {
    match value {
        Value::String(s) => each(s),
        Value::Array(items) => items.iter().for_each(|v| strings_in(v, each)),
        Value::Object(map) => map.values().for_each(|v| strings_in(v, each)),
        _ => {}
    }
}

/// A regular file's bytes, at most `limit` of them; never a FIFO or a link's
/// target that isn't a regular file.
fn read_regular(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Largest credentials file Desktop reads: Claude Code's is under 1 KiB.
const MAX_CREDENTIALS_BYTES: u64 = 64 * 1024;

/// A regular file's text, if it's at most [`MAX_CREDENTIALS_BYTES`]. A run
/// allowed the sign-in can replace the file, so never a FIFO, which would
/// hold up whoever starts runs, or a huge one.
fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_CREDENTIALS_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_CREDENTIALS_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_CREDENTIALS_BYTES).then_some(text)
}

/// Every token of Anthropic's in Claude Code's credentials (`accessToken`,
/// `refreshToken`), wherever it is in the JSON.
fn tokens_in(text: &str) -> Vec<Secret> {
    fn walk(value: &Value, found: &mut Vec<Secret>) {
        match value {
            Value::String(s) if s.starts_with("sk-ant-") && s.len() >= 20 => {
                found.push(Secret::new(s.clone()));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            Value::Object(map) => map.values().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        walk(&value, &mut found);
    }
    found
}

/// The keychain item Claude Code keeps its tokens in, or `None`: with
/// `password`, its tokens, read as Claude Code reads them (`security
/// find-generic-password -w`), which a locked keychain (an SSH session's)
/// refuses; without, only that it's there, which needs no unlocking.
fn keychain_item(password: bool) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        let mut args = vec!["find-generic-password", "-s", KEYCHAIN_SERVICE];
        if password {
            args.push("-w");
        }
        let mut child = Command::new("/usr/bin/security")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // As Claude Code does, give up after two seconds: a locked keychain
        // waits on a dialog.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        output.status.success().then(|| text.trim().to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = password;
        None
    }
}

/// Forget the token `cww coding sign-in` kept, everywhere it can be, and
/// the config's note of it: runs now use Claude Code's own sign-in.
pub fn forget_legacy_token(config: &mut Config, paths: &Paths) -> anyhow::Result<()> {
    let store = SecretStore::for_config(config, paths);
    store.delete(LEGACY_TOKEN_NAME)?;
    if store.in_keychain() {
        SecretStore::Keyring.delete(LEGACY_TOKEN_NAME)?;
    }
    SecretStore::File(paths.secrets_file()).delete(LEGACY_TOKEN_NAME)?;
    config.coding.legacy_token = false;
    Ok(())
}

/// Whether an agent is ready for a bot's commands in a folder, as the
/// config says. A run checks again, with the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Ready,
    /// Setup found no program for it here.
    NotInstalled,
    /// It isn't signed in on this computer. Only `cww coding` and preparing
    /// a repository say so: the server's `roots` comes from the config, which
    /// doesn't know, and a run then hears it from the agent itself.
    NotSignedIn,
    /// Not lent to this bot in this folder.
    NotAllowed,
    /// The folder's commands have no network, which the agents need.
    NoNetwork,
}

impl AgentState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NotInstalled => "not_installed",
            Self::NotSignedIn => "not_signed_in",
            Self::NotAllowed => "not_allowed",
            Self::NoNetwork => "no_network",
        }
    }

    /// What to tell the person, in a sentence.
    pub fn sentence(self, agent: Agent) -> String {
        let name = agent.name();
        match self {
            Self::Ready => format!("{name} is ready"),
            Self::NotInstalled => format!("{name} isn't installed"),
            Self::NotSignedIn => format!(
                "{name} isn't signed in on this computer: {}",
                agent.sign_in_hint()
            ),
            Self::NotAllowed => format!("this bot's runs here don't use {name}'s sign-in"),
            Self::NoNetwork => {
                format!("this folder's commands have no network, which {name} needs")
            }
        }
    }
}

/// The state as the config says it: a loan's yes, and the network. Whether
/// the agent is installed and signed in, the config can't say (a run finds
/// the program itself when no setup recorded one), so only `cww coding
/// status`, which looks, says `not_installed` and `not_signed_in`.
pub fn agent_state(agent: Agent, root: &Root, bot: &str) -> AgentState {
    if !root.runs_commands() || !root.loan(bot).is_some_and(|loan| agent.allowed(loan)) {
        AgentState::NotAllowed
    } else if !root.network {
        AgentState::NoNetwork
    } else {
        AgentState::Ready
    }
}

/// What a bot's `roots` says of a folder lent to it: whether each agent is
/// ready, whether this computer prepares repositories by itself, and the
/// repositories' names. Never a path, an address or a token.
pub fn coding_for_bot(coding: &CodingConfig, root: &Root, bot: &str) -> Value {
    let held = || {
        coding
            .repositories
            .iter()
            .filter(|repo| repo.root == root.id)
    };
    let repositories: Vec<&str> = held()
        .map(|repo| repo.name.as_str())
        // Names are at most 100 characters; this keeps a folder's answer
        // small whatever the config holds.
        .take(MAX_REPOSITORIES_LISTED)
        .collect();
    // Only those a bot's own run named, which the server knows already.
    let named: serde_json::Map<String, Value> = held()
        .filter_map(|repo| Some((repo.github.clone()?, json!(repo.name))))
        .take(MAX_REPOSITORIES_LISTED)
        .collect();
    json!({
        // Kept for servers that know only this one.
        "claude_code": agent_state(Agent::ClaudeCode, root, bot).name(),
        "agents": Agent::ALL
            .iter()
            .map(|agent| {
                let state = match agent.unavailable() {
                    Some(_) => "not_available",
                    None => agent_state(*agent, root, bot).name(),
                };
                (agent.key().to_string(), json!(state))
            })
            .collect::<serde_json::Map<_, _>>(),
        "prepares": true,
        "owners": root.loan(bot).map(|loan| loan.owners.clone()).unwrap_or_default(),
        "repositories": repositories,
        "named": named,
    })
}

/// What a run gets beyond a person's: the folder's mirrors to read, and,
/// for a bot allowed them, the agents with their sign-ins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCoding {
    /// Mirrors of the repositories checked out in the run's folder, with
    /// their repositories: read only, fetched before the run when stale.
    pub mirrors: Vec<(PathBuf, crate::config::Repository)>,
    pub agents: Vec<AgentRun>,
    /// The folder's own place for toolchains and caches that outlast a run,
    /// once a run prepared a repository there ([`prepare::tools_env`]).
    pub tools: Option<PathBuf>,
}

/// An agent for one run: the program setup checked, where it really is,
/// what the sandbox reads for it, and the computer's sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRun {
    pub agent: Agent,
    pub program: PathBuf,
    pub real: PathBuf,
    /// The folder the program really is in, or only the program where that
    /// folder is too broad to read (see [`grant`]).
    pub read: PathBuf,
    /// Windows: the folder holding the program setup checked, as it is
    /// named there (the program may be a link to a file named otherwise),
    /// first on the run's PATH and read too.
    pub entry: Option<PathBuf>,
    pub login: Login,
}

impl AgentRun {
    /// The programs that come with it, which it starts from beside itself:
    /// Codex's `codex-code-mode-host` and `codex-exec-server`. Only those,
    /// where its folder is too broad to read whole.
    pub fn helpers(&self) -> Vec<PathBuf> {
        if self.agent != Agent::Codex {
            return Vec::new();
        }
        std::fs::read_dir(self.real_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("codex-")
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
            })
            // A program of its own beside it, never a link that leads elsewhere.
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .collect()
    }

    /// The folder holding the program itself, where any link leads.
    pub fn real_dir(&self) -> PathBuf {
        self.real
            .parent()
            .map_or_else(|| self.real.clone(), Path::to_path_buf)
    }
}

/// What a run reads for an agent at `real`: the folder it really is in,
/// where its installers put it with nothing else (`~/.local/share/claude/versions`,
/// mise's install), or, where that folder is too broad (the home folder, one
/// that holds it, a drive, a system folder, one that holds a shared folder
/// or Desktop's own), only the program itself. Windows grants a run folders
/// only, so there such a program gets nothing.
pub fn grant(real: &Path, config: &Config, paths: &Paths) -> Option<PathBuf> {
    grant_with(real, config, paths, &crate::commands::toolchain_dirs())
}

/// As [`grant`], with the toolchain folders every run reads: a program in
/// one gets its own folder even where that is under a broad one (mise's
/// installs in `%LOCALAPPDATA%` on Windows, which `roots::too_broad` counts
/// as the profile's application data).
pub fn grant_with(
    real: &Path,
    config: &Config,
    paths: &Paths,
    toolchains: &[PathBuf],
) -> Option<PathBuf> {
    let dir = real.parent()?;
    let canonical = |p: &Path| crate::roots::canonical(p).unwrap_or_else(|_| p.to_path_buf());
    if toolchains
        .iter()
        .map(|t| canonical(t))
        .any(|t| dir.starts_with(&t) && dir != t)
        && !config
            .roots
            .iter()
            .any(|r| canonical(&r.path).starts_with(dir))
    {
        return Some(dir.to_path_buf());
    }
    let holds =
        |inner: &Path| crate::roots::canonical(inner).is_ok_and(|inner| inner.starts_with(dir));
    let broad = crate::roots::too_broad(dir).map_or(true, |reason| reason.is_some())
        || config.roots.iter().any(|r| holds(&r.path))
        || [&paths.config_dir, &paths.data_dir, &paths.state_dir]
            .iter()
            .any(|own| holds(own));
    if !broad {
        Some(dir.to_path_buf())
    } else if cfg!(windows) {
        None
    } else {
        Some(real.to_path_buf())
    }
}

/// What a run of `request` in folder `root` gets. `login` looks for an
/// agent's sign-in, and is asked only for those the bot's loan allows. A run
/// gets the agent even when it isn't signed in, and hears so from the agent
/// itself.
pub fn for_run(
    config: &Config,
    paths: &Paths,
    root: &str,
    bot: Option<&str>,
    login: impl Fn(Agent) -> Login,
) -> RunCoding {
    let mirrors = config
        .coding
        .repositories
        .iter()
        .filter(|repo| repo.root == root)
        .map(|repo| (repositories::mirror_dir(paths, repo), repo.clone()))
        // Never one a command could have changed: fetching it would run what
        // its config says, outside the sandbox.
        .filter(|(dir, _)| dir.is_dir() && !repositories::writable_by_commands(dir, &config.roots))
        .collect();
    let agents = bot
        .and_then(|bot| Some((bot, config.roots.iter().find(|r| r.id == root)?)))
        .map(|(bot, root)| {
            Agent::ALL
                .into_iter()
                .filter(|agent| agent.unavailable().is_none())
                .filter(|agent| agent_state(*agent, root, bot) == AgentState::Ready)
                .filter_map(|agent| agent_for_run(agent, config, paths, &login))
                .collect()
        })
        .unwrap_or_default();
    let tools = prepare::tools_dir(config, root);
    RunCoding {
        mirrors,
        agents,
        tools,
    }
}

fn agent_for_run(
    agent: Agent,
    config: &Config,
    paths: &Paths,
    login: &impl Fn(Agent) -> Login,
) -> Option<AgentRun> {
    // The one setup recorded while it's there, else the one this computer
    // has: a bot allowed an agent needs no `cww coding setup`.
    let program = setup::program_for_run(agent, &config.coding, &config.roots)?;
    // Without Windows' `\\?\` prefix, which a run's PATH and grants can't use.
    let real = crate::roots::canonical(&program)
        .ok()
        .filter(|p| p.is_file())?;
    // A program that's a script in an npm package (Gemini CLI's
    // `bundle/gemini.js`, pi's `dist/cli.js` from `npm install -g`) reads the
    // package's own files, its dependencies too: the package's folder.
    let package = npm_package_entry(&real);
    let read = grant(package.as_deref().unwrap_or(&real), config, paths)?;
    let entry = if cfg!(windows) {
        let named = crate::roots::canonical(program.parent()?)
            .ok()?
            .join(program.file_name()?);
        Some(grant(&named, config, paths)?)
    } else {
        None
    };
    Some(AgentRun {
        agent,
        program,
        real,
        read,
        entry,
        login: login(agent).narrowed(config, paths),
    })
}

/// For a program that's a JavaScript file in an npm package, a path whose
/// folder is that package's (what [`grant`] reads): the nearest folder above
/// it, at most three up, with a `package.json`.
fn npm_package_entry(real: &Path) -> Option<PathBuf> {
    let script = real
        .extension()
        .is_some_and(|e| e == "js" || e == "mjs" || e == "cjs");
    if !script {
        return None;
    }
    real.ancestors()
        .skip(1)
        .take(3)
        .find(|dir| dir.join("package.json").is_file())
        .map(|dir| dir.join("package.json"))
}

/// Takes secrets' exact text out of a stream as it passes, across the
/// chunks it arrives in, each replaced by its label: it holds back only an
/// end that could be the start of one, and lets it go once it can't be, or
/// at the end.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Each secret's bytes and label, longest first, so a secret that
    /// starts another never leaves the longer one's rest behind.
    secrets: Vec<(Vec<u8>, String)>,
    held: Vec<u8>,
}

impl Redactor {
    pub fn new<'a>(secrets: impl IntoIterator<Item = (&'a Secret, String)>) -> Self {
        let mut secrets: Vec<(Vec<u8>, String)> = secrets
            .into_iter()
            .map(|(s, label)| (s.expose().as_bytes().to_vec(), label))
            .filter(|(s, _)| !s.is_empty())
            .collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.0.len()));
        Self {
            secrets,
            held: Vec::new(),
        }
    }

    /// The bytes that may go out now.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.secrets.is_empty() {
            return bytes.to_vec();
        }
        self.held.extend_from_slice(bytes);
        self.drain(false)
    }

    /// The end of the stream: whatever was held back, with any whole
    /// secret in it taken out.
    pub fn finish(&mut self) -> Vec<u8> {
        self.drain(true)
    }

    fn drain(&mut self, last: bool) -> Vec<u8> {
        let held = std::mem::take(&mut self.held);
        let mut out = Vec::with_capacity(held.len());
        let mut i = 0;
        while i < held.len() {
            let rest = &held[i..];
            if !last
                && self
                    .secrets
                    .iter()
                    .any(|(s, _)| s.len() > rest.len() && s.starts_with(rest))
            {
                // The end could be a secret's start: wait for more.
                self.held = rest.to_vec();
                return out;
            }
            if let Some((secret, label)) = self.secrets.iter().find(|(s, _)| rest.starts_with(s)) {
                out.extend_from_slice(label.as_bytes());
                i += secret.len();
            } else {
                out.push(held[i]);
                i += 1;
            }
        }
        out
    }
}
