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
//!   changes, commands, their local services or the internet off. A
//!   compromised server could make the computer share less, never more.
//! - Widening happens only after the person says yes in a dialog the app
//!   draws, which no page can see or answer: resuming, allowing changes,
//!   commands, local services or the internet for commands, or one address
//!   a command was refused (`allowReach`, placed by this computer, never by
//!   the page). A folder is added only through the system's folder picker,
//!   where the person picks it.
//! - One question at a time, so a page can't stack dialogs.
//! - Pages never see where a folder is on this computer (`local_path`), the
//!   activity log, the deny list, the config file or the proxy. Those, and
//!   sharing folders with bots, stay in app-owned child webviews.

use std::sync::atomic::{AtomicBool, Ordering};

use cww::control::ControlRequest;
use cww::reach::Where;
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
            "commands": commands_supported(None),
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
        "commands": commands_supported(Some(status)),
    })
}

/// The legacy remote folder picker gets only the chosen label. Validation
/// errors can include absolute paths and private rules; those belong to the
/// app-owned Shared folders page, not the website's JavaScript.
pub fn added_folder<E>(answer: Result<Value, E>) -> Result<Value, String> {
    answer.map(|answer| json!({ "folder": answer["root"]["label"] }))
        .map_err(|_| "This folder could not be shared. Try adding it from Settings, This computer, Shared folders to see why.".into())
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
        "local_services": root.local_services(),
        "bots": root.bots.len(),
    })
}

/// Whether commands can run in folders here: on Windows only where the
/// daemon says so (Windows 11 24H2 and later, docs/commands.md).
fn commands_supported(status: Option<&Status>) -> bool {
    Platform::current() != Platform::Windows
        || status
            .and_then(|s| s.commands.as_ref())
            .is_some_and(|c| c.available)
}

/// Why commands can't run here, in the daemon's words if it said.
fn commands_unsupported(status: &Status) -> String {
    let Some(detail) = status.commands.as_ref().and_then(|c| c.detail.as_deref()) else {
        return "Commands can't run on this computer.".into();
    };
    let mut chars = detail.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => "Commands can't run on this computer.".into(),
    }
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
    LocalServices { folder: String, on: bool },
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
                            "Let Chat with Work change files in “{}”? Changes follow your approval settings in Chat with Work. Old versions and deleted files go to the trash on this computer.",
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
                if *on && !root.allows_commands && !commands_supported(Some(status)) {
                    return Err(commands_unsupported(status));
                }
                if *on && !root.writable {
                    return Err("Allow changes in this folder first.".into());
                }
                let question = if *on && !root.allows_commands {
                    ask(
                        "Allow Commands",
                        format!(
                            "Let Chat with Work run commands in “{}”? Each one is approved in Chat with Work, or runs without asking in a chat where you chose Allow everything. A command can change or delete anything in this folder, and nothing outside it. {}",
                            root.label,
                            if cfg!(windows) {
                                "It has no network unless you allow the internet."
                            } else {
                                "Its network is the folder’s: this computer’s own services, like a database, unless you set it to None, and the local network or the internet only if you allow them."
                            }
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
                        // Asking again for what's allowed keeps its network as it is.
                        network: *on && root.allows_commands && root.network,
                        local_services: None,
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
                        "Allow the Internet",
                        format!(
                            "Let commands in “{}” reach the internet and the local network? They can then install packages or fetch dependencies, and reach this computer's own services too.",
                            root.label
                        ),
                        "Allow the Internet",
                    )
                } else {
                    None
                };
                (
                    ControlRequest::RootsCommands {
                        root: root.id.clone(),
                        commands: true,
                        network: *on,
                        local_services: None,
                    },
                    question,
                )
            }
            Change::LocalServices { folder, on } => {
                let root = find(folder)?;
                if !root.allows_commands {
                    return Err("Allow commands in this folder first.".into());
                }
                let question = if *on && !root.local_services() {
                    ask(
                        "Allow Local Services",
                        format!(
                            "Let commands in “{}” reach this computer's own services, such as a database in Docker? They still can't reach the local network or the internet.",
                            root.label
                        ),
                        "Allow Local Services",
                    )
                } else {
                    None
                };
                (
                    ControlRequest::RootsLocalServices {
                        root: root.id.clone(),
                        on: *on,
                    },
                    question,
                )
            }
        })
    }
}

