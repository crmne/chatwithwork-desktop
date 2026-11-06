//! Bots' requests about a folder lent to them (PROTOCOL.md 7.2.1): to run
//! their commands there unattended (`ask_unattended`), or to run Claude Code
//! there with this computer's own sign-in (`ask_claude_code`,
//! docs/coding-computers.md).
//!
//! The server can only queue a request here; it never changes what the
//! computer allows. Only the person on this computer answers one: in the
//! desktop app's own dialog, or with `cww roots allow <root> <bot>` or
//! `cww roots decline <root> <bot>`, which go through the control channel
//! (CONTROL.md, `roots_unattended_answer`). A request lasts a day, and is
//! kept in the state folder so a restart doesn't lose it, but only for the
//! pairing it came through.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::config::Config;

/// How long a request waits for an answer.
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// At most this many wait at once: each needs a folder lent to a bot, so a
/// real computer has far fewer.
const MAX_PENDING: usize = 64;

/// What a bot asks for in a folder lent to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    /// Its commands there start without anyone approving each one.
    #[default]
    Unattended,
    /// Its runs there use this computer's Claude Code sign-in.
    ClaudeCode,
}

impl AskKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unattended => "unattended",
            Self::ClaudeCode => "claude_code",
        }
    }
}

/// One request: the folder's ID and the bot's, what it asks for, and when
/// it was made and ends, in Unix seconds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ask {
    pub root: String,
    pub bot: String,
    /// Before 0.14.0 every request was for unattended commands.
    #[serde(default)]
    pub kind: AskKind,
    /// The pairing it came through ([`pairing`]): another server's bots
    /// have other numbers, so a request never outlives its pairing.
    #[serde(default)]
    pub pairing: String,
    pub asked_at: i64,
    pub expires_at: i64,
}

impl Ask {
    /// The system notification's ID for it.
    pub fn notice_id(&self) -> String {
        match self.kind {
            AskKind::Unattended => format!("unattended:{}:{}", self.root, self.bot),
            AskKind::ClaudeCode => format!("claude_code:{}:{}", self.root, self.bot),
        }
    }

    fn is(&self, root: &str, bot: &str, kind: AskKind) -> bool {
        self.root == root && self.bot == bot && self.kind == kind
    }
}

/// A request as clients show it, named from this computer's own settings:
/// the folder's label and the name the loan was given here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskView {
    pub root: String,
    pub folder: String,
    pub bot: String,
    #[serde(default)]
    pub kind: AskKind,
    pub name: String,
    pub asked_at: String,
    pub expires_at: String,
}

/// What [`Asks::ask`] did with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// It's waiting for the person now.
    Queued,
    /// The same request already was.
    Already,
    /// Too many wait already; it wasn't queued.
    Full,
}

pub struct Asks {
    file: Option<PathBuf>,
    /// The pairing requests are made through now.
    pairing: Mutex<String>,
    pending: Mutex<Vec<Ask>>,
    /// Held while the list is written, so writes land in order, outside
    /// the list's own lock.
    saving: Mutex<()>,
    /// Woken when a new request comes in.
    asked: tokio::sync::Notify,
}

