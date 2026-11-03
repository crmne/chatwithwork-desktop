//! Coding bots on this computer (docs/coding-computers.md): the Claude Code
//! sign-in a bot's commands get, the repositories Desktop mirrors for them,
//! and what a bot's `roots` says is ready.
//!
//! The token lives in the secret store (`auth::secrets`), never in a file a
//! command reads. Only whoever starts a command reads it, for one run of a
//! bot whose loan says `claude_code`, and the run's output has its text
//! taken out ([`Redactor`]). Mirrors live in Desktop's data folder, which no
//! command writes, so fetching one with the person's credentials runs
//! nothing a bot wrote.

pub mod detect;
pub mod repositories;
pub mod setup;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::auth::secrets::SecretStore;
use crate::config::{CodingConfig, Config, Root};
use crate::paths::Paths;

/// The secret store's name for the token `claude setup-token` printed.
pub const TOKEN_NAME: &str = "claude-code-token";
/// Claude Code's own variable for such a token.
pub const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// What a run's output says where the token was.
pub const REDACTED: &str = "[claude-code-token]";

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

/// Whether `token` looks like what `claude setup-token` prints: an OAuth
/// token of Anthropic's, one line of URL-safe characters.
pub fn valid_token(token: &str) -> bool {
    token.starts_with("sk-ant-oat")
        && (40..=512).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Where the token is kept: the store this computer's pairing uses.
pub fn store(config: &Config, paths: &Paths) -> SecretStore {
    SecretStore::for_config(config, paths)
}

/// The stored token, if there is one and it can be read.
pub fn token(config: &Config, paths: &Paths) -> Option<Secret> {
    store(config, paths)
        .get(TOKEN_NAME)
        .ok()
        .flatten()
        .filter(|t| valid_token(t))
        .map(Secret::new)
}

/// Whether Claude Code is ready for a bot's commands in a folder, as the
/// config says. A run checks again, with the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeState {
    Ready,
    /// Setup found no `claude` here.
    NotInstalled,
    /// No token is stored.
    NotSignedIn,
    /// Stored, but not for this bot in this folder.
    NotAllowed,
    /// The folder's commands have no network, which Claude Code needs.
    NoNetwork,
}

impl ClaudeState {
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
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Ready => "Claude Code is ready",
            Self::NotInstalled => "Claude Code isn't installed",
            Self::NotSignedIn => "Claude Code isn't signed in",
            Self::NotAllowed => "Claude Code is signed in, but not for this bot here",
            Self::NoNetwork => "this folder's commands have no network, which Claude Code needs",
        }
    }
}

pub fn claude_state(coding: &CodingConfig, root: &Root, bot: &str) -> ClaudeState {
    if coding.claude.is_none() {
        ClaudeState::NotInstalled
    } else if !coding.claude_signed_in {
        ClaudeState::NotSignedIn
    } else if !root.runs_commands() || !root.loan(bot).is_some_and(|loan| loan.claude_code) {
        ClaudeState::NotAllowed
    } else if !root.network {
        ClaudeState::NoNetwork
    } else {
        ClaudeState::Ready
    }
}

/// What a bot's `roots` says of a folder lent to it: whether Claude Code is
/// ready, and the repositories' names. Never a path, an address or a token.
pub fn coding_for_bot(coding: &CodingConfig, root: &Root, bot: &str) -> Value {
    let repositories: Vec<&str> = coding
        .repositories
        .iter()
        .filter(|repo| repo.root == root.id)
        .map(|repo| repo.name.as_str())
        .collect();
    json!({
        "claude_code": claude_state(coding, root, bot).name(),
        "repositories": repositories,
    })
}

/// What a run gets beyond a person's: the folder's mirrors to read, and,
/// for a bot allowed it, Claude Code with its sign-in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCoding {
    /// Mirrors of the repositories checked out in the run's folder, with
    /// their repositories: read only, fetched before the run when stale.
    pub mirrors: Vec<(PathBuf, crate::config::Repository)>,
    pub claude: Option<Claude>,
}

/// Claude Code for one run: the program setup checked, the folder it really
/// is in (the sandbox reads it), and the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claude {
    pub program: PathBuf,
    pub real: PathBuf,
    pub token: Secret,
}

impl Claude {
    /// The folder holding the program itself, where any link leads: the
    /// sandbox reads only it.
    pub fn real_dir(&self) -> PathBuf {
        self.real
            .parent()
            .map_or_else(|| self.real.clone(), Path::to_path_buf)
    }
}

/// What a run of `request` in folder `root` gets. `token` reads the secret
/// store, and is asked only when the bot's loan allows Claude Code.
pub fn for_run(
    config: &Config,
    paths: &Paths,
    root: &str,
    bot: Option<&str>,
    token: impl FnOnce() -> Option<Secret>,
) -> RunCoding {
    let mirrors = config
        .coding
        .repositories
        .iter()
        .filter(|repo| repo.root == root)
        .map(|repo| (repositories::mirror_dir(paths, repo), repo.clone()))
        .filter(|(dir, _)| dir.is_dir())
        .collect();
    let claude = bot.and_then(|bot| {
        let root = config.roots.iter().find(|r| r.id == root)?;
        if claude_state(&config.coding, root, bot) != ClaudeState::Ready {
            return None;
        }
        let program = config.coding.claude.clone()?;
        let real = program.canonicalize().ok().filter(|p| p.is_file())?;
        let token = token()?;
        Some(Claude {
            program,
            real,
            token,
        })
    });
    RunCoding { mirrors, claude }
}

/// Takes a secret's exact text out of a stream as it passes, across the
/// chunks it arrives in: it holds back only an end that could be the start
/// of the secret, and lets it go once it can't be, or at the end.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secret: Vec<u8>,
    held: Vec<u8>,
}

impl Redactor {
    pub fn new(secret: Option<&Secret>) -> Self {
        Self {
            secret: secret
                .map(|s| s.expose().as_bytes().to_vec())
                .unwrap_or_default(),
            held: Vec::new(),
        }
    }

    /// The bytes that may go out now.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.secret.is_empty() {
            return bytes.to_vec();
        }
        self.held.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(self.held.len());
        let mut i = 0;
        let held = std::mem::take(&mut self.held);
        while i < held.len() {
            let rest = &held[i..];
            if rest.starts_with(&self.secret) {
                out.extend_from_slice(REDACTED.as_bytes());
                i += self.secret.len();
            } else if self.secret.starts_with(rest) {
                // The end could be the secret's start: wait for more.
                self.held = rest.to_vec();
                return out;
            } else {
                out.push(held[i]);
                i += 1;
            }
        }
        out
    }

    /// The end of the stream: whatever was held back.
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }
}
