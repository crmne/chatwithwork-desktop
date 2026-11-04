//! Bots' requests to run their commands unattended in a folder lent to them
//! (PROTOCOL.md 7.2.1, `ask_unattended`).
//!
//! The server can only queue a request here; it never changes what the
//! computer allows. Only the person on this computer answers one: in the
//! desktop app's own dialog, or with `cww roots share <root> <bot>
//! --unattended` or `cww roots decline <root> <bot>`, which go through the
//! control channel (CONTROL.md, `roots_unattended_answer`). A request lasts
//! a day, and is kept in the state folder so a restart doesn't lose it.

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

/// One request: the folder's ID and the bot's, and when it was made and
/// ends, in Unix seconds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ask {
    pub root: String,
    pub bot: String,
    pub asked_at: i64,
    pub expires_at: i64,
}

impl Ask {
    /// The system notification's ID for it.
    pub fn notice_id(&self) -> String {
        format!("unattended:{}:{}", self.root, self.bot)
    }
}

/// A request as clients show it, named from this computer's own settings:
/// the folder's label and the name the loan was given here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskView {
    pub root: String,
    pub folder: String,
    pub bot: String,
    pub name: String,
    pub asked_at: String,
    pub expires_at: String,
}

pub struct Asks {
    file: Option<PathBuf>,
    pending: Mutex<Vec<Ask>>,
    /// Woken when a new request comes in.
    asked: tokio::sync::Notify,
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
    /// The requests saved in `file`, if any, without the expired ones.
    pub fn load(file: PathBuf) -> Self {
        let pending: Vec<Ask> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let asks = Self {
            file: Some(file),
            pending: Mutex::new(pending),
            asked: tokio::sync::Notify::new(),
        };
        asks.edit(|_| ());
        asks
    }

    /// Requests kept only in memory, for tests.
    pub fn in_memory() -> Self {
        Self {
            file: None,
            pending: Mutex::new(Vec::new()),
            asked: tokio::sync::Notify::new(),
        }
    }

    /// Change the list (dropping expired requests first) and save it.
    fn edit<T>(&self, change: impl FnOnce(&mut Vec<Ask>) -> T) -> T {
        let now = now();
        self.edit_all(|pending| {
            pending.retain(|ask| ask.expires_at > now);
            change(pending)
        })
    }

    /// Change the list, expired requests included, and save it.
    fn edit_all<T>(&self, change: impl FnOnce(&mut Vec<Ask>) -> T) -> T {
        let mut pending = self.pending.lock().expect("unattended asks");
        let before = pending.clone();
        let result = change(&mut pending);
        if *pending != before
            && let Some(file) = &self.file
        {
            let saved = serde_json::to_vec(&*pending)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| crate::paths::write_private_file(file, &bytes));
            if let Err(e) = saved {
                tracing::warn!("couldn't save the bots' requests: {e:#}");
            }
        }
        result
    }

    /// Queue a request from `bot` for folder `root`. False when one is
    /// already waiting (or too many are), so nobody is asked twice.
    pub fn ask(&self, root: &str, bot: &str) -> bool {
        let added = self.edit(|pending| {
            if pending.len() >= MAX_PENDING
                || pending.iter().any(|ask| ask.root == root && ask.bot == bot)
            {
                return false;
            }
            let asked_at = now();
            pending.push(Ask {
                root: root.into(),
                bot: bot.into(),
                asked_at,
                expires_at: asked_at + TTL.as_secs() as i64,
            });
            true
        });
        if added {
            self.asked.notify_one();
        }
        added
    }

    /// Take the request from `bot` for `root` off the list, if one waits.
    pub fn take(&self, root: &str, bot: &str) -> Option<Ask> {
        self.edit(|pending| {
            let index = pending
                .iter()
                .position(|ask| ask.root == root && ask.bot == bot)?;
            Some(pending.remove(index))
        })
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
                    name: loan.name.clone(),
                    asked_at: rfc3339(ask.asked_at),
                    expires_at: rfc3339(ask.expires_at),
                })
            })
            .collect()
    }

    /// The requests waiting, as stored.
    pub fn pending(&self) -> Vec<Ask> {
        self.edit(|pending| pending.clone())
    }

    /// Wait for a new request.
    pub async fn asked(&self) {
        self.asked.notified().await;
    }
}

/// Whether a request still has something to answer: the folder is lent to
/// the bot, allows commands, and its loan isn't unattended yet.
fn open(config: &Config, ask: &Ask) -> bool {
    config
        .roots
        .iter()
        .find(|root| root.id == ask.root)
        .is_some_and(|root| {
            root.runs_commands() && root.loan(&ask.bot).is_some_and(|loan| !loan.unattended)
        })
}

/// What the person is asked, from this computer's own names for the bot
/// and the folder, never the server's words.
pub fn question(ask: &AskView) -> String {
    format!(
        "{} asks to run commands in {} without asking each time. They still run in the \
         folder's sandbox and are recorded. Allow?",
        bot_name(ask),
        ask.folder
    )
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

    #[test]
    fn asks_once_until_answered() {
        let asks = Asks::in_memory();
        assert!(asks.ask("app", "42"));
        assert!(!asks.ask("app", "42"), "a repeat raises no second question");
        let list = asks.list(&config(true, false));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].folder, "App");
        assert_eq!(list[0].name, "Issue fixer");
        assert_eq!(
            question(&list[0]),
            "Issue fixer asks to run commands in App without asking each time. They still \
             run in the folder's sandbox and are recorded. Allow?"
        );
        assert!(asks.take("app", "42").is_some());
        assert!(asks.take("app", "42").is_none());
        assert!(asks.ask("app", "42"), "asks again after an answer");
    }

    #[test]
    fn drops_what_nothing_is_left_to_answer_for() {
        let asks = Asks::in_memory();
        asks.ask("app", "42");
        assert!(asks.prune(&config(true, false)).is_empty());
        assert_eq!(
            asks.prune(&config(true, true)).len(),
            1,
            "already unattended"
        );
        asks.ask("app", "42");
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
        let asks = Asks::load(file.clone());
        asks.ask("app", "42");
        let again = Asks::load(file.clone());
        assert_eq!(again.pending().len(), 1);
        let mut old = again.pending();
        old[0].expires_at = now() - 1;
        std::fs::write(&file, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(Asks::load(file).pending().is_empty());
    }
}
