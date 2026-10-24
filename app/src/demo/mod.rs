//! `--demo`: a stand-in daemon with sample folders, activity, chats and a
//! paired account, speaking the real control protocol on a private socket.
//! For screenshots, for trying the app without pairing a computer, and for
//! the window's tests, which also read the requests it received. Built only
//! with the `demo` feature, and in tests.

mod assets;
mod chats;
mod people;
mod project_pages;
mod schedules;
mod templates;

/// Northwind's projects, as the demo's list sends them.
#[cfg(test)]
pub fn projects() -> Vec<Value> {
    chats::projects()
}

/// One more project, invite-only.
#[cfg(test)]
pub fn project(id: u64, name: &str, icon: &str) -> Value {
    chats::project(id, name, icon, false, false)
}

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::paths::Paths;

struct State {
    status: Value,
    log: Vec<Value>,
    generation: u64,
    requests: Vec<Value>,
    chats: Vec<chats::Chat>,
    /// Every chat update so far, as followers get them.
    chat_events: Vec<Value>,
    /// Every change to the list so far, as `chats` subscribers get them.
    list_events: Vec<Value>,
    /// The chats as the list's subscribers last heard of them.
    listed: Vec<Value>,
    /// Refuse the `chats` topic, as a daemon from before live lists does.
    old_daemon: bool,
    /// Web search is available in the list and the first model.
    web_search: bool,
    /// The organization's credits left, of 5,000.
    credits_left: u64,
    /// The projects the person is on.
    projects: Vec<Value>,
    project_catalog: Vec<Value>,
    pins: Value,
    /// Answers don't stream on their own (tests hold them still).
    held: bool,
    /// Chats whose answer is streaming now.
    streaming: HashSet<u64>,
    /// Chats asked to stop.
    cancelled: HashSet<u64>,
    next_id: u64,
    /// Files uploaded for questions, by `signed_id`.
    uploads: Vec<Value>,
    workspace: Value,
    notification_preferences: Value,
    preferred_model: Value,
    profile: Value,
    copy_answers_as: Value,
    templates: Vec<Value>,
    schedules: Vec<Value>,
    people: Value,
    notification_permission: String,
    instructions: Vec<Value>,
    memory: Value,
    /// The commands this computer runs (`commands`), and what each printed.
    commands: Vec<Value>,
    printed: HashMap<String, String>,
}

struct Demo {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Paired, two folders, a day of activity.
    Sample,
    /// Running, but not paired and sharing nothing: a first run.
    Fresh,
}

/// A running stand-in daemon.
pub struct Server {
    pub paths: Paths,
    demo: Arc<Demo>,
}

impl Server {
    /// Pairing and logging out, without a Chat with Work server.
    pub fn account(&self) -> Arc<dyn crate::pairing::Account> {
        Arc::new(Account(Arc::clone(&self.demo)))
    }

    /// Every request received so far, except `status` and `watch`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn requests(&self) -> Vec<Value> {
        self.demo.state.lock().expect("demo lock").requests.clone()
    }

    /// Hold answers still: nothing streams until released, so tests see
    /// one moment of it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hold_streams(&self, held: bool) {
        self.demo.change(|s| s.held = held);
    }

    /// Finish a held answer in chat `chat`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn finish(&self, chat: u64, message: u64, content: &str) {
        self.demo.apply(
            chat,
            &chats::Step::Finish {
                id: message,
                content: content.to_string(),
            },
        );
    }

    /// Refuse the `chats` topic, as a daemon from before live lists does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn act_old(&self) {
        self.demo.change(|s| s.old_daemon = true);
    }

    #[cfg(test)]
    pub fn set_notification_permission(&self, permission: &str) {
        self.demo
            .change(|s| s.notification_permission = permission.into());
    }

    #[cfg(test)]
    pub fn enable_web_search(&self) {
        self.demo.change(|s| s.web_search = true);
    }

    /// A chat started on the web: it shows up being answered.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn start_elsewhere(&self, number: u64, title: &str, question: &str) {
        self.demo.change(|s| {
            let mut summary = chats::summary(number, title, "processing", ago(0));
            summary["url"] = json!(format!("https://chatwithwork.com/northwind/chats/{number}"));
            s.chats.insert(
                0,
                chats::Chat::new(
                    summary,
                    vec![json!({ "kind": "user", "id": number * 10 + 1, "content": question })],
                ),
            );
        });
        self.demo.sync_list();
    }

    /// A message queued in chat `number` by someone else in its project,
    /// as the web's composer queues it while the answer runs.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn queue_elsewhere(
        &self,
        number: u64,
        author: &str,
        content: &str,
        steer: bool,
        files: u64,
    ) {
        self.demo.change(|s| {
            let id = s.next_id;
            s.next_id += 10;
            if let Some(chat) = s
                .chats
                .iter_mut()
                .find(|c| c.summary["number"] == json!(number))
            {
                let mut queued = json!({ "id": id, "content": content, "steer": steer,
                                         "queued_at": ago(0), "author": author });
                if files > 0 {
                    queued["attachments"] = json!(files);
                }
                chat.queued.push(queued);
            }
        });
        self.demo.changed(number);
    }

    /// Change a chat as someone elsewhere would: a new title, say.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn edit_elsewhere(&self, number: u64, edit: impl FnOnce(&mut Value)) {
        self.demo.change(|s| {
            if let Some(chat) = s
                .chats
                .iter_mut()
                .find(|c| c.summary["number"] == json!(number))
            {
                edit(&mut chat.summary);
                chat.summary["updated_at"] = json!(ago(0));
            }
        });
        self.demo.sync_list();
    }

    /// A chat deleted on the web, or moved out of sight.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn delete_elsewhere(&self, number: u64) {
        self.demo
            .change(|s| s.chats.retain(|c| c.summary["number"] != json!(number)));
        self.demo.sync_list();
    }

    /// Commands run on this computer: each a `commands` entry, with what
    /// it printed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn run_commands(&self, commands: Vec<(Value, &str)>) {
        self.demo.change(|s| {
            s.printed = commands
                .iter()
                .map(|(c, text)| {
                    (
                        c["id"].as_str().unwrap_or_default().to_string(),
                        text.to_string(),
                    )
                })
                .collect();
            s.commands = commands.into_iter().map(|(c, _)| c).collect();
        });
    }

    /// The credits move elsewhere: the list's header says so.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spend(&self, left: u64) {
        self.demo.change(|s| {
            s.credits_left = left;
            let mut update = chats::account_update(left);
            update["copy_answers_as"] = s.copy_answers_as.clone();
            update["user"]["name"] = s.profile["display_name"].clone();
            s.list_events.push(update);
        });
    }

    /// The projects change elsewhere: joined, renamed, left.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_projects(&self, projects: Vec<Value>) {
        self.demo.change(|s| {
            s.projects = projects.clone();
            s.list_events
                .push(json!({ "event": "projects", "projects": projects }));
        });
    }

    /// What happens elsewhere while `--demo` runs: a chat started on the
    /// web shows up, is answered and renamed, and the credits run low.
    #[cfg_attr(test, allow(dead_code))]
    pub fn script_live_events(&self) {
        let me = Server {
            paths: self.paths.clone(),
            demo: Arc::clone(&self.demo),
        };
        std::thread::spawn(move || {
            let pause = |s| std::thread::sleep(Duration::from_secs(s));
            pause(20);
            me.start_elsewhere(
                30,
                "Draft the Q4 hiring plan",
                "Draft the Q4 hiring plan from the headcount sheet.",
            );
            pause(6);
            me.finish(30, 302, "Here's a first draft of the Q4 hiring plan.");
            pause(4);
            me.edit_elsewhere(30, |c| c["title"] = json!("Q4 hiring plan"));
            pause(10);
            me.spend(410);
        });
    }
}

fn ago(seconds: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(seconds)).to_string()
}

