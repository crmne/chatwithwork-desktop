//! Coding bots on this computer (docs/coding-computers.md): the Claude Code
//! sign-in a bot's commands get, the repositories Desktop mirrors for them,
//! and what a bot's `roots` says is ready.
//!
//! The sign-in is Claude Code's own, where Claude Code keeps it on this
//! computer ([`Login`]): a bot whose loan says `claude_code` gets those
//! paths in its run's sandbox, to read and write as Claude Code does when
//! it refreshes its tokens, and nothing else of the home folder. Any token
//! Desktop can read there is taken out of the run's output ([`Redactor`]).
//! Mirrors live in Desktop's data folder, which no command writes, so
//! fetching one with the person's credentials runs nothing a bot wrote.

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

/// Claude Code's sign-in on this computer, where Claude Code itself keeps
/// it for whoever runs `claude` here: `~/.claude` and `~/.claude.json` in
/// the home folder, and on macOS the login keychain's
/// `Claude Code-credentials`. Only those standard places: never another
/// folder `CLAUDE_CONFIG_DIR` or an account switcher points at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Login {
    /// `~/.claude`, where it really is, if it's there.
    pub dir: Option<PathBuf>,
    /// `~/.claude.json`, where it really is, if it's there.
    pub file: Option<PathBuf>,
    /// macOS: the keychain holds the tokens, so the run's sandbox reaches
    /// the keychain.
    pub keychain: bool,
    /// The tokens Desktop could read, to take out of a run's output.
    pub secrets: Vec<Secret>,
}

impl Login {
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
            .filter(|p| p.file_name().is_some_and(is_config_dir))
            .filter(|p| matches!(crate::roots::too_broad(p), Ok(None)));
        // Only the file itself, never another a link leads to.
        let file = real(CONFIG_FILE).filter(|p| {
            p.is_file()
                && crate::roots::canonical(home)
                    .is_ok_and(|home| p.as_path() == home.join(CONFIG_FILE))
        });
        let mut secrets = dir
            .as_ref()
            .and_then(|d| read_small(&d.join(CREDENTIALS_FILE)))
            .map(|text| tokens_in(&text))
            .unwrap_or_default();
        let keychain = cfg!(target_os = "macos")
            && crate::paths::home_dir()
                .ok()
                .and_then(|h| crate::roots::canonical(&h).ok())
                == crate::roots::canonical(home).ok()
            && keychain_item(false).is_some();
        if keychain && let Some(text) = keychain_item(true) {
            secrets.extend(tokens_in(&text));
        }
        Self {
            dir,
            file,
            keychain,
            secrets,
        }
    }

    /// Whether Claude Code is signed in here: tokens in its folder, or in
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
            [&paths.config_dir, &paths.data_dir, &paths.state_dir]
                .iter()
                .any(|own| inner(own))
                || config.roots.iter().any(|r| inner(&r.path))
        };
        if self.dir.as_ref().is_some_and(holds) {
            self.dir = None;
        }
        self
    }
}

/// Whether a folder is named as Claude Code's: on Windows, whose file names
/// ignore case, `.Claude` is that folder too.
fn is_config_dir(name: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(CONFIG_DIR)
    } else {
        name == CONFIG_DIR
    }
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

/// Whether Claude Code is ready for a bot's commands in a folder, as the
/// config says. A run checks again, with the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeState {
    Ready,
    /// Setup found no `claude` here.
    NotInstalled,
    /// Claude Code isn't signed in on this computer. Only `cww coding`
    /// says so: the server's `roots` comes from the config, which doesn't
    /// know, and a run then hears it from Claude Code itself.
    NotSignedIn,
    /// Not lent to this bot in this folder.
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
            Self::NotSignedIn => {
                "Claude Code isn't signed in on this computer: run `claude` once and sign in"
            }
            Self::NotAllowed => "this bot's runs here don't use Claude Code's sign-in",
            Self::NoNetwork => "this folder's commands have no network, which Claude Code needs",
        }
    }
}

/// The state as the config says it: a loan's yes, and the network. Whether
/// Claude Code is installed and signed in, the config can't say (a run finds
/// `claude` itself when no setup recorded one), so only `cww coding status`,
/// which looks, says `not_installed` and `not_signed_in`.
pub fn claude_state(_coding: &CodingConfig, root: &Root, bot: &str) -> ClaudeState {
    if !root.runs_commands() || !root.loan(bot).is_some_and(|loan| loan.claude_code) {
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
        // Names are at most 100 characters; this keeps a folder's answer
        // small whatever the config holds.
        .take(MAX_REPOSITORIES_LISTED)
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

/// Claude Code for one run: the program setup checked, where it really is,
/// what the sandbox reads for it, and the computer's sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claude {
    pub program: PathBuf,
    pub real: PathBuf,
    /// The folder the program really is in, or only the program where that
    /// folder is too broad to read (see [`grant`]).
    pub read: PathBuf,
    /// Windows: the folder holding the `claude.exe` setup checked, as it
    /// is named there (the program may be a link to a file named otherwise),
    /// first on the run's PATH and read too.
    pub entry: Option<PathBuf>,
    pub login: Login,
}

impl Claude {
    /// The folder holding the program itself, where any link leads.
    pub fn real_dir(&self) -> PathBuf {
        self.real
            .parent()
            .map_or_else(|| self.real.clone(), Path::to_path_buf)
    }
}

/// What a run reads for Claude Code at `real`: the folder it really is in,
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

/// What a run of `request` in folder `root` gets. `login` looks for Claude
/// Code's sign-in, and is asked only when the bot's loan allows Claude Code.
/// A run gets Claude Code even when it isn't signed in, and hears so from
/// Claude Code itself.
pub fn for_run(
    config: &Config,
    paths: &Paths,
    root: &str,
    bot: Option<&str>,
    login: impl FnOnce() -> Login,
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
    let claude = bot.and_then(|bot| {
        let root = config.roots.iter().find(|r| r.id == root)?;
        if claude_state(&config.coding, root, bot) != ClaudeState::Ready {
            return None;
        }
        let program = config
            .coding
            .claude
            .clone()
            .or_else(|| found_claude(&config.roots))?;
        // Without Windows' `\\?\` prefix, which a run's PATH and grants can't use.
        let real = crate::roots::canonical(&program)
            .ok()
            .filter(|p| p.is_file())?;
        let read = grant(&real, config, paths)?;
        let entry = if cfg!(windows) {
            let named = crate::roots::canonical(program.parent()?)
                .ok()?
                .join(program.file_name()?);
            Some(grant(&named, config, paths)?)
        } else {
            None
        };
        Some(Claude {
            program,
            real,
            read,
            entry,
            login: login().narrowed(config, paths),
        })
    });
    RunCoding { mirrors, claude }
}

/// The `claude` this computer has, found once by whoever starts runs when
/// no setup recorded one: a bot allowed Claude Code needs no `cww coding
/// setup`. Never one a command could have changed.
fn found_claude(roots: &[Root]) -> Option<PathBuf> {
    static FOUND: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    if found.as_ref().is_none_or(|p| !p.is_file()) {
        *found = setup::find_claude_outside(roots);
    }
    found.clone()
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