/// The pairing `config` names: the server and this computer's device ID
/// there, empty when unpaired.
pub fn pairing(config: &Config) -> String {
    config
        .server
        .as_ref()
        .map(|server| format!("{} {}", server.url, server.device_id))
        .unwrap_or_default()
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn rfc3339(seconds: i64) -> String {
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

impl Asks {
    /// The requests saved in `file`, if any, without the expired ones and
    /// those made through another pairing than `pairing`: the computer may
    /// have been paired again while the daemon was stopped.
    pub fn load(file: PathBuf, pairing: String) -> Self {
        let saved: Vec<Ask> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let asks = Self {
            file: Some(file),
            pairing: Mutex::new(pairing.clone()),
            pending: Mutex::new(saved),
            saving: Mutex::new(()),
            asked: tokio::sync::Notify::new(),
        };
        // Dropped from the file too.
        let _ = asks.edit(|pending| pending.retain(|ask| ask.pairing == pairing));
        // Restored requests are told of as new ones are.
        asks.asked.notify_one();
        asks
    }

    /// Requests kept only in memory, for tests.
    pub fn in_memory() -> Self {
        Self {
            file: None,
            pairing: Mutex::new(String::new()),
            pending: Mutex::new(Vec::new()),
            saving: Mutex::new(()),
            asked: tokio::sync::Notify::new(),
        }
    }

    /// Change the list (dropping expired requests first) and save it.
    fn edit<T>(&self, change: impl FnOnce(&mut Vec<Ask>) -> T) -> (T, anyhow::Result<()>) {
        let now = now();
        self.edit_all(|pending| {
            pending.retain(|ask| ask.expires_at > now);
            change(pending)
        })
    }

    /// Change the list, expired requests included, and save it. The file
    /// is written after the list's lock is let go, so a slow disk never
    /// holds up a reader; a failed save says why.
    fn edit_all<T>(&self, change: impl FnOnce(&mut Vec<Ask>) -> T) -> (T, anyhow::Result<()>) {
        let _saving = self.saving.lock().expect("unattended asks saving");
        let (result, snapshot) = {
            let mut pending = self.pending.lock().expect("unattended asks");
            let before = pending.clone();
            let result = change(&mut pending);
            let changed = *pending != before;
            (result, changed.then(|| pending.clone()))
        };
        let saved = match (snapshot, &self.file) {
            (Some(snapshot), Some(file)) => serde_json::to_vec(&snapshot)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| crate::paths::write_private_file(file, &bytes)),
            _ => Ok(()),
        };
        if let Err(e) = &saved {
            tracing::warn!("couldn't save the bots' requests: {e:#}");
        }
        (result, saved)
    }

    /// Queue a request of `kind` from `bot` for folder `root`, unless the
    /// same one waits or too many do. A request that can't be saved isn't
    /// queued.
    pub fn ask(&self, root: &str, bot: &str, kind: AskKind) -> anyhow::Result<Asked> {
        let (asked, saved) = self.edit(|pending| {
            if pending.iter().any(|ask| ask.is(root, bot, kind)) {
                return Asked::Already;
            }
            if pending.len() >= MAX_PENDING {
                return Asked::Full;
            }
            let asked_at = now();
            pending.push(Ask {
                root: root.into(),
                bot: bot.into(),
                kind,
                pairing: self
                    .pairing
                    .lock()
                    .expect("unattended asks pairing")
                    .clone(),
                asked_at,
                expires_at: asked_at + TTL.as_secs() as i64,
            });
            Asked::Queued
        });
        if asked == Asked::Queued {
            if let Err(e) = saved {
                // Not kept in memory either: the bot is told it failed.
                let _ = self.edit(|pending| pending.retain(|a| !a.is(root, bot, kind)));
                return Err(e);
            }
            self.asked.notify_one();
        }
        Ok(asked)
    }

    /// Take the request of `kind` from `bot` for `root` off the list, if
    /// one waits and `config` still leaves it something to answer for: the
    /// folder still lent to that bot, with commands, and what it asks for
    /// not given yet. With `asked_at`, only the request shown as made then.
    pub fn take(
        &self,
        root: &str,
        bot: &str,
        kind: AskKind,
        asked_at: Option<&str>,
        config: &Config,
    ) -> Option<Ask> {
        self.edit(|pending| {
            let index = pending.iter().position(|ask| {
                ask.is(root, bot, kind)
                    && open(config, ask)
                    && asked_at.is_none_or(|shown| shown == rfc3339(ask.asked_at))
            })?;
            Some(pending.remove(index))
        })
        .0
    }

    /// Put back a request taken for an answer that didn't go through.
    pub fn restore(&self, ask: Ask) {
        let _ = self.edit_all(|pending| {
            if !pending.iter().any(|a| a.is(&ask.root, &ask.bot, ask.kind)) {
                pending.push(ask);
            }
        });
    }

    /// Forget every request: the computer was paired again, as `pairing`,
    /// and another server's bots' numbers mean other bots. Returns them.
    pub fn clear(&self, pairing: String) -> Vec<Ask> {
        *self.pairing.lock().expect("unattended asks pairing") = pairing;
        self.edit_all(std::mem::take).0
    }

    /// Wake whoever tells of requests, to look again.
    pub fn wake(&self) {
        self.asked.notify_one();
    }

    /// Drop the requests `config` no longer leaves anything to answer for:
    /// the folder isn't lent to that bot any more, its commands stopped, or
    /// the loan is already unattended. Returns them, with the expired ones.
    pub fn prune(&self, config: &Config) -> Vec<Ask> {
        let now = now();
        self.edit_all(|pending| {
            let (stale, kept): (Vec<Ask>, Vec<Ask>) = std::mem::take(pending)
                .into_iter()
                .partition(|ask| ask.expires_at <= now || !open(config, ask));
            *pending = kept;
            stale
        })
        .0
    }

    /// The requests waiting, oldest first, named from `config`.
    pub fn list(&self, config: &Config) -> Vec<AskView> {
        let now = now();
        self.pending
            .lock()
            .expect("unattended asks")
            .iter()
            .filter(|ask| ask.expires_at > now && open(config, ask))
            .filter_map(|ask| {
                let root = config.roots.iter().find(|r| r.id == ask.root)?;
                let loan = root.loan(&ask.bot)?;
                Some(AskView {
                    root: ask.root.clone(),
                    folder: root.label.clone(),
                    bot: ask.bot.clone(),
                    kind: ask.kind,
                    name: loan.name.clone(),
                    asked_at: rfc3339(ask.asked_at),
                    expires_at: rfc3339(ask.expires_at),
                })
            })
            .collect()
    }

    /// The requests waiting, as stored.
    pub fn pending(&self) -> Vec<Ask> {
        self.edit(|pending| pending.clone()).0
    }

    /// Wait for a new request.
    pub async fn asked(&self) {
        self.asked.notified().await;
    }
}

/// Whether a request still has something to answer: it came through the
/// pairing `config` names (another server's bot can have the same number),
/// the folder is lent to the bot, allows commands, and its loan doesn't
/// give what it asks for yet.
pub fn open(config: &Config, ask: &Ask) -> bool {
    ask.pairing == pairing(config)
        && config
            .roots
            .iter()
            .find(|root| root.id == ask.root)
            .is_some_and(|root| {
                root.runs_commands()
                    && root.loan(&ask.bot).is_some_and(|loan| match ask.kind {
                        AskKind::Unattended => !loan.unattended,
                        AskKind::ClaudeCode => !loan.claude_code,
                    })
            })
}

/// What the person is asked, from this computer's own names for the bot
/// and the folder, never the server's words.
pub fn question(ask: &AskView) -> String {
    match ask.kind {
        AskKind::Unattended => format!(
            "{} asks to run commands in {} without asking each time. They still run in the \
             folder's sandbox and are recorded. Allow?",
            bot_name(ask),
            ask.folder
        ),
        AskKind::ClaudeCode => format!(
            "Let {} run Claude Code in {}? Its runs there use this computer's Claude Code \
             sign-in: they can read and change ~/.claude and ~/.claude.json{}, where a setting \
             they change runs the next time you start `claude` yourself.",
            bot_name(ask),
            ask.folder,
            if cfg!(target_os = "macos") {
                " and reach your login keychain"
            } else {
                ""
            }
        ),
    }
}

/// The dialog's title for a request.
pub fn title(kind: AskKind) -> &'static str {
    match kind {
        AskKind::Unattended => "Allow unattended commands",
        AskKind::ClaudeCode => "Allow Claude Code",
    }
}