fn fresh() -> State {
    let home = crate::paths::home_dir().unwrap_or_else(|| PathBuf::from("/home/me"));
    let status = json!({
        "ok": true,
        "version": "0.1.0",
        "pid": 4242,
        "connection": { "connection": "not_paired", "since": ago(5) },
        "server": null,
        "device_id": null,
        "paused": false,
        "roots": [],
        "pairing": null,
        "config_file": home.join(".config/cww/config.toml"),
        "audit_file": home.join(".local/state/cww/audit.jsonl"),
    });
    State {
        status,
        log: vec![json!({ "ts": ago(5), "event": "started", "detail": "cww 0.1.0" })],
        generation: 0,
        requests: Vec::new(),
        listed: chats::sample().into_iter().map(|c| c.summary).collect(),
        chats: chats::sample(),
        chat_events: Vec::new(),
        list_events: Vec::new(),
        old_daemon: false,
        web_search: false,
        credits_left: 1250,
        projects: chats::projects(),
        project_catalog: project_pages::catalog(),
        pins: chats::list_header(5000)["pins"].clone(),
        held: false,
        streaming: HashSet::new(),
        cancelled: HashSet::new(),
        next_id: 1000,
        uploads: Vec::new(),
        workspace: chats::workspace(),
        notification_preferences: chats::notification_preferences(),
        preferred_model: json!(12),
        schedules: vec![],
        people: Value::Null,
        profile: json!({"full_name":"Alex Rivera","display_name":"Alex"}),
        copy_answers_as: json!("rich_text"),
        templates: vec![],
        notification_permission: "unknown".into(),
        instructions: chats::instructions(),
        memory: json!({ "enabled": true, "entries": [{ "id": 1, "body": "Prefers short answers", "date": "October 16, 2026", "chat": 12 }] }),
        commands: Vec::new(),
        printed: HashMap::new(),
    }
}

fn sample() -> State {
    let home = crate::paths::home_dir().unwrap_or_else(|| PathBuf::from("/home/me"));
    let tool = |secs: i64, tool: &str, path: Option<&str>, query: Option<&str>, decision: &str| {
        let mut e = json!({ "ts": ago(secs), "event": "tool", "tool": tool, "decision": decision, "chat_id": "1842" });
        if let Some(path) = path {
            e["path"] = json!(path);
        }
        if let Some(query) = query {
            e["query"] = json!(query);
            e["results"] = json!(6);
        }
        if decision == "allowed" && tool == "read" {
            e["bytes"] = json!(18_432);
        }
        if decision == "denied" {
            e["code"] = json!("denied");
            e["reason"] = json!("this path is on the deny list (.env*)");
        }
        e
    };
    let log = vec![
        json!({ "ts": ago(26 * 3600), "event": "started", "detail": "cww 0.1.0" }),
        json!({ "ts": ago(26 * 3600 - 2), "event": "connected", "detail": "wss://chatwithwork.com/desktop as device 42" }),
        tool(
            25 * 3600,
            "search",
            None,
            Some("vendor contract renewal"),
            "allowed",
        ),
        tool(
            25 * 3600 - 30,
            "read",
            Some("documents:Contracts/Acme renewal 2026.pdf"),
            None,
            "allowed",
        ),
        json!({ "ts": ago(3 * 3600), "event": "paused" }),
        json!({ "ts": ago(3 * 3600 - 600), "event": "resumed" }),
        tool(40 * 60, "roots", None, None, "allowed"),
        tool(39 * 60, "search", None, Some("Q3 roadmap"), "allowed"),
        tool(
            38 * 60,
            "list",
            Some("work-projects:Falcon"),
            None,
            "allowed",
        ),
        tool(
            37 * 60,
            "read",
            Some("work-projects:Falcon/.env"),
            None,
            "denied",
        ),
        tool(
            36 * 60,
            "read",
            Some("documents:Plans/Q3 roadmap.md"),
            None,
            "allowed",
        ),
        tool(
            2 * 60,
            "read",
            Some("documents:Plans/Budget 2027.xlsx"),
            None,
            "allowed",
        ),
        json!({ "ts": ago(90), "event": "tool", "tool": "edit", "chat_id": "1234",
                "path": "work-projects:Falcon/README.md", "decision": "allowed",
                "effect": "edited", "written": 4_210,
                "trash": home_trash(&home).join("README.md"), "bytes": 412 }),
    ];
    let status = json!({
        "ok": true,
        "version": "0.1.0",
        "pid": 4242,
        "connection": { "connection": "connected", "since": ago(26 * 3600 - 2) },
        "server": "https://chatwithwork.com",
        "device_id": "42",
        "paused": false,
        "roots": [
            { "id": "documents", "label": "Documents", "available": true, "index": "ready",
              "indexed_files": 1532, "local_path": home.join("Documents") },
            { "id": "work-projects", "label": "Work projects", "available": true, "index": "indexing",
              "indexed_files": 214, "local_path": home.join("Work/Projects"), "writable": true,
              "allows_commands": true, "bots": [{ "id": "42", "name": "Issue fixer", "unattended": false }] },
        ],
        "pairing": null,
        "config_file": home.join(".config/cww/config.toml"),
        "audit_file": home.join(".local/state/cww/audit.jsonl"),
    });
    State {
        status,
        log,
        generation: 0,
        requests: Vec::new(),
        listed: chats::sample().into_iter().map(|c| c.summary).collect(),
        chats: chats::sample(),
        chat_events: Vec::new(),
        list_events: Vec::new(),
        old_daemon: false,
        web_search: false,
        credits_left: 1250,
        projects: chats::projects(),
        project_catalog: project_pages::catalog(),
        pins: chats::list_header(5000)["pins"].clone(),
        held: false,
        streaming: HashSet::new(),
        cancelled: HashSet::new(),
        next_id: 1000,
        uploads: Vec::new(),
        workspace: chats::workspace(),
        notification_preferences: chats::notification_preferences(),
        preferred_model: json!(12),
        schedules: vec![],
        people: Value::Null,
        profile: json!({"full_name":"Alex Rivera","display_name":"Alex"}),
        copy_answers_as: json!("rich_text"),
        templates: vec![],
        notification_permission: "unknown".into(),
        instructions: chats::instructions(),
        memory: json!({ "enabled": true, "entries": [{ "id": 1, "body": "Prefers short answers", "date": "October 16, 2026", "chat": 12 }] }),
        commands: Vec::new(),
        printed: HashMap::new(),
    }
}

/// Where the sample's trash would be, for the activity log.
fn home_trash(home: &std::path::Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join(".Trash")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\$Recycle.Bin")
    } else {
        home.join(".local/share/Trash/files")
    }
}

fn deny() -> Value {
    json!({
        "ok": true,
        "builtin": [".ssh", ".gnupg", ".aws", ".azure", ".config/gcloud", ".kube",
            ".docker/config.json", ".netrc", ".npmrc", ".pypirc", ".git-credentials",
            ".config/gh/hosts.yml", ".env*", "*.pem", "*.key", "*.p12", "*.pfx", "id_*",
            "*.kdbx", "*.1pux", ".password-store", ".local/share/keyrings", "Library/Keychains",
            "*.keychain", "*.keychain-db", "Library/Mail", "Library/Messages", ".mozilla",
            ".config/google-chrome", "Library/Safari", "Library/Cookies", "/etc/shadow"],
        "extra": ["*.secret", "Clients/Confidential"],
        "removed": [],
        "own_dirs": [],
        "allow_hardlinks": false,
        "config_file": crate::paths::home_dir().map(|h| h.join(".config/cww/config.toml")),
    })
}

impl Demo {
    fn change(&self, edit: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().expect("demo lock");
        edit(&mut state);
        state.generation += 1;
        self.changed.notify_all();
    }

    fn event(state: &mut State, event: &str) {
        state.log.push(json!({ "ts": ago(0), "event": event }));
    }