/// A page's `computer.allowReach`: a command in `folder` was refused
/// `host:port`, and the page asks to allow it, for its next command
/// (`once`) or for good. The page's words name nothing the person reads:
/// this computer places the address, and names the folder, the bot and the
/// service itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    pub folder: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub bot: Option<String>,
    #[serde(default)]
    pub once: bool,
}

/// What to do with a [`Reach`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReachPlan {
    /// Never reachable from commands (a port that leads out of the
    /// sandbox): declined without asking.
    Refused,
    /// Ask `question`; the person's answer goes to the daemon as `answer`
    /// with `allow` set.
    Ask {
        question: Question,
        answer: Box<ControlRequest>,
    },
}

impl Reach {
    /// `place` says where an address is, from this computer's own networks
    /// (`cww::reach::HostNetwork::classify`).
    pub fn plan(
        &self,
        status: &Status,
        place: impl Fn(std::net::IpAddr) -> Where,
    ) -> Result<ReachPlan, String> {
        let root = status
            .roots
            .iter()
            .find(|root| root.id == self.folder)
            .ok_or_else(|| "That folder isn't shared on this computer.".to_string())?;
        if !root.allows_commands {
            return Err("Allow commands in this folder first.".into());
        }
        let address = cww::reach::resolve(&self.host, self.port)
            .ok_or_else(|| "That isn't an address this computer can find.".to_string())?;
        if self.port == 0 || cww::reach::excluded(self.port) {
            return Ok(ReachPlan::Refused);
        }
        let place = place(address);
        let who = match &self.bot {
            Some(bot) => root
                .bots
                .iter()
                .find(|loan| loan.id == *bot)
                .map(|loan| {
                    if loan.name.trim().is_empty() {
                        format!("Bot {}", loan.id)
                    } else {
                        loan.name.clone()
                    }
                })
                .ok_or_else(|| "That bot isn't lent this folder.".to_string())?,
            None => "Chat with Work".to_string(),
        };
        // What this system's sandbox can give for it: nothing of this
        // computer's own on Windows, and the local network only with the
        // internet on macOS and Windows, which the question then says.
        let Some(given) = cww::reach::grantable(place) else {
            return Ok(ReachPlan::Refused);
        };
        // An exact address is IPv4 only (the sandbox's network has no IPv6),
        // so an IPv6 one on the local network can't be given.
        if address.is_ipv6() && given == Where::LocalNetwork {
            return Ok(ReachPlan::Refused);
        }
        let what = cww::reach::describe(&address.to_string(), self.port);
        let after = match (self.once, given) {
            (true, Where::Internet) if place != Where::Internet => {
                "This computer can only allow that with the internet: its next command there gets the internet, for that command alone."
            }
            (false, Where::Internet) if place != Where::Internet => {
                "This computer can only allow that with the internet: commands in that folder can then reach the internet and the local network."
            }
            (true, Where::Internet) => {
                "Only its next command there gets it: the internet, for that command alone."
            }
            (true, _) => "Only its next command there gets it.",
            (false, Where::ThisComputer) => {
                "Commands in that folder can then reach this computer's own services."
            }
            (false, Where::LocalNetwork) => {
                "Commands in that folder can then reach this address, and nothing else on the local network."
            }
            (false, Where::Internet) => {
                "Commands in that folder can then reach the internet and the local network."
            }
        };
        let question = Question {
            title: "Allow a Connection".into(),
            message: format!(
                "Let {who} reach {what} from {} {}? {after}",
                root.label,
                place_words(place)
            ),
            yes: "Allow".into(),
        };
        // Bound to what the question named: this pairing and this folder.
        let answer = ControlRequest::RootsReachAnswer {
            root: root.id.clone(),
            host: address.to_string(),
            port: self.port,
            bot: self.bot.clone(),
            once: self.once,
            allow: true,
            device: status.device_id.clone(),
            path: root
                .local_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            server: status.server.clone(),
            place: Some(given),
        };
        Ok(ReachPlan::Ask {
            question,
            answer: Box::new(answer),
        })
    }
}

