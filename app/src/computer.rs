//! "This computer": what the web app's pages may see and change of the
//! background agent while they run in Chat with Work Desktop, so the
//! website's Settings and its new chat page can show this computer's shared
//! folders and switches (docs/desktop-app.md, "This computer").
//!
//! What a computer shares and allows is decided on the computer, never by
//! the server (the server's docs/security/desktop.md). A page of the server
//! is the server's code, so the app treats what it asks as a suggestion:
//!
//! - Narrowing happens at once: pausing, stopping sharing a folder, turning
//!   changes, commands or their network off. A compromised server could make
//!   the computer share less, never more.
//! - Widening happens only after the person says yes in a dialog the app
//!   draws, which no page can see or answer: resuming, allowing changes,
//!   commands, or the network for commands. A folder is added only through
//!   the system's folder picker, where the person picks it.
//! - One question at a time, so a page can't stack dialogs.
//! - Pages never see where a folder is on this computer (`local_path`), the
//!   activity log, the deny list, the config file or the proxy. Those, and
//!   lending folders to bots, stay in the app's own settings window.

use std::sync::atomic::{AtomicBool, Ordering};

use cww::control::ControlRequest;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{Platform, RootStatus, Status};

/// What a page sees of the computer. `None` while the background agent isn't
/// running.
pub fn view(status: Option<&Status>) -> Value {
    let health = crate::model::Health::of(status);
    let Some(status) = status else {
        return json!({
            "running": false,
            "health": { "label": health.label(), "tone": health.tone() },
            "folders": [],
            "commands": commands_supported(),
        });
    };
    json!({
        "running": true,
        "paired": status.is_paired(),
        "server": status.server,
        "device_id": status.device_id,
        "paused": status.paused,
        "connection": status.connection.connection,
        "health": { "label": health.label(), "tone": health.tone() },
        "folders": status.roots.iter().map(folder).collect::<Vec<_>>(),
        "commands": commands_supported(),
    })
}

fn folder(root: &RootStatus) -> Value {
    json!({
        "id": root.id,
        "label": root.label,
        "available": root.available,
        "index": root.index,
        "indexed_files": root.indexed_files,
        "changes": root.writable,
        "commands": root.allows_commands,
        "network": root.network,
        "bots": root.bots.len(),
    })
}

/// Commands run in folders on macOS and Linux only.
fn commands_supported() -> bool {
    Platform::current() != Platform::Windows
}

/// What a page may ask. Anything else is refused before it reaches the
/// daemon.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Change {
    Pause,
    Resume,
    Remove { folder: String },
    Rename { folder: String, label: String },
    Changes { folder: String, on: bool },
    Commands { folder: String, on: bool },
    Network { folder: String, on: bool },
}

/// A question the person answers in the app's own dialog before a change
/// that widens what the computer allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub title: String,
    pub message: String,
    pub yes: String,
}