    fn answer(self: &Arc<Self>, request: &Value) -> Value {
        let cmd = request["cmd"].as_str().unwrap_or_default();
        // The command list is read every few seconds.
        if cmd != "status" && cmd != "commands" {
            self.state
                .lock()
                .expect("demo lock")
                .requests
                .push(request.clone());
        }
        match cmd {
            "status" => self.state.lock().expect("demo lock").status.clone(),
            "pause" | "resume" => {
                let paused = cmd == "pause";
                self.change(|s| {
                    s.status["paused"] = json!(paused);
                    Self::event(s, if paused { "paused" } else { "resumed" });
                });
                json!({ "ok": true, "paused": paused })
            }
            "roots_add" => {
                let path = PathBuf::from(request["path"].as_str().unwrap_or_default());
                let label = request["label"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "Folder".into());
                let id: String = label
                    .to_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                let root = json!({ "id": id, "label": label, "available": true, "index": "indexing",
                                   "indexed_files": 0, "local_path": path });
                self.change(|s| {
                    s.status["roots"]
                        .as_array_mut()
                        .expect("roots")
                        .push(root.clone());
                    Self::event(s, "reloaded");
                });
                let me = Arc::clone(self);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(3));
                    me.change(|s| {
                        for r in s.status["roots"].as_array_mut().expect("roots") {
                            if r["id"] == json!(id) {
                                r["index"] = json!("ready");
                                r["indexed_files"] = json!(87);
                            }
                        }
                    });
                });
                json!({ "ok": true, "root": root })
            }
            "roots_writable" => {
                let which = request["root"].as_str().unwrap_or_default().to_string();
                let writable = request["writable"].as_bool().unwrap_or(false);
                self.change(|s| {
                    for r in s.status["roots"].as_array_mut().expect("roots") {
                        if r["id"] == json!(which) {
                            r["writable"] = json!(writable);
                        }
                    }
                    Self::event(
                        s,
                        if writable {
                            "root_changes_allowed"
                        } else {
                            "root_changes_stopped"
                        },
                    );
                });
                json!({ "ok": true })
            }
            // Lending a folder to a bot, as the daemon's `roots_bot`.
            "roots_bot" => {
                let which = request["root"].as_str().unwrap_or_default().to_string();
                let bot = request["bot"].as_str().unwrap_or_default().to_string();
                let lent = request["lent"].as_bool().unwrap_or(false);
                let unattended = request["unattended"].as_bool().unwrap_or(false);
                let name = request["name"].as_str().map(str::to_string);
                if bot.is_empty() || !bot.bytes().all(|b| b.is_ascii_digit()) {
                    return json!({ "ok": false, "error": "that isn't a bot's ID" });
                }
                self.change(|s| {
                    for r in s.status["roots"].as_array_mut().expect("roots") {
                        if r["id"] != json!(which) {
                            continue;
                        }
                        let mut loans: Vec<Value> = r["bots"].as_array().cloned().unwrap_or_default();
                        let kept = loans
                            .iter()
                            .find(|l| l["id"] == json!(bot))
                            .and_then(|l| l["name"].as_str().map(str::to_string));
                        loans.retain(|l| l["id"] != json!(bot));
                        if lent {
                            loans.push(json!({ "id": bot, "name": name.clone().or(kept).unwrap_or_default(), "unattended": unattended }));
                        }
                        r["bots"] = json!(loans);
                    }
                    Self::event(s, if lent { "root_bot_lent" } else { "root_bot_withdrawn" });
                });
                json!({ "ok": true })
            }
            "commands" => {
                json!({ "ok": true, "commands": self.state.lock().expect("demo lock").commands })
            }
            "command_output" => {
                let state = self.state.lock().expect("demo lock");
                let run = request["run"].as_str().unwrap_or_default();
                let Some(text) = state.printed.get(run) else {
                    return json!({ "ok": false, "code": "not_found", "error": "no such command" });
                };
                let offset = request["offset"].as_u64().unwrap_or(0) as usize;
                let rest = text.get(offset.min(text.len())..).unwrap_or_default();
                let running = state
                    .commands
                    .iter()
                    .any(|c| c["id"] == json!(run) && c["state"] == json!("running"));
                json!({ "ok": true, "id": run, "offset": offset, "next_offset": offset + rest.len(),
                        "skipped": 0, "text": rest, "done": !running })
            }
            "command_stop" => {
                let run = request["run"].as_str().unwrap_or_default().to_string();
                let mut stopped = Value::Null;
                self.change(|s| {
                    for c in &mut s.commands {
                        if c["id"] == json!(run) {
                            c["state"] = json!("stopped");
                            stopped = c.clone();
                        }
                    }
                    Self::event(s, "command_stopped");
                });
                if stopped.is_null() {
                    json!({ "ok": false, "code": "not_found", "error": "no such command" })
                } else {
                    stopped["ok"] = json!(true);
                    stopped
                }
            }
            "roots_commands" => {
                let which = request["root"].as_str().unwrap_or_default().to_string();
                let commands = request["commands"].as_bool().unwrap_or(false);
                let network = commands && request["network"].as_bool().unwrap_or(false);
                self.change(|s| {
                    for r in s.status["roots"].as_array_mut().expect("roots") {
                        if r["id"] == json!(which) {
                            r["allows_commands"] = json!(commands);
                            r["network"] = json!(network);
                        }
                    }
                    Self::event(
                        s,
                        if commands {
                            "root_commands_allowed"
                        } else {
                            "root_commands_stopped"
                        },
                    );
                });
                json!({ "ok": true })
            }
            "roots_remove" | "roots_label" => {
                let which = request["root"].as_str().unwrap_or_default().to_string();
                let label = request["label"].as_str().map(str::to_string);
                self.change(|s| {
                    let roots = s.status["roots"].as_array_mut().expect("roots");
                    match &label {
                        Some(label) => {
                            for r in roots.iter_mut().filter(|r| r["id"] == json!(which)) {
                                r["label"] = json!(label);
                            }
                        }
                        None => roots.retain(|r| r["id"] != json!(which)),
                    }
                    Self::event(s, "reloaded");
                });
                json!({ "ok": true })
            }
            "chats"
            | "chat"
            | "chat_send"
            | "chat_cancel"
            | "chat_access"
            | "models"
            | "templates"
            | "template_upload"
            | "people"
            | "account_settings"
            | "schedules"
            | "model_preferences"
            | "model_preferences_update"
            | "chat_download"
            | "chat_upload"
            | "chat_approve"
            | "chat_deny"
            | "chat_answer"
            | "chat_decline"
            | "chat_retry"
            | "chat_branch"
            | "chat_rename"
            | "chat_delete"
            | "chat_pin"
            | "chat_history"
            | "projects"
            | "chat_move"
            | "chat_share"
            | "chat_unshare"
            | "workspace"
            | "service_pause"
            | "memory_preference"
            | "memory_forget"
            | "instructions"
            | "instruction_update"
            | "instruction_count"
            | "computer_remove"
            | "connectors"
            | "connector_handoff"
            | "notification_preferences"
            | "notification_preferences_update"
            | "notification_browser_link"
            | "scan_destinations"
            | "scan_send"
            | "chat_unqueue" => {
                let answer = self.chat_request(cmd, request);
                self.sync_list();
                answer
            }
            "notification_status" => {
                json!({"ok": true, "available": true, "permission": self.state.lock().expect("demo lock").notification_permission, "connected": true, "account": "482139075"})
            }
            "notification_permission" | "notification_view" => json!({"ok": true}),
            "asset" => assets::serve(request["path"].as_str().unwrap_or_default()),
            "dictation" => {
                json!({ "ok": true, "cloud": false, "reason": "Dictation is off in the demo." })
            }
            "deny" => deny(),
            "audit_tail" => {
                let n = request["lines"].as_u64().unwrap_or(50) as usize;
                let log = &self.state.lock().expect("demo lock").log;
                json!({ "ok": true, "entries": log[log.len().saturating_sub(n)..] })
            }
            _ => json!({ "ok": false, "error": format!("the demo doesn't do {cmd:?}") }),
        }
    }

    fn serve(self: Arc<Self>, stream: impl Read + Write) {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
            let request: Value = serde_json::from_str(&line).unwrap_or_default();
            line.clear();
            if request["cmd"] == "subscribe"
                && request["topics"]
                    .as_array()
                    .is_some_and(|t| t.iter().any(|t| t == "chat"))
            {
                self.state
                    .lock()
                    .expect("demo lock")
                    .requests
                    .push(request.clone());
                return self.follow(
                    reader.get_mut(),
                    request["chat"].as_str().unwrap_or_default(),
                );
            }
            if request["cmd"] == "subscribe"
                && request["topics"]
                    .as_array()
                    .is_some_and(|t| t.iter().any(|t| t == "chats"))
            {
                self.state
                    .lock()
                    .expect("demo lock")
                    .requests
                    .push(request.clone());
                return self.follow_list(reader.get_mut());
            }
            if request["cmd"] == "subscribe" {
                self.state
                    .lock()
                    .expect("demo lock")
                    .requests
                    .push(request.clone());
                let topics = json!(["status", "audit"]);
                if send(reader.get_mut(), &json!({ "ok": true, "topics": topics })).is_err() {
                    return;
                }
                // A status event now and after each change, and an audit
                // event for each new log entry, as the daemon sends them.
                let mut seen = u64::MAX;
                let mut logged = self.state.lock().expect("demo lock").log.len();
                loop {
                    let (status, entries) = {
                        let mut state = self.state.lock().expect("demo lock");
                        while state.generation == seen {
                            state = self.changed.wait(state).expect("demo lock");
                        }
                        seen = state.generation;
                        let entries = state.log[logged.min(state.log.len())..].to_vec();
                        logged = state.log.len();
                        (state.status.clone(), entries)
                    };
                    for entry in entries {
                        if send(
                            reader.get_mut(),
                            &json!({ "event": "audit", "entry": entry }),
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                    let mut status = status;
                    if let Some(map) = status.as_object_mut() {
                        map.remove("ok");
                    }
                    if send(
                        reader.get_mut(),
                        &json!({ "event": "status", "status": status }),
                    )
                    .is_err()
                    {
                        return;
                    }
                }
            }
            if send(reader.get_mut(), &self.answer(&request)).is_err() {
                return;
            }
        }
    }
}