/// The loan's name for the bot, or its number when it has none.
pub fn bot_name(ask: &AskView) -> String {
    if ask.name.trim().is_empty() {
        format!("Bot {}", ask.bot)
    } else {
        ask.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BotLoan, Root};

    fn config(commands: bool, unattended: bool) -> Config {
        Config {
            roots: vec![Root {
                id: "app".into(),
                label: "App".into(),
                path: "/nowhere".into(),
                follow_symlinks: false,
                writable: true,
                commands,
                network: false,
                read_budget: None,
                bots: vec![BotLoan {
                    id: "42".into(),
                    name: "Issue fixer".into(),
                    unattended,
                    claude_code: false,
                }],
            }],
            ..Config::default()
        }
    }

    /// A bot's request for Claude Code is its own: it waits beside one for
    /// unattended commands, says what it gives, and is open until the loan
    /// gets Claude Code.
    #[test]
    fn a_request_for_claude_code_is_its_own() {
        let asks = Asks::in_memory();
        assert_eq!(
            asks.ask("app", "42", AskKind::Unattended).unwrap(),
            Asked::Queued
        );
        assert_eq!(
            asks.ask("app", "42", AskKind::ClaudeCode).unwrap(),
            Asked::Queued
        );
        assert_eq!(
            asks.ask("app", "42", AskKind::ClaudeCode).unwrap(),
            Asked::Already
        );
        let list = asks.list(&config(true, false));
        assert_eq!(list.len(), 2);
        let claude = list.iter().find(|a| a.kind == AskKind::ClaudeCode).unwrap();
        assert!(
            question(claude).starts_with("Let Issue fixer run Claude Code in App?"),
            "{}",
            question(claude)
        );
        let mut given = config(true, true);
        given.roots[0].bots[0].claude_code = true;
        assert!(
            asks.take("app", "42", AskKind::ClaudeCode, None, &given)
                .is_none()
        );
        assert!(
            asks.take("app", "42", AskKind::ClaudeCode, None, &config(true, true))
                .is_some(),
            "unattended already, but not Claude Code"
        );
        // An answer reaches only the request it was shown.
        asks.ask("app", "42", AskKind::ClaudeCode).unwrap();
        assert!(
            asks.take(
                "app",
                "42",
                AskKind::ClaudeCode,
                Some("then"),
                &config(true, true)
            )
            .is_none()
        );
        let shown = asks
            .list(&config(true, true))
            .into_iter()
            .find(|a| a.kind == AskKind::ClaudeCode)
            .unwrap()
            .asked_at;
        assert!(
            asks.take(
                "app",
                "42",
                AskKind::ClaudeCode,
                Some(&shown),
                &config(true, true)
            )
            .is_some()
        );
        // A request saved before kinds existed is for unattended commands.
        let old: Ask = serde_json::from_value(serde_json::json!({
            "root": "app", "bot": "42", "asked_at": 1, "expires_at": 2
        }))
        .unwrap();
        assert_eq!(old.kind, AskKind::Unattended);
    }

    #[test]
    fn asks_once_until_answered() {
        let asks = Asks::in_memory();
        assert_eq!(
            asks.ask("app", "42", AskKind::Unattended).unwrap(),
            Asked::Queued
        );
        assert_eq!(
            asks.ask("app", "42", AskKind::Unattended).unwrap(),
            Asked::Already,
            "a repeat raises no second question"
        );
        let list = asks.list(&config(true, false));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].folder, "App");
        assert_eq!(list[0].name, "Issue fixer");
        assert_eq!(
            question(&list[0]),
            "Issue fixer asks to run commands in App without asking each time. They still \
             run in the folder's sandbox and are recorded. Allow?"
        );
        assert!(
            asks.take("app", "42", AskKind::Unattended, None, &config(true, false))
                .is_some()
        );
        assert!(
            asks.take("app", "42", AskKind::Unattended, None, &config(true, false))
                .is_none()
        );
        assert_eq!(
            asks.ask("app", "42", AskKind::Unattended).unwrap(),
            Asked::Queued,
            "asks again after an answer"
        );
    }

    #[test]
    fn a_request_whose_loan_went_away_cant_be_answered() {
        let asks = Asks::in_memory();
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        let mut withdrawn = config(true, false);
        withdrawn.roots[0].bots.clear();
        assert!(
            asks.take("app", "42", AskKind::Unattended, None, &withdrawn)
                .is_none()
        );
        assert!(
            asks.take(
                "app",
                "42",
                AskKind::Unattended,
                None,
                &config(false, false)
            )
            .is_none(),
            "commands stopped"
        );
        let ask = asks
            .take("app", "42", AskKind::Unattended, None, &config(true, false))
            .unwrap();
        asks.restore(ask);
        assert_eq!(asks.pending().len(), 1, "put back when the answer failed");
        assert_eq!(asks.clear(String::new()).len(), 1);
        assert!(asks.pending().is_empty());
    }

    #[test]
    fn a_full_queue_says_so() {
        let asks = Asks::in_memory();
        for bot in 0..MAX_PENDING {
            assert_eq!(
                asks.ask("app", &bot.to_string(), AskKind::Unattended)
                    .unwrap(),
                Asked::Queued
            );
        }
        assert_eq!(
            asks.ask("app", "999", AskKind::Unattended).unwrap(),
            Asked::Full
        );
    }

    #[test]
    fn a_request_that_cant_be_saved_isnt_queued() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file should be: every save fails.
        let file = dir.path().join("asks.json");
        std::fs::create_dir(&file).unwrap();
        let asks = Asks::load(file, String::new());
        assert!(asks.ask("app", "42", AskKind::Unattended).is_err());
        assert!(asks.pending().is_empty());
    }

    #[test]
    fn drops_what_nothing_is_left_to_answer_for() {
        let asks = Asks::in_memory();
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        assert!(asks.prune(&config(true, false)).is_empty());
        assert_eq!(
            asks.prune(&config(true, true)).len(),
            1,
            "already unattended"
        );
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        assert!(asks.list(&config(false, false)).is_empty());
        assert_eq!(
            asks.prune(&config(false, false)).len(),
            1,
            "commands stopped"
        );
        assert!(asks.pending().is_empty());
    }

    #[test]
    fn requests_expire_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("asks.json");
        let asks = Asks::load(file.clone(), String::new());
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        let again = Asks::load(file.clone(), String::new());
        assert_eq!(again.pending().len(), 1);
        let mut old = again.pending();
        old[0].expires_at = now() - 1;
        std::fs::write(&file, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(Asks::load(file, String::new()).pending().is_empty());
    }

    #[test]
    fn a_request_is_never_answered_for_another_pairing() {
        let asks = Asks::in_memory();
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        // Paired meanwhile, before the reload dropped the request: the
        // new server's bot 42 is another bot.
        let mut paired = config(true, false);
        paired.server = Some(crate::config::ServerConfig {
            url: "https://two.example".into(),
            device_id: "9".into(),
        });
        assert!(
            asks.take("app", "42", AskKind::Unattended, None, &paired)
                .is_none()
        );
        assert!(asks.list(&paired).is_empty());
        assert!(
            asks.take("app", "42", AskKind::Unattended, None, &config(true, false))
                .is_some()
        );
    }

    #[test]
    fn a_new_pairing_drops_the_old_ones_requests() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("asks.json");
        let asks = Asks::load(file.clone(), "https://one.example 7".into());
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        // Paired again while the daemon was stopped.
        let again = Asks::load(file.clone(), "https://two.example 9".into());
        assert!(again.pending().is_empty());
        assert!(
            Asks::load(file, "https://one.example 7".into())
                .pending()
                .is_empty()
        );
        // Paired again while it ran.
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        assert_eq!(asks.clear("https://two.example 9".into()).len(), 1);
        asks.ask("app", "42", AskKind::Unattended).unwrap();
        assert_eq!(asks.pending()[0].pairing, "https://two.example 9");
    }
}