impl Change {
    /// The daemon's request for it, and the question to ask first if it
    /// widens anything. Folders are named by their ID and must be shared;
    /// what the question names comes from the daemon, not from the page.
    pub fn plan(&self, status: &Status) -> Result<(ControlRequest, Option<Question>), String> {
        let find = |id: &str| {
            status
                .roots
                .iter()
                .find(|root| root.id == id)
                .ok_or_else(|| "That folder isn't shared on this computer.".to_string())
        };
        let ask = |title: &str, message: String, yes: &str| {
            Some(Question {
                title: title.into(),
                message,
                yes: yes.into(),
            })
        };
        Ok(match self {
            Change::Pause => (ControlRequest::Pause, None),
            Change::Resume => (
                ControlRequest::Resume,
                if status.paused {
                    ask(
                        "Resume Sharing",
                        "Let Chat with Work use this computer again? It can search and read your shared folders, and changes and commands are allowed where they were before.".into(),
                        "Resume Sharing",
                    )
                } else {
                    None
                },
            ),
            Change::Remove { folder } => (
                ControlRequest::RootsRemove {
                    root: find(folder)?.id.clone(),
                },
                None,
            ),
            Change::Rename { folder, label } => {
                let label = label.trim();
                if label.is_empty() || label.chars().count() > 80 {
                    return Err("A folder's name is 1 to 80 characters.".into());
                }
                (
                    ControlRequest::RootsLabel {
                        root: find(folder)?.id.clone(),
                        label: label.to_string(),
                    },
                    None,
                )
            }
            Change::Changes { folder, on } => {
                let root = find(folder)?;
                let question = if *on && !root.writable {
                    ask(
                        "Allow Changes",
                        format!(
                            "Let Chat with Work change files in “{}”? It asks you in Chat with Work before each change, and old versions and deleted files go to the trash on this computer.",
                            root.label
                        ),
                        "Allow Changes",
                    )
                } else {
                    None
                };
                (
                    ControlRequest::RootsWritable {
                        root: root.id.clone(),
                        writable: *on,
                        i_know: false,
                    },
                    question,
                )
            }
            Change::Commands { folder, on } => {
                let root = find(folder)?;
                if *on && !commands_supported() {
                    return Err("Commands can't run on Windows yet.".into());
                }
                if *on && !root.writable {
                    return Err("Allow changes in this folder first.".into());
                }
                let question = if *on && !root.allows_commands {
                    ask(
                        "Allow Commands",
                        format!(
                            "Let Chat with Work run commands in “{}”? It asks you in Chat with Work before each one. A command can change or delete anything in this folder, and nothing outside it.",
                            root.label
                        ),
                        "Allow Commands",
                    )
                } else {
                    None
                };
                (
                    ControlRequest::RootsCommands {
                        root: root.id.clone(),
                        commands: *on,
                        network: false,
                    },
                    question,
                )
            }
            Change::Network { folder, on } => {
                let root = find(folder)?;
                if !root.allows_commands {
                    return Err("Allow commands in this folder first.".into());
                }
                let question = if *on && !root.network {
                    ask(
                        "Allow the Network",
                        format!(
                            "Let commands in “{}” use the network? They can then reach the internet, to install packages or fetch dependencies.",
                            root.label
                        ),
                        "Allow the Network",
                    )
                } else {
                    None
                };
                (
                    ControlRequest::RootsCommands {
                        root: root.id.clone(),
                        commands: true,
                        network: *on,
                    },
                    question,
                )
            }
        })
    }
}

/// Only one of the app's questions shows at a time.
#[derive(Default)]
pub struct Asking(AtomicBool);

impl Asking {
    /// Holds the right to ask until the guard drops, or says a question is
    /// already showing.
    pub fn start(&self) -> Option<AskingGuard<'_>> {
        (!self.0.swap(true, Ordering::SeqCst)).then_some(AskingGuard(&self.0))
    }
}

pub struct AskingGuard<'a>(&'a AtomicBool);