/// "on this computer", "on the local network", "on the internet".
fn place_words(place: Where) -> String {
    format!("on {}", place.words())
}

/// The same answer, declined.
pub fn declined(answer: &ControlRequest) -> ControlRequest {
    let mut answer = answer.clone();
    if let ControlRequest::RootsReachAnswer { allow, .. } = &mut answer {
        *allow = false;
    }
    answer
}

/// Only one of the app's questions shows at a time.
#[derive(Default)]
pub struct Asking(AtomicBool);

impl Asking {
    /// Holds the right to ask until the guard drops, or says a question is
    /// already showing.
    pub fn start(&self) -> Option<AskingGuard<'_>> {
        (!self.0.swap(true, Ordering::SeqCst)).then(|| AskingGuard(&self.0))
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
    fn folder_picker_failures_do_not_reveal_the_chosen_path_or_private_rule() {
        let temp = tempfile::tempdir().unwrap();
        let paths = cww::paths::Paths::under(&temp.path().join("private-app-state"));
        let chosen = temp.path().join("private-client-folder");
        std::fs::create_dir(&chosen).unwrap();
        let mut config = cww::config::Config::default();
        config.deny.extra.push("private-client-*".into());
        let failure = cww::roots::add_root(
            &mut config,
            &paths,
            cww::roots::NewRoot {
                path: &chosen,
                label: None,
                i_know: false,
                follow_symlinks: false,
                writable: false,
            },
        )
        .unwrap_err();
        let detail = failure.to_string();
        assert!(detail.contains("private-client-*"));
        assert!(detail.contains("private-app-state"));
        let remote = added_folder(Err::<Value, _>(failure)).unwrap_err();
        for private in [
            chosen.to_string_lossy().as_ref(),
            "private-client-*",
            "private-app-state",
        ] {
            assert!(!remote.contains(private), "{remote}");
        }
        assert_eq!(
            added_folder::<()>(Ok(json!({"root":{"label":"Work","path":chosen}}))).unwrap(),
            json!({"folder":"Work"})
        );
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
    fn one_question_at_a_time_however_often_another_tries() {
        let asking = Asking::default();
        let first = asking.start().expect("nothing is showing");
        // A refused start must leave the first question's hold in place.
        assert!(asking.start().is_none());
        assert!(asking.start().is_none());
        drop(first);
        assert!(asking.start().is_some());
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
            json!({ "action": "local_services", "folder": "app", "on": false }),
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
    fn allowing_commands_again_keeps_their_network() {
        let mut status = status();
        status.roots[1].network = true;
        let (request, _) = change(json!({ "action": "commands", "folder": "app", "on": true }))
            .plan(&status)
            .unwrap();
        assert_eq!(
            request,
            ControlRequest::RootsCommands {
                root: "app".into(),
                commands: true,
                network: true,
                local_services: None,
            }
        );
    }

    #[test]
    fn local_services_show_and_widen_only_after_a_question() {
        let mut status = status();
        assert_eq!(
            view(Some(&status))["folders"][1]["local_services"],
            json!(false)
        );
        let (request, question) =
            change(json!({ "action": "local_services", "folder": "app", "on": true }))
                .plan(&status)
                .unwrap();
        assert_eq!(
            request,
            ControlRequest::RootsLocalServices {
                root: "app".into(),
                on: true
            }
        );
        assert_eq!(question.expect("a question").yes, "Allow Local Services");
        status.roots[1].network_access = Some("local".into());
        assert_eq!(
            view(Some(&status))["folders"][1]["local_services"],
            json!(true)
        );
        let (_, question) =
            change(json!({ "action": "local_services", "folder": "app", "on": true }))
                .plan(&status)
                .unwrap();
        assert!(question.is_none());
        assert!(
            change(json!({ "action": "local_services", "folder": "docs", "on": true }))
                .plan(&status)
                .is_err()
        );
    }

    fn reach(json: Value) -> Reach {
        serde_json::from_value(json).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn windows_is_never_asked_for_this_computers_own_services() {
        let asked = reach(json!({
            "folder": "app", "host": "172.17.0.4", "port": 5432, "bot": "42", "once": false
        }));
        assert_eq!(
            asked.plan(&status(), |_| Where::ThisComputer).unwrap(),
            ReachPlan::Refused
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_reach_is_asked_in_this_computers_words_never_the_pages() {
        let status = status();
        let asked = reach(json!({
            "folder": "app", "host": "172.17.0.4", "port": 5432, "bot": "42", "once": false
        }));
        // The page can't say where the address is: this computer does.
        let ReachPlan::Ask { question, answer } =
            asked.plan(&status, |_| Where::ThisComputer).unwrap()
        else {
            panic!("a question");
        };
        assert_eq!(
            question.message,
            "Let Fixer reach PostgreSQL (172.17.0.4:5432) from App on this computer? \
             Commands in that folder can then reach this computer's own services."
        );
        assert_eq!(question.yes, "Allow");
        assert_eq!(
            *answer,
            ControlRequest::RootsReachAnswer {
                root: "app".into(),
                host: "172.17.0.4".into(),
                port: 5432,
                bot: Some("42".into()),
                once: false,
                allow: true,
                device: status.device_id.clone(),
                path: Some("/home/ana/app".into()),
                server: status.server.clone(),
                place: Some(Where::ThisComputer),
            }
        );
        assert!(matches!(
            declined(&answer),
            ControlRequest::RootsReachAnswer { allow: false, .. }
        ));
        let ReachPlan::Ask { question, .. } = reach(json!({
            "folder": "app", "host": "192.168.1.20", "port": 6379, "once": true
        }))
        .plan(&status, |_| Where::LocalNetwork)
        .unwrap() else {
            panic!("a question");
        };
        assert!(question.message.starts_with(
            "Let Chat with Work reach Redis (192.168.1.20:6379) from App on the local network?"
        ));
        // Only Linux gives one address on the local network; elsewhere the
        // question says it takes the internet.
        assert_eq!(
            question
                .message
                .contains("only allow that with the internet"),
            cfg!(any(windows, target_os = "macos"))
        );
        let ReachPlan::Ask { question, .. } = reach(json!({
            "folder": "app", "host": "151.101.1.227", "port": 443, "bot": "42", "once": true
        }))
        .plan(&status, |_| Where::Internet)
        .unwrap() else {
            panic!("a question");
        };
        assert!(question.message.contains("from App on the internet?"));
    }

    #[test]
    fn a_reach_is_refused_where_it_would_leave_the_sandbox_or_names_nothing_real() {
        let status = status();
        let at = |json: Value| reach(json).plan(&status, |_| Where::ThisComputer);
        assert_eq!(
            at(json!({ "folder": "app", "host": "127.0.0.1", "port": 2375 })).unwrap(),
            ReachPlan::Refused
        );
        assert!(at(json!({ "folder": "app", "host": "db.example", "port": 5432 })).is_err());
        assert!(at(json!({ "folder": "docs", "host": "127.0.0.1", "port": 5432 })).is_err());
        assert!(
            at(json!({ "folder": "app", "host": "127.0.0.1", "port": 5432, "bot": "7" })).is_err()
        );
        assert!(
            serde_json::from_value::<Reach>(json!({
                "folder": "app", "host": "127.0.0.1", "port": 5432, "where": "this_computer"
            }))
            .is_err()
        );
    }

    #[test]
    fn commands_run_only_where_the_daemon_says_this_windows_can() {
        let mut status = status();
        status.roots[1].allows_commands = false;
        let allow = || change(json!({ "action": "commands", "folder": "app", "on": true }));
        status.commands = Some(crate::model::CommandsHost {
            available: false,
            detail: Some("commands need Windows 11 24H2 or later".into()),
            drives: Vec::new(),
        });
        let planned = allow().plan(&status);
        if cfg!(windows) {
            assert_eq!(
                planned.unwrap_err(),
                "Commands need Windows 11 24H2 or later."
            );
            assert_eq!(view(Some(&status))["commands"], json!(false));
        } else {
            assert!(planned.is_ok(), "only Windows depends on the daemon's word");
        }
        status.commands = Some(crate::model::CommandsHost {
            available: true,
            ..Default::default()
        });
        assert!(allow().plan(&status).is_ok());
        assert_eq!(view(Some(&status))["commands"], json!(true));
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