impl Demo {
    /// The chat API (CONTROL.md, "Chats"), for a paired sample.
    fn chat_request(self: &Arc<Self>, cmd: &str, request: &Value) -> Value {
        let paired = !self.state.lock().expect("demo lock").status["server"].is_null();
        if !paired {
            return json!({ "ok": false, "code": "not_paired", "error": "This computer isn't paired." });
        }
        let number = request["chat"].as_str().and_then(|c| c.parse::<u64>().ok());
        match cmd {
            "projects" => {
                let mut state = self.state.lock().expect("demo lock");
                project_pages::request(&mut state, &request["request"])
            }
            "chat_history" => {
                let state = self.state.lock().expect("demo lock");
                let query = request["query"].as_str().unwrap_or_default().to_lowercase();
                let items: Vec<_> = state
                    .listed
                    .iter()
                    .filter(|chat| {
                        chat["title"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&query)
                    })
                    .map(|chat| json!({"chat": chat, "group": "Today", "age": "about 1 hour ago"}))
                    .collect();
                json!({"ok": true, "total": state.listed.len(), "items": items, "next_cursor": null})
            }
            "notification_preferences" => self
                .state
                .lock()
                .expect("demo lock")
                .notification_preferences
                .clone(),
            "notification_preferences_update" => {
                let mut state = self.state.lock().expect("demo lock");
                let preferences = &mut state.notification_preferences;
                let update = &request["preferences"];
                if update["account"] != preferences["account"]["id"] {
                    return json!({"ok": false, "code": "stale_account", "error": "The organization changed. Reload its preferences."});
                }
                for choice in preferences["chat_notifications"].as_array_mut().unwrap() {
                    let key = choice["key"].as_str().unwrap().to_string();
                    for channel in ["push", "email"] {
                        if update["chat_notifications"][&key][channel].is_boolean() {
                            choice[channel] = update["chat_notifications"][&key][channel].clone();
                        }
                    }
                }
                for choice in preferences["notification_emails"].as_array_mut().unwrap() {
                    let key = choice["key"].as_str().unwrap();
                    if update["notification_emails"][key].is_boolean() {
                        choice["enabled"] = update["notification_emails"][key].clone();
                    }
                }
                if update["notification_content"].is_boolean() {
                    preferences["notification_content"] = update["notification_content"].clone();
                }
                preferences.clone()
            }
            "notification_browser_link" => {
                json!({"ok": true, "path": "/482139075/native/notification_browser_link?token=synthetic-demo"})
            }
            "workspace" => {
                let mut state = self.state.lock().expect("demo lock");
                let mut workspace = state.workspace.clone();
                if let Some(project) = request["project"].as_u64() {
                    let Some(instruction) = project_pages::instruction(&mut state, project) else {
                        return json!({"ok": false, "code": "not_found", "error": "No such project"});
                    };
                    workspace["instructions"].as_array_mut().unwrap().push(json!({"layer": "project", "owner": instruction["owner"], "written": !instruction["body"].as_str().unwrap_or_default().is_empty(), "editable": true}));
                }
                workspace
            }
            "connectors" => {
                let state = self.state.lock().expect("demo lock");
                chats::connectors(
                    &state.workspace,
                    request["query"].as_str().unwrap_or_default(),
                    request["category"].as_str(),
                )
            }
            "connector_handoff" => {
                json!({ "ok": true, "path": format!("/482139075/native/connector_handoff?key={}&access={}&token=synthetic-demo", request["key"].as_str().unwrap_or_default(), request["access"].as_str().unwrap_or("read")) })
            }
            "computer_remove" => {
                let mut state = self.state.lock().expect("demo lock");
                let id = request["id"].as_u64().unwrap_or(0).to_string();
                state.workspace["services"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|s| !(s["kind"] == "computer" && s["key"] == id));
                state.workspace.clone()
            }
            "service_pause" => {
                let mut state = self.state.lock().expect("demo lock");
                let Some(service) = state.workspace["services"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|s| s["key"] == request["key"] && s["kind"] == request["kind"])
                else {
                    return json!({ "ok": false, "error": "No such service" });
                };
                let paused = request["paused"].as_bool().unwrap_or(false);
                service["paused"] = json!(paused);
                service["live"] = json!(!paused);
                service["label"] = json!(if paused { "Paused" } else { "Connected" });
                state.workspace.clone()
            }
            "instructions" => {
                let mut state = self.state.lock().expect("demo lock");
                let mut instructions = state.instructions.clone();
                if let Some(project) = request["project"].as_u64() {
                    let Some(instruction) = project_pages::instruction(&mut state, project) else {
                        return json!({"ok": false, "code": "not_found", "error": "No such project"});
                    };
                    instructions.push(instruction.clone());
                }
                let project = request["project"].as_u64().and_then(|id| state.project_catalog.iter().find(|p| p["id"] == id)).map(|p| json!({"id":p["id"], "number":p["id"], "name":p["name"], "archived":p["archived"]}));
                instructions.sort_by_key(|i| match i["layer"].as_str() {
                    Some("organization") => 0,
                    Some("project") => 1,
                    _ => 2,
                });
                json!({ "ok": true, "context":{"account":state.notification_preferences["account"],"email":"alex@example.com","project":project}, "instructions": instructions, "memory": state.memory })
            }
            "memory_preference" | "memory_forget" => {
                let mut state = self.state.lock().expect("demo lock");
                if cmd == "memory_preference" {
                    state.memory["enabled"] = request["enabled"].clone();
                } else {
                    state.memory["entries"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|entry| entry["id"] != request["id"]);
                }
                let mut memory = state.memory.clone();
                memory["ok"] = json!(true);
                memory
            }
            "instruction_count" => {
                json!({ "ok": true, "tokens": request["text"].as_str().unwrap_or_default().split_whitespace().count() })
            }
            "instruction_update" => {
                let mut state = self.state.lock().expect("demo lock");
                let instruction = if request["layer"] == "project" {
                    project_pages::instruction(&mut state, request["project"].as_u64().unwrap_or(0))
                } else {
                    state
                        .instructions
                        .iter_mut()
                        .find(|i| i["layer"] == request["layer"])
                };
                let Some(instruction) = instruction else {
                    return json!({ "ok": false, "error": "No such instructions" });
                };
                if instruction["editable"] != true {
                    return json!({ "ok": false, "error": "You cannot edit these instructions." });
                }
                let restoring = request["change"]["kind"] == "restore";
                let body = if restoring {
                    instruction["revisions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["id"] == request["change"]["revision_id"])
                        .map(|r| r["body"].clone())
                } else {
                    Some(request["change"]["body"].clone())
                };
                let Some(body) = body else {
                    return json!({ "ok": false, "error": "That revision is no longer available." });
                };
                let tokens = body.as_str().unwrap_or_default().split_whitespace().count();
                if tokens > 3000 {
                    return json!({ "ok": false, "error": "Please keep it under 3,000 tokens." });
                }
                instruction["body"] = body.clone();
                instruction["tokens"] = json!(tokens);
                let id = instruction["revisions"].as_array().unwrap().len() + 1;
                instruction["revisions"].as_array_mut().unwrap().insert(0, json!({ "id": id, "body": body, "tokens": tokens, "author": "You", "created_at": ago(0), "age": "less than a minute ago", "changes": "+1 line" }));
                let instruction = instruction.clone();
                for summary in state.workspace["instructions"].as_array_mut().unwrap() {
                    if summary["layer"] == request["layer"] {
                        summary["written"] = json!(!body.as_str().unwrap_or_default().is_empty());
                    }
                }
                json!({ "ok": true, "instruction": instruction, "notice": if restoring { "Brought back that version of your instructions." } else { "Saved your instructions." } })
            }
            "chats" => {
                let state = self.state.lock().expect("demo lock");
                let list: Vec<Value> = state.chats.iter().map(|c| c.summary.clone()).collect();
                let mut answer = chats::list_header(state.credits_left);
                if state.status["account"].is_object() {
                    answer["account"] = state.status["account"].clone();
                }
                answer["copy_answers_as"] = state.copy_answers_as.clone();
                answer["user"]["name"] = state.profile["display_name"].clone();
                answer["ok"] = json!(true);
                answer["chats"] = json!(list);
                answer["projects"] = json!(state.projects);
                answer["pins"] = state.pins.clone();
                if state.web_search {
                    answer["web_search"] = json!({
                        "allowed": true,
                        "rate": "1.4 credits per search",
                        "note": "Web searches go to Google for Gemini models and to Bing for GPT models, outside the EU.",
                    });
                }
                answer
            }
            "chat" => {
                let state = self.state.lock().expect("demo lock");
                match state
                    .chats
                    .iter()
                    .find(|c| Some(c.summary["number"].as_u64().unwrap_or(0)) == number)
                {
                    Some(chat) => json!({
                        "ok": true,
                        "chat": chat.summary,
                        "locked_reason": null,
                        "entries": chat.entries,
                        "approvals": chat.approvals,
                        "questions": chat.questions,
                        "queued": chat.queued,
                    }),
                    None => json!({ "ok": false, "code": "not_found", "error": "No such chat" }),
                }
            }
            "templates" | "template_upload" => {
                templates::respond(&mut self.state.lock().expect("demo lock"), cmd, request)
            }
            "people" => people::respond(
                &mut self.state.lock().expect("demo lock"),
                &request["request"],
            ),
            "schedules" => schedules::respond(
                &mut self.state.lock().expect("demo lock"),
                &request["request"],
            ),
            "account_settings" => {
                let mut state = self.state.lock().expect("demo lock");
                let request: cww::account_settings::Request =
                    serde_json::from_value(request["request"].clone()).unwrap();
                if let cww::account_settings::Request::Update { account, change } = request {
                    if account
                        != state.notification_preferences["account"]["id"]
                            .as_str()
                            .unwrap()
                    {
                        return json!({"ok":false,"code":"stale_account","error":"Your organization changed."});
                    }
                    match change {
                        cww::account_settings::Change::Profile {
                            full_name,
                            display_name,
                        } => {
                            state.profile =
                                json!({"full_name":full_name,"display_name":display_name});
                        }
                        cww::account_settings::Change::CopyFormat { copy_answers_as } => {
                            state.copy_answers_as = json!(copy_answers_as);
                        }
                    }
                }
                json!({"ok":true,"account":state.notification_preferences["account"],"email":"alex@example.com","profile":state.profile,"copy_answers_as":state.copy_answers_as})
            }
            "models" | "model_preferences" | "model_preferences_update" => {
                let mut state = self.state.lock().expect("demo lock");
                let mut models = chats::models();
                if cmd == "model_preferences_update" {
                    if request["account"] != state.notification_preferences["account"]["id"] {
                        return json!({"ok":false,"code":"stale_account","error":"Your organization changed."});
                    }
                    let model = request["model_id"]
                        .as_str()
                        .and_then(|id| id.parse::<u64>().ok());
                    if let Some(id) = model
                        && !models["models"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|m| m["id"] == id)
                    {
                        return json!({"ok":false,"code":"invalid","error":"Please select an available model."});
                    }
                    state.preferred_model = json!(model.unwrap_or(12));
                }
                models["ok"] = json!(true);
                if state.web_search {
                    models["models"][0]["searches_web"] = json!(true);
                }
                models["default_model_id"] = if state.preferred_model == 15 {
                    json!(12)
                } else {
                    state.preferred_model.clone()
                };
                if cmd != "models" {
                    models["account"] = state.notification_preferences["account"].clone();
                    models["email"] = json!("alex@example.com");
                    models["preferred_model_id"] = state.preferred_model.clone();
                }
                models
            }
            "chat_download" => {
                json!({"ok": true, "data": "VmVuZG9yLFRvdGFsCkFjbWUsNDIwMDAK", "content_type": "text/csv"})
            }
            "chat_upload" => self.upload(request),
            "scan_destinations" => json!({
                "ok": true,
                "destinations": [
                    { "id": 41, "name": "Mail scanner", "bot": "Mail scanner" },
                    { "id": 42, "name": "Bills from the post", "bot": "Bills from the post" },
                ],
            }),
            "scan_send" => Self::scan_send(request),
            "chat_approve" | "chat_deny" | "chat_answer" | "chat_decline" => {
                let Some(number) = number else {
                    return json!({ "ok": false, "code": "bad_request", "error": "No chat" });
                };
                self.decide(cmd, number, request)
            }
            "chat_retry" | "chat_branch" | "chat_rename" | "chat_delete" | "chat_pin"
            | "chat_move" | "chat_share" | "chat_unshare" => {
                let Some(number) = number else {
                    return json!({ "ok": false, "code": "bad_request", "error": "No chat" });
                };
                self.act(cmd, number, request)
            }
            "chat_send" => {
                let text = request["text"].as_str().unwrap_or_default().to_string();
                if let Some(model) = request["model"].as_str() {
                    let models = chats::models();
                    let found = models["models"]
                        .as_array()
                        .and_then(|m| m.iter().find(|m| model_id(m) == model).cloned());
                    match found {
                        Some(m) if m["selectable"] == json!(false) => {
                            return json!({ "ok": false, "code": "model_unavailable", "error": m["reason"] });
                        }
                        None => {
                            return json!({ "ok": false, "code": "model_unavailable",
                                           "error": "That model isn't available. Pick another one." });
                        }
                        Some(_) => {}
                    }
                }
                let model = request["model"].as_str().and_then(|id| {
                    chats::models()["models"]
                        .as_array()
                        .and_then(|m| m.iter().find(|m| model_id(m) == id).cloned())
                        .map(|m| json!({ "id": m["id"], "name": m["name"], "logo": m["logo"] }))
                });
                let attachments: Vec<Value> = {
                    let state = self.state.lock().expect("demo lock");
                    request["attachments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|id| state.uploads.iter().find(|u| u["signed_id"] == *id))
                        .map(|u| json!({ "filename": u["filename"], "byte_size": u["byte_size"], "content_type": u["content_type"], "icon": u["icon"] }))
                        .collect()
                };
                let (number, summary) = {
                    let mut state = self.state.lock().expect("demo lock");
                    let id = state.next_id;
                    state.next_id += 10;
                    let number = match number {
                        Some(n) => n,
                        None => {
                            let n = state
                                .chats
                                .iter()
                                .filter_map(|c| c.summary["number"].as_u64())
                                .max()
                                .unwrap_or(0)
                                + 1;
                            let mut summary =
                                chats::summary(n, &chats::title_for(&text), "idle", ago(0));
                            // Started in a project: it's the project's.
                            if let Some(project) = state.projects.iter().find(|p| {
                                request["project"].is_u64() && p["id"] == request["project"]
                            }) {
                                summary["project"] =
                                    json!({ "id": project["id"], "name": project["name"] });
                                summary["can"]["share"] = json!(false);
                            }
                            state.chats.insert(0, chats::Chat::new(summary, Vec::new()));
                            n
                        }
                    };
                    let Some(chat) = state
                        .chats
                        .iter_mut()
                        .find(|c| c.summary["number"] == json!(number))
                    else {
                        return json!({ "ok": false, "code": "not_found", "error": "No such chat" });
                    };
                    if chat.summary["state"] == "processing" {
                        let Some(steer) = request["steer"].as_bool() else {
                            return json!({ "ok": false, "code": "chat_busy", "error": "An answer is being written in this chat." });
                        };
                        // The server checks files before they go, so they
                        // wait for the answer to finish.
                        if !attachments.is_empty() {
                            return json!({ "ok": false, "code": "attachment_refused",
                                           "error": "Files can go once this answer finishes, so they can be checked first. Send them then." });
                        }
                        let queued = json!({
                            "id": id, "content": text, "steer": steer, "queued_at": ago(0),
                            "unqueue_path": format!("/desktop/chats/{number}/queued_messages/{id}"),
                        });
                        chat.queued.push(queued.clone());
                        let summary = chat.summary.clone();
                        drop(state);
                        self.changed(number);
                        return json!({ "ok": true, "chat": summary, "queued": queued });
                    }
                    let mut entry = json!({ "kind": "user", "id": id, "content": text });
                    if !attachments.is_empty() {
                        entry["attachments"] = json!(attachments);
                    }
                    chat.entries.push(entry);
                    if let Some(model) = model {
                        chat.summary["model"] = model;
                    }
                    chat.approvals.clear();
                    chat.questions.clear();
                    chat.summary["state"] = json!("processing");
                    chat.summary["updated_at"] = json!(ago(0));
                    (number, chat.summary.clone())
                };
                self.changed(number);
                let held = self.state.lock().expect("demo lock").held;
                if !held {
                    let id = self.state.lock().expect("demo lock").next_id;
                    self.stream(number, chats::canned_stream(id + 1, id + 2));
                }
                json!({ "ok": true, "chat": summary })
            }
            "chat_unqueue" => {
                let queued = request["queued"]
                    .as_str()
                    .and_then(|q| q.parse::<u64>().ok());
                let taken = {
                    let mut state = self.state.lock().expect("demo lock");
                    state
                        .chats
                        .iter_mut()
                        .find(|c| Some(c.summary["number"].as_u64().unwrap_or(0)) == number)
                        .and_then(|chat| {
                            let at = chat.queued.iter().position(|q| {
                                q["id"].as_u64() == queued && !q["unqueue_path"].is_null()
                            })?;
                            Some(chat.queued.remove(at))
                        })
                        .is_some()
                };
                match number {
                    Some(number) if taken => {
                        self.changed(number);
                        json!({ "ok": true })
                    }
                    _ => {
                        json!({ "ok": false, "code": "not_found", "error": "That message isn't queued any more." })
                    }
                }
            }
            "chat_cancel" => {
                let Some(number) = number else {
                    return json!({ "ok": false, "code": "bad_request", "error": "No chat" });
                };
                let streaming = {
                    let mut state = self.state.lock().expect("demo lock");
                    state.cancelled.insert(number);
                    state.streaming.contains(&number)
                };
                if !streaming {
                    self.settle(number);
                }
                json!({ "ok": true })
            }
            _ => json!({ "ok": true, "granted": true, "requested": false }),
        }
    }

    /// `chat_upload`: what the server takes, refused with its sentences.
    /// A scan from the scan page. With `CWW_DEMO_SCAN_OUT=<file>`, the PDF
    /// is saved there, to look at what the page made.
    fn scan_send(request: &Value) -> Value {
        use base64::Engine as _;
        let pdf = base64::engine::general_purpose::STANDARD
            .decode(request["data"].as_str().unwrap_or_default())
            .unwrap_or_default();
        if !pdf.starts_with(b"%PDF-") {
            return json!({ "ok": false, "code": "invalid", "error": "A scan must be a PDF" });
        }
        if let Some(out) = std::env::var_os("CWW_DEMO_SCAN_OUT") {
            let _ = std::fs::write(out, &pdf);
        }
        if request["destination"]["kind"] == "chat" {
            json!({ "ok": true, "chat": { "number": 99, "title": "Scan", "url": "https://chatwithwork.com/482139075/chats/99" } })
        } else {
            json!({ "ok": true, "event": { "id": 7, "state": "received" } })
        }
    }

    fn upload(&self, request: &Value) -> Value {
        let name = request["filename"].as_str().unwrap_or_default().to_string();
        if name.is_empty() {
            return json!({ "ok": false, "code": "attachment_refused", "error": "Choose a file to attach" });
        }
        let lower = name.to_lowercase();
        if [".svg", ".svgz", ".swf"].iter().any(|e| lower.ends_with(e)) {
            return json!({ "ok": false, "code": "attachment_refused", "error": format!("{name} has a blocked file type") });
        }
        if lower.ends_with(".exe") {
            return json!({ "ok": false, "code": "attachment_refused", "error": format!("{name} is not a supported file type") });
        }
        let size = request["data"].as_str().map_or(0, |d| d.len() * 3 / 4);
        let mut state = self.state.lock().expect("demo lock");
        let n = state.uploads.len() + 1;
        let uploaded = json!({
            "signed_id": format!("demo-upload-{n}"),
            "filename": name,
            "byte_size": size,
            "content_type": request["content_type"].as_str().unwrap_or("application/octet-stream"),
            "icon": assets::file_icon(request["content_type"].as_str().unwrap_or_default()),
        });
        state.uploads.push(uploaded.clone());
        let mut answer = uploaded;
        answer["ok"] = json!(true);
        answer
    }

    /// Approve, deny, answer or decline what chat `number` waits for: the
    /// step settles and the answer carries on.
    fn decide(self: &Arc<Self>, cmd: &str, number: u64, request: &Value) -> Value {
        let tool_call = request["tool_call"]
            .as_str()
            .and_then(|t| t.parse::<u64>().ok());
        let reply = {
            let mut state = self.state.lock().expect("demo lock");
            let Some(chat) = state
                .chats
                .iter_mut()
                .find(|c| c.summary["number"] == json!(number))
            else {
                return json!({ "ok": false, "code": "not_found", "error": "No such chat" });
            };
            let approval = matches!(cmd, "chat_approve" | "chat_deny");
            let list = if approval {
                &mut chat.approvals
            } else {
                &mut chat.questions
            };
            let Some(at) = list.iter().position(|a| a["id"].as_u64() == tool_call) else {
                return json!({ "ok": false, "code": "not_found", "error": "Nothing waits for that here." });
            };
            if cmd == "chat_answer" {
                let question = &list[at];
                let input = &request["input"];
                for field in question["fields"].as_array().into_iter().flatten() {
                    let value = &input[field["name"].as_str().unwrap_or_default()];
                    let missing = value.is_null() || value == &json!("");
                    if field["required"] == json!(true) && missing {
                        let title = field["title"].as_str().unwrap_or_default();
                        return json!({ "ok": false, "code": "invalid", "error": format!("{title} is needed.") });
                    }
                }
            }
            list.remove(at);
            for entry in &mut chat.entries {
                for step in entry["steps"].as_array_mut().into_iter().flatten() {
                    if step["waiting"] == json!(true) {
                        step["waiting"] = json!(false);
                    }
                }
            }
            chat.summary["state"] = json!("processing");
            chat.summary["updated_at"] = json!(ago(0));
            match cmd {
                "chat_approve" => "Posted to #launch. The team can see it now.".to_string(),
                "chat_deny" => match request["reason"].as_str() {
                    Some(reason) => format!("I didn't post it. You said: {reason}"),
                    None => "I didn't post it.".to_string(),
                },
                "chat_decline" => "Understood, I'll leave it there.".to_string(),
                _ => "Thanks, carrying on with that.".to_string(),
            }
        };
        let summary = self.summary_of(number);
        self.changed(number);
        let id = {
            let mut state = self.state.lock().expect("demo lock");
            state.next_id += 10;
            state.next_id
        };
        if !self.state.lock().expect("demo lock").held {
            self.stream(
                number,
                vec![(
                    Duration::from_millis(600),
                    chats::Step::Finish { id, content: reply },
                )],
            );
        }
        json!({ "ok": true, "chat": summary })
    }

    fn summary_of(&self, number: u64) -> Value {
        let state = self.state.lock().expect("demo lock");
        state
            .chats
            .iter()
            .find(|c| c.summary["number"] == json!(number))
            .map(|c| c.summary.clone())
            .unwrap_or(Value::Null)
    }

    /// Retry, branch, rename, delete, share or stop sharing chat `number`.
    fn act(self: &Arc<Self>, cmd: &str, number: u64, request: &Value) -> Value {
        let message = request["message"]
            .as_str()
            .and_then(|m| m.parse::<u64>().ok());
        let mut state = self.state.lock().expect("demo lock");
        let Some(at) = state
            .chats
            .iter()
            .position(|c| c.summary["number"] == json!(number))
        else {
            return json!({ "ok": false, "code": "not_found", "error": "No such chat" });
        };
        let can = |what: &str| state.chats[at].summary["can"][what] == json!(true);
        let forbidden = |what: &str| {
            json!({ "ok": false, "code": "forbidden",
            "error": format!("Only the person who started a chat can {what} it.") })
        };
        match cmd {
            "chat_retry" => {
                if !can("retry") {
                    return json!({ "ok": false, "code": "forbidden",
                        "error": "Only the person who asked can retry this. Ask again to use your own sources." });
                }
                let chat = &mut state.chats[at];
                if chat.summary["state"] == "processing" {
                    return json!({ "ok": false, "code": "chat_busy", "error": "Hold on, I'm still working on the current reply." });
                }
                // Everything after the question goes.
                let keep = match message
                    .and_then(|m| chat.entries.iter().position(|e| e["id"] == json!(m)))
                {
                    Some(i) if chat.entries[i]["kind"] == "user" => i + 1,
                    Some(i) => chat.entries[..i]
                        .iter()
                        .rposition(|e| e["kind"] == "user")
                        .map_or(0, |u| u + 1),
                    None => chat
                        .entries
                        .iter()
                        .rposition(|e| e["kind"] == "user")
                        .map_or(0, |u| u + 1),
                };
                chat.entries.truncate(keep);
                chat.approvals.clear();
                chat.questions.clear();
                chat.summary["state"] = json!("processing");
                let summary = chat.summary.clone();
                let held = state.held;
                state.next_id += 10;
                let id = state.next_id;
                drop(state);
                self.changed(number);
                if !held {
                    self.stream(number, chats::canned_stream(id + 1, id + 2));
                }
                json!({ "ok": true, "chat": summary })
            }
            "chat_branch" => {
                let chat = &state.chats[at];
                let upto = message
                    .and_then(|m| chat.entries.iter().position(|e| e["id"] == json!(m)))
                    .map_or(chat.entries.len(), |i| i + 1);
                let entries = chat.entries[..upto].to_vec();
                let title = format!(
                    "Branch of {}",
                    chat.summary["title"].as_str().unwrap_or_default()
                );
                let n = state
                    .chats
                    .iter()
                    .filter_map(|c| c.summary["number"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1;
                let summary = chats::summary(n, &title, "idle", ago(0));
                state
                    .chats
                    .insert(0, chats::Chat::new(summary.clone(), entries));
                drop(state);
                json!({ "ok": true, "chat": summary })
            }
            "chat_rename" => {
                if !can("rename") {
                    return forbidden("change");
                }
                let title = request["title"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if title.is_empty() {
                    return json!({ "ok": false, "code": "invalid", "error": "Give the chat a title" });
                }
                state.chats[at].summary["title"] = json!(title);
                let summary = state.chats[at].summary.clone();
                drop(state);
                self.changed(number);
                json!({ "ok": true, "chat": summary })
            }
            "chat_pin" => {
                if !can("pin") {
                    return forbidden("pin");
                }
                let pins = state.pins["chats"].as_array_mut().unwrap();
                pins.retain(|pin| *pin != number);
                if request["pinned"] == true {
                    pins.push(json!(number));
                }
                json!({ "ok": true, "pins": state.pins })
            }
            "chat_move" => {
                if !can("move") {
                    return forbidden("move");
                }
                let project = match request["project"].as_u64() {
                    Some(id) => match state.projects.iter().find(|project| project["id"] == id) {
                        Some(project) => project.clone(),
                        None => {
                            return json!({ "ok": false, "code": "not_found", "error": "No such project" });
                        }
                    },
                    None => Value::Null,
                };
                state.chats[at].summary["can"]["share"] = json!(project.is_null());
                state.chats[at].summary["project"] = project;
                state.chats[at].summary["share"] = Value::Null;
                json!({ "ok": true, "chat": state.chats[at].summary })
            }
            "chat_delete" => {
                if !can("delete") {
                    return forbidden("change");
                }
                state.chats.remove(at);
                json!({ "ok": true })
            }
            "chat_share" => {
                if !can("share") {
                    return forbidden("share");
                }
                let link = json!({ "url": chats::SHARE_URL, "expires_at": chats::expires() });
                state.chats[at].summary["share"] = link.clone();
                json!({ "ok": true, "url": link["url"], "expires_at": link["expires_at"], "chat": state.chats[at].summary })
            }
            _ => {
                state.chats[at].summary["share"] = Value::Null;
                json!({ "ok": true, "chat": state.chats[at].summary })
            }
        }
    }

    /// Tell followers that `chat` changed.
    fn changed(&self, chat: u64) {
        self.chat_event(chat, json!({ "type": "changed" }));
    }

    fn chat_event(&self, chat: u64, update: Value) {
        self.change(|s| {
            s.chat_events
                .push(json!({ "event": "chat", "chat": chat.to_string(), "update": update }));
        });
    }

    /// Stop a chat's answer where it is.
    fn settle(&self, chat: u64) {
        self.change(|s| {
            if let Some(c) = s
                .chats
                .iter_mut()
                .find(|c| c.summary["number"] == json!(chat))
            {
                c.summary["state"] = json!("idle");
                for entry in &mut c.entries {
                    if entry["kind"] == "activity" {
                        entry["pending"] = json!(false);
                    }
                }
            }
            s.cancelled.remove(&chat);
        });
        self.changed(chat);
        self.sync_list();
    }

    /// Tell the list's subscribers about every chat that started, changed
    /// or went since they last heard, as the server's list channel does.
    fn sync_list(&self) {
        self.change(|s| {
            let mut events = Vec::new();
            for chat in &s.chats {
                let number = &chat.summary["number"];
                let before = s.listed.iter().find(|c| &c["number"] == number);
                if before != Some(&chat.summary) {
                    events.push(json!({ "event": "chat", "chat": chat.summary }));
                }
            }
            for gone in s.listed.iter().filter(|c| {
                !s.chats
                    .iter()
                    .any(|chat| chat.summary["number"] == c["number"])
            }) {
                events.push(json!({ "event": "removed", "number": gone["number"] }));
            }
            s.listed = s.chats.iter().map(|c| c.summary.clone()).collect();
            s.list_events.extend(events);
        });
    }

    /// A `chats` subscription: "watching", then every change to the list
    /// until the client goes away, as the daemon relays the server's.
    fn follow_list(self: &Arc<Self>, stream: &mut impl Write) {
        if self.state.lock().expect("demo lock").old_daemon {
            let _ = send(
                stream,
                &json!({ "ok": false, "error": "bad request: unknown variant `chats`, expected `status` or `audit`" }),
            );
            return;
        }
        if send(stream, &json!({ "ok": true, "topics": ["chats"] })).is_err() {
            return;
        }
        let mut seen = self.state.lock().expect("demo lock").list_events.len();
        let watching = json!({ "event": "chats", "update": { "event": "watching" } });
        if send(stream, &watching).is_err() {
            return;
        }
        loop {
            let events = {
                let mut state = self.state.lock().expect("demo lock");
                while state.list_events.len() == seen {
                    state = self.changed.wait(state).expect("demo lock");
                }
                let events = state.list_events[seen..].to_vec();
                seen = state.list_events.len();
                events
            };
            for update in events {
                if send(stream, &json!({ "event": "chats", "update": update })).is_err() {
                    return;
                }
            }
        }
    }

    /// Play `steps` into `chat` on a thread of their own.
    fn stream(self: &Arc<Self>, chat: u64, steps: Vec<(Duration, chats::Step)>) {
        let me = Arc::clone(self);
        if !me.state.lock().expect("demo lock").streaming.insert(chat) {
            return;
        }
        std::thread::spawn(move || {
            for (pause, step) in steps {
                std::thread::sleep(pause);
                let cancelled = me
                    .state
                    .lock()
                    .expect("demo lock")
                    .cancelled
                    .contains(&chat);
                if cancelled {
                    break;
                }
                me.apply(chat, &step);
            }
            me.change(|s| {
                s.streaming.remove(&chat);
            });
            if me
                .state
                .lock()
                .expect("demo lock")
                .cancelled
                .contains(&chat)
            {
                me.settle(chat);
            }
        });
    }

    /// One step of a streamed answer, as the server would write it.
    fn apply(&self, chat: u64, step: &chats::Step) {
        use chats::Step;
        match step {
            Step::Progress(text) => {
                self.chat_event(chat, json!({ "type": "progress", "text": text }))
            }
            Step::Chunk { id, text } => {
                self.change(|s| {
                    if let Some(c) = s
                        .chats
                        .iter_mut()
                        .find(|c| c.summary["number"] == json!(chat))
                        && !c.entries.iter().any(|e| e["id"] == json!(id))
                    {
                        c.entries.push(
                            json!({ "kind": "assistant", "id": id, "content": "", "sources": [] }),
                        );
                    }
                });
                self.chat_event(
                    chat,
                    json!({ "type": "chunk", "message_id": id, "text": text }),
                );
            }
            Step::Activity {
                id,
                pending,
                progress,
            } => {
                self.change(|s| {
                    let Some(c) = s.chats.iter_mut().find(|c| c.summary["number"] == json!(chat)) else {
                        return;
                    };
                    match c.entries.iter_mut().find(|e| e["id"] == json!(id)) {
                        Some(entry) => {
                            entry["pending"] = json!(pending);
                            if !pending {
                                let title = entry["title"].as_str().unwrap_or_default().replace("Searching", "Searched");
                                entry["title"] = json!(title);
                                entry["progress"] = Value::Null;
                                if let Some(steps) = entry["steps"].as_array_mut() {
                                    for step in steps.iter_mut() {
                                        step["pending"] = json!(false);
                                        let summary = step["summary"].as_str().unwrap_or_default().replace("Reading", "Read").replace('…', "");
                                        step["summary"] = json!(summary);
                                    }
                                }
                                let reads = entry["steps"].as_array().map_or(0, |s| s.iter().filter(|s| s["summary"].as_str().is_some_and(|t| t.starts_with("Read"))).count());
                                let searches = entry["steps"].as_array().map_or(0, |s| s.len() - reads);
                                let mut details = format!("{searches} search{}", if searches == 1 { "" } else { "es" });
                                if reads > 0 {
                                    details.push_str(&format!(", read {reads} file{}", if reads == 1 { "" } else { "s" }));
                                }
                                entry["details"] = json!(details);
                            }
                        }
                        None => c.entries.push(json!({
                            "kind": "activity", "id": id,
                            "title": "Searching Drive and Slack", "details": "1 search",
                            "progress": progress, "services": ["Drive", "Slack"], "pending": pending,
                            "logos": [assets::logo("drive"), assets::logo("slack")],
                            "steps": [
                                { "summary": "Searched Drive for “budget owner”", "pending": false, "files": ["Budget 2026.xlsx"], "logo": assets::logo("drive") },
                                { "summary": "Reading Budget 2026.xlsx…", "pending": true, "files": [], "logo": assets::logo("drive") },
                            ],
                        })),
                    }
                });
                self.changed(chat);
            }
            Step::Finish { id, content } => {
                self.change(|s| {
                    if let Some(c) = s.chats.iter_mut().find(|c| c.summary["number"] == json!(chat)) {
                        c.summary["state"] = json!("idle");
                        c.summary["updated_at"] = json!(ago(0));
                        // The answer took the messages that steer it before
                        // it finished; the others wait for the next question.
                        let (steered, waiting): (Vec<Value>, Vec<Value>) =
                            std::mem::take(&mut c.queued)
                                .into_iter()
                                .partition(|q| q["steer"] == json!(true));
                        c.queued = waiting;
                        let at = c
                            .entries
                            .iter()
                            .position(|e| e["id"] == json!(id))
                            .unwrap_or(c.entries.len());
                        for (n, q) in steered.into_iter().enumerate() {
                            c.entries.insert(
                                at + n,
                                json!({ "kind": "user", "id": q["id"], "content": q["content"] }),
                            );
                        }
                        match c.entries.iter_mut().find(|e| e["id"] == json!(id)) {
                            Some(entry) => entry["content"] = json!(content),
                            None => c.entries.push(json!({ "kind": "assistant", "id": id, "content": content, "sources": [] })),
                        }
                    }
                });
                self.changed(chat);
                self.sync_list();
            }
        }
    }

    /// A `chat` subscription: "watching", then every update to the chat
    /// until the client goes away, as the daemon relays them.
    fn follow(self: &Arc<Self>, stream: &mut impl Write, chat: &str) {
        let Ok(number) = chat.parse::<u64>() else {
            let _ = send(
                stream,
                &json!({ "ok": false, "code": "bad_request", "error": "not a chat number" }),
            );
            return;
        };
        if send(stream, &json!({ "ok": true, "topics": ["chat"] })).is_err() {
            return;
        }
        let watching = json!({ "event": "chat", "chat": chat, "update": { "type": "watching" } });
        if send(stream, &watching).is_err() {
            return;
        }
        let (mut seen, held, catch_up) = {
            let state = self.state.lock().expect("demo lock");
            let processing = state.chats.iter().any(|c| {
                c.summary["number"] == json!(number) && c.summary["state"] == "processing"
            });
            (
                state.chat_events.len(),
                state.held,
                number == 12 && processing,
            )
        };
        // Chat 12 was being answered before it was opened: what's written
        // so far arrives first, and the rest streams unless held.
        if catch_up {
            let chunk = json!({ "event": "chat", "chat": chat,
                                "update": { "type": "chunk", "message_id": 123, "text": chats::stream_so_far() } });
            if send(stream, &chunk).is_err() {
                return;
            }
            if !held {
                self.stream(12, chats::rest_of_stream());
            }
        }
        loop {
            let events = {
                let mut state = self.state.lock().expect("demo lock");
                while state.chat_events.len() == seen {
                    state = self.changed.wait(state).expect("demo lock");
                }
                let events = state.chat_events[seen..].to_vec();
                seen = state.chat_events.len();
                events
            };
            for event in events.into_iter().filter(|e| e["chat"] == json!(chat)) {
                if send(stream, &event).is_err() {
                    return;
                }
            }
        }
    }
}

/// A model's ID as the control API passes it: a string.
fn model_id(model: &Value) -> String {
    match &model["id"] {
        Value::String(id) => id.clone(),
        other => other.to_string(),
    }
}

fn send(stream: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let mut out = serde_json::to_vec(value)?;
    out.push(b'\n');
    stream.write_all(&out)?;
    stream.flush()
}

/// Start a stand-in daemon in a fresh temporary directory; its paths point
/// the app at it.
pub fn start(scenario: Scenario) -> anyhow::Result<Server> {
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    let n = STARTED.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("cww-demo-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    start_at(scenario, Paths::under(&base))
}

fn start_at(scenario: Scenario, paths: Paths) -> anyhow::Result<Server> {
    std::fs::create_dir_all(&paths.daemon.config_dir)?;
    std::fs::create_dir_all(&paths.daemon.runtime_dir)?;
    if scenario == Scenario::Sample {
        // A paired sample has been through the welcome page.
        crate::state::AppState { onboarded: true }.save(&paths)?;
    }
    // A scan language chosen before, which this computer may not have
    // (`CWW_DEMO_SCAN_LANGUAGE=spa`), for the window's missing-language note.
    if let Some(language) = std::env::var_os("CWW_DEMO_SCAN_LANGUAGE") {
        crate::scan::Preferences {
            settings: Some(cww_scan::Settings {
                ocr: Some(language.to_string_lossy().into_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }
        .save(&paths)?;
    }
    let demo = Arc::new(Demo {
        state: Mutex::new(match scenario {
            Scenario::Sample => sample(),
            Scenario::Fresh => fresh(),
        }),
        changed: Condvar::new(),
    });
    // The daemon's own listener, so the endpoint, its permissions and, on
    // Windows, the pipe's owner are exactly what the app will meet.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let mut listener = {
        let _guard = runtime.enter();
        cww::control::bind(&paths.socket_path())?
    };
    let serving = Arc::clone(&demo);
    std::thread::spawn(move || {
        runtime.block_on(async move {
            loop {
                match listener.accept().await {
                    Ok(stream) => {
                        let demo = Arc::clone(&serving);
                        tokio::task::spawn_blocking(move || {
                            demo.serve(tokio_util::io::SyncIoBridge::new(stream));
                        });
                    }
                    Err(e) => log::warn!("demo: {e:#}"),
                }
            }
        });
    });
    Ok(Server { paths, demo })
}

#[cfg(test)]
pub fn start_organization(
    primary: &Paths,
    profile: &cww::auth::organizations::SavedOrganization,
) -> anyhow::Result<Server> {
    let server = start_at(
        Scenario::Sample,
        Paths {
            daemon: profile.runtime_paths(&primary.daemon)?,
            socket: None,
        },
    )?;
    server.demo.change(|state| {
        state.status["server"] = json!(profile.server.url);
        state.status["device_id"] = json!(profile.server.device_id);
        state.status["account"] = json!(profile.account);
        state.status["chat_only"] = json!(true);
        state.status["roots"] = json!([]);
        state.notification_preferences["account"] = json!(profile.account);
        state.held = true;
    });
    Ok(server)
}

/// Pairs instantly-ish and records what it was asked, in place of
/// `cww login --json`.
struct Account(Arc<Demo>);

impl crate::pairing::Account for Account {
    fn pair(
        &self,
        server: Option<String>,
        report: crate::pairing::Report,
    ) -> crate::pairing::Cancel {
        use crate::pairing::{Code, PairEvent};
        use std::sync::atomic::{AtomicBool, Ordering};

        let demo = Arc::clone(&self.0);
        demo.change(|s| s.requests.push(json!({ "cmd": "pair", "server": server })));
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            report(PairEvent::Code(Code {
                user_code: "WDJB-MJHT".into(),
                verification_uri: "https://chatwithwork.com/device".into(),
                browser_url: Some("https://chatwithwork.com/device?user_code=WDJB-MJHT".into()),
                device_name: "carmine-laptop".into(),
                fingerprint: "3sQ2kX9vY0a1Lr8TqZ4mN7pC6bW5eD2fH1gJ0kA9sE".into(),
            }));
            // Long enough for a test to cancel it first.
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(100));
                if stop.load(Ordering::SeqCst) {
                    return;
                }
            }
            demo.change(|s| {
                s.status["server"] = json!("https://chatwithwork.com");
                s.status["device_id"] = json!("42");
                s.status["paired"] = json!(true);
                s.status["connection"] = json!({ "connection": "connected", "since": ago(0) });
            });
            report(PairEvent::Paired { daemon_error: None });
        });
        let demo = Arc::clone(&self.0);
        Box::new(move || {
            cancelled.store(true, Ordering::SeqCst);
            demo.change(|s| s.requests.push(json!({ "cmd": "pair_cancel" })));
        })
    }

    fn logout(&self) -> Result<(), String> {
        self.0.change(|s| {
            s.requests.push(json!({ "cmd": "logout" }));
            s.status["server"] = Value::Null;
            s.status["device_id"] = Value::Null;
            s.status["paired"] = json!(false);
            s.status["connection"] = json!({ "connection": "not_paired", "since": ago(0) });
            Demo::event(s, "logged_out");
        });
        Ok(())
    }
}

#[cfg(test)]
pub fn people_page() -> Value {
    people::page()
}
