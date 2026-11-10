//! `cww plan-codex <step folder> [model]`: drives `codex app-server` for
//! one thinking step (docs/own-plans.md), inside the step's sandbox. It says
//! who is calling (`clientInfo`), starts one ephemeral thread and one turn
//! with the step's conversation, and prints the notifications that matter
//! on its standard output, one JSON object a line, which `parse` reads.
//! It uses Codex's own built-in sign-in, never `chatgptAuthTokens`.
//!
//! Codex's home here is the step's own ([`super::CODEX_HOME_DIR`]): the
//! app-server has no `--ignore-user-config`, so a home of only the sign-in
//! is what keeps the person's settings and instructions out of a step.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

use super::{CODEX_HOME_DIR, PROMPT_FILE, codex_server_arguments};

/// What this client calls itself to OpenAI.
pub const CLIENT_NAME: &str = "chat_with_work_desktop";
const CLIENT_TITLE: &str = "Chat with Work Desktop";

const INITIALIZE: u64 = 1;
const THREAD_START: u64 = 2;
const TURN_START: u64 = 3;

/// The conversation: what to send, and what comes of one message.
pub struct Conversation {
    cwd: String,
    prompt: String,
    model: Option<String>,
}

/// What one of the app-server's messages leads to.
#[derive(Debug, Default, PartialEq)]
pub struct Reaction {
    /// Messages to send it.
    pub send: Vec<Value>,
    /// The line to print.
    pub print: Option<Value>,
    /// The step is over.
    pub done: bool,
}

/// An error as a line [`super::parse`] reads as one that ended the step.
fn failed(message: &str) -> Value {
    json!({ "method": "error", "params": { "error": { "message": message }, "willRetry": false } })
}

impl Conversation {
    pub fn new(cwd: String, prompt: String, model: Option<String>) -> Self {
        Self { cwd, prompt, model }
    }

    /// The first message: who is calling.
    pub fn first(&self) -> Value {
        json!({
            "id": INITIALIZE,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": CLIENT_NAME,
                    "title": CLIENT_TITLE,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {},
            },
        })
    }

    /// What `message` from the app-server leads to.
    pub fn react(&self, mut message: Value) -> Reaction {
        let mut reaction = Reaction::default();
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        match (id, message["method"].as_str().map(String::from)) {
            // It asks: nothing it could ask of a step has an answer here.
            (Some(id), Some(_)) => {
                reaction.send.push(json!({
                    "id": id,
                    "error": { "code": -32601, "message": "not available in a thinking step" },
                }));
            }
            // An answer to one of ours.
            (Some(id), None) => {
                if let Some(error) = message.get("error") {
                    reaction.print = Some(failed(error["message"].as_str().unwrap_or("")));
                    reaction.done = true;
                    return reaction;
                }
                match id.as_u64() {
                    Some(INITIALIZE) => {
                        reaction.send.push(json!({ "method": "initialized" }));
                        let mut params = json!({
                            "ephemeral": true,
                            "cwd": self.cwd,
                            "approvalPolicy": "never",
                            "sandbox": "read-only",
                        });
                        if let Some(model) = &self.model {
                            params["model"] = json!(model);
                        }
                        reaction.send.push(json!({
                            "id": THREAD_START,
                            "method": "thread/start",
                            "params": params,
                        }));
                    }
                    Some(THREAD_START) => {
                        reaction.send.push(json!({
                            "id": TURN_START,
                            "method": "turn/start",
                            "params": {
                                "threadId": message["result"]["thread"]["id"],
                                "input": [{ "type": "text", "text": self.prompt }],
                            },
                        }));
                    }
                    _ => {}
                }
            }
            (None, Some(method)) => {
                let keep = match method.as_str() {
                    "item/started" | "item/completed" => matches!(
                        message["params"]["item"]["type"].as_str(),
                        Some("agentMessage" | "mcpToolCall")
                    ),
                    "item/agentMessage/delta" | "thread/tokenUsage/updated" | "error" => true,
                    "turn/completed" => {
                        reaction.done = true;
                        // The turn's items were already sent, one by one.
                        message["params"]["turn"]["items"] = json!([]);
                        true
                    }
                    _ => false,
                };
                if keep {
                    reaction.print = Some(message);
                }
            }
            (None, None) => {}
        }
        reaction
    }
}

fn spawn(dir: &Path, shim: &Path) -> std::io::Result<Child> {
    let arguments = codex_server_arguments(dir, shim);
    #[cfg(windows)]
    let mut command = {
        // Codex is a `.cmd` of npm's here, which only cmd.exe finds and runs.
        use std::os::windows::process::CommandExt;
        let line: Vec<String> = arguments.iter().map(|w| super::cmd_quote(w)).collect();
        let mut command = Command::new("cmd.exe");
        command.raw_arg(format!("/d /s /c \"codex {}\"", line.join(" ")));
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("codex");
        command.args(&arguments);
        command
    };
    command
        .env("CODEX_HOME", dir.join(CODEX_HOME_DIR))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
}

/// Run the step in `dir` to the end of its turn.
pub fn serve(dir: &Path, model: Option<&str>) -> anyhow::Result<()> {
    let shim = std::env::current_exe()?;
    let prompt = std::fs::read_to_string(dir.join(PROMPT_FILE))?;
    let conversation = Conversation::new(
        dir.to_string_lossy().into_owned(),
        prompt,
        model.map(String::from),
    );
    let mut child = spawn(dir, &shim)?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let mut out = std::io::stdout().lock();
    writeln!(stdin, "{}", conversation.first())?;
    stdin.flush()?;
    for line in BufReader::new(stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line?) else {
            continue;
        };
        let reaction = conversation.react(message);
        if let Some(print) = reaction.print {
            writeln!(out, "{print}")?;
            out.flush()?;
        }
        for message in reaction.send {
            writeln!(stdin, "{message}")?;
            stdin.flush()?;
        }
        if reaction.done {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}