impl Drop for AskingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The JavaScript that hands the computer's state to a page.
pub fn event(view: &Value) -> String {
    format!(
        "window.dispatchEvent(new CustomEvent(\"chatwithwork-desktop:computer\", {{ detail: {view} }}))"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BotLoan;

    fn status() -> Status {
        Status {
            server: Some("https://work.example".into()),
            device_id: Some("7".into()),
            roots: vec![
                RootStatus {
                    id: "docs".into(),
                    label: "Docs".into(),
                    available: true,
                    index: "ready".into(),
                    indexed_files: Some(12),
                    local_path: Some("/home/ana/Secret Plans".into()),
                    writable: false,
                    ..Default::default()
                },
                RootStatus {
                    id: "app".into(),
                    label: "App".into(),
                    available: true,
                    index: "ready".into(),
                    local_path: Some("/home/ana/app".into()),
                    writable: true,
                    allows_commands: true,
                    bots: vec![BotLoan {
                        id: "42".into(),
                        name: "Fixer".into(),
                        unattended: true,
                    }],
                    ..Default::default()
                },
            ],
            config_file: Some("/home/ana/.config/cww/config.toml".into()),
            audit_file: Some("/home/ana/.local/state/cww/audit.jsonl".into()),
            ..Default::default()
        }
    }

    fn change(json: Value) -> Change {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn pages_never_see_where_folders_are_or_the_local_files() {
        let view = view(Some(&status())).to_string();
        assert!(!view.contains("/home/ana"), "{view}");
        assert!(!view.contains("Secret Plans"));
        assert!(!view.contains("config.toml") && !view.contains("audit.jsonl"));
        assert!(view.contains(r#""label":"Docs""#));
        assert!(view.contains(r#""device_id":"7""#));
        assert!(view.contains(r#""bots":1"#) && !view.contains("Fixer"));
    }

    #[test]
    fn without_the_agent_pages_see_that_it_isnt_running() {
        let view = view(None);
        assert_eq!(view["running"], json!(false));
        assert_eq!(view["folders"], json!([]));
    }

    #[test]
    fn narrowing_happens_without_a_question() {
        let status = status();
        for asked in [
            json!({ "action": "pause" }),
            json!({ "action": "remove", "folder": "docs" }),
            json!({ "action": "rename", "folder": "docs", "label": "Team docs" }),
            json!({ "action": "changes", "folder": "app", "on": false }),
            json!({ "action": "commands", "folder": "app", "on": false }),
            json!({ "action": "network", "folder": "app", "on": false }),
        ] {
            let (_, question) = change(asked.clone()).plan(&status).unwrap();
            assert_eq!(question, None, "{asked}");
        }
    }

    #[test]
    fn widening_asks_first_and_names_the_folder_as_the_daemon_has_it() {
        let status = status();
        let (request, question) =
            change(json!({ "action": "changes", "folder": "docs", "on": true }))
                .plan(&status)
                .unwrap();
        assert_eq!(
            request,
            ControlRequest::RootsWritable {
                root: "docs".into(),
                writable: true,
                i_know: false
            }
        );
        let question = question.expect("a question");
        assert!(question.message.contains("“Docs”"));
        assert_eq!(question.yes, "Allow Changes");

        let (_, question) = change(json!({ "action": "network", "folder": "app", "on": true }))
            .plan(&status)
            .unwrap();
        assert!(question.is_some());

        let mut paused = status.clone();
        paused.paused = true;
        let (_, question) = change(json!({ "action": "resume" })).plan(&paused).unwrap();
        assert!(question.is_some());
        // Resuming what isn't paused changes nothing, so asks nothing.
        let (_, question) = change(json!({ "action": "resume" })).plan(&status).unwrap();
        assert!(question.is_none());
    }

    #[test]
    fn already_allowed_asks_nothing() {
        let (_, question) = change(json!({ "action": "commands", "folder": "app", "on": true }))
            .plan(&status())
            .unwrap();
        assert!(question.is_none());
    }

    #[test]
    fn folders_must_be_shared_and_steps_taken_in_order() {
        let status = status();
        assert!(
            change(json!({ "action": "remove", "folder": "/etc" }))
                .plan(&status)
                .is_err()
        );
        assert!(
            change(json!({ "action": "commands", "folder": "docs", "on": true }))
                .plan(&status)
                .is_err()
        );
        assert!(
            change(json!({ "action": "network", "folder": "docs", "on": true }))
                .plan(&status)
                .is_err()
        );
        assert!(
            change(json!({ "action": "rename", "folder": "docs", "label": "  " }))
                .plan(&status)
                .is_err()
        );
    }

    #[test]
    fn pages_cannot_ask_for_anything_else() {
        for refused in [
            json!({ "action": "add", "path": "/" }),
            json!({ "action": "shutdown" }),
            json!({ "cmd": "roots_add", "path": "/" }),
            json!({ "action": "lend", "folder": "app", "bot": "42" }),
        ] {
            assert!(
                serde_json::from_value::<Change>(refused.clone()).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn one_question_at_a_time() {
        let asking = Asking::default();
        let first = asking.start();
        assert!(first.is_some());
        assert!(asking.start().is_none());
        drop(first);
        assert!(asking.start().is_some());
    }

    #[test]
    fn the_state_goes_in_as_json() {
        let event = event(&json!({ "folders": [{ "label": "\");alert(1);//" }] }));
        assert!(event.contains(r#"{"folders":[{"label":"\");alert(1);//"}]}"#));
    }
}
