//! The command tools: `run` in a shared folder that allows commands, and
//! `run_status`, `run_output` and `run_stop` for a command left running.
//!
//! `run` is destructive (a command can change anything in its folder) and
//! open-world (with the folder's network setting on, it can reach the
//! network), so Chat with Work asks the person every time. A dry run says
//! what would run, where, with or without network and with which limits,
//! for the approval card, without running anything.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Finished, Limits, MAX_COMMAND_CHARS, Output, Request, RunError, Runner, Status};
use crate::error::{ErrorCode, ToolError};
use crate::reader::safe_fs::ToolPath;

/// Offered only while a shared folder allows commands.
pub const COMMAND_TOOLS: [&str; 4] = ["run", "run_status", "run_output", "run_stop"];

/// How long `run` waits for a command before answering that it's still
/// running: under the 20 seconds Chat with Work waits for a tool call.
pub const FOREGROUND_WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(250);
/// Characters of output each progress report carries at most.
const PROGRESS_CHARS: usize = 200;

/// A folder that allows commands, as the tools see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRoot {
    pub id: String,
    pub label: String,
    pub network: bool,
}

/// What the command tools need: where commands run, and which folders
/// allow them.
#[derive(Clone)]
pub struct CommandTools {
    pub runner: Runner,
    pub roots: Vec<CommandRoot>,
    pub limits: Limits,
}

/// Reports a running command's output as it comes: how many bytes so far,
/// and its latest line.
pub type Progress = std::sync::Arc<dyn Fn(u64, String) + Send + Sync>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    command: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandleArgs {
    run: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputArgs {
    run: String,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    max_chars: Option<usize>,
}

/// A `run` result: where and what, and how it stands or ended.
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub path: String,
    pub folder: String,
    pub command: String,
    pub network: bool,
    #[serde(flatten)]
    pub status: Status,
    /// The end of what it printed, while it's running too.
    pub stdout: String,
    pub stderr: String,
    pub stdout_cut: usize,
    pub stderr_cut: usize,
    /// Says what to do next with a command still running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CommandTools {
    pub fn offered(&self) -> bool {
        !self.roots.is_empty() && !matches!(self.runner, Runner::Off(_))
    }

    fn root(&self, id: &str) -> Result<&CommandRoot, ToolError> {
        self.roots.iter().find(|r| r.id == id).ok_or_else(|| {
            ToolError::new(
                ErrorCode::NotWritable,
                format!("{id} doesn't allow commands (see `allows_commands` in `roots`)"),
            )
        })
    }

    /// Run one of the command tools. `progress` hears a foreground run's
    /// output as it comes. Returns the result and, for the audit log, the
    /// command and how it ended.
    pub async fn call(
        &self,
        name: &str,
        args: Value,
        progress: Option<Progress>,
    ) -> Result<(Value, Option<String>), ToolError> {
        match name {
            "run" => {
                let args: RunArgs = parse(args)?;
                let path = args.path.as_deref().unwrap_or_default();
                let path = if path.is_empty() {
                    return Err(ToolError::invalid_argument(
                        "`path` names the folder to run in: `<root_id>:` or `<root_id>:dir`",
                    ));
                } else {
                    ToolPath::parse(path)?
                };
                let root = self.root(&path.root_id)?.clone();
                let command = args.command.trim().to_string();
                if command.is_empty() || command.chars().count() > MAX_COMMAND_CHARS {
                    return Err(ToolError::invalid_argument(format!(
                        "`command` is one line of shell, at most {MAX_COMMAND_CHARS} characters"
                    )));
                }
                let shown = ToolPath::display(&root.id, &path.rel);
                if args.dry_run {
                    let preview = json!({
                        "dry_run": true,
                        "path": shown,
                        "folder": root.label,
                        "dir": path.rel.as_string(),
                        "command": command,
                        "network": root.network,
                        "background": args.background,
                        "limits": self.limits,
                    });
                    return Ok((preview, Some(command)));
                }
                let request = Request {
                    root: root.id.clone(),
                    dir: path.rel.as_string(),
                    command: command.clone(),
                };
                let status = self.runner.start(request).await.map_err(tool_error)?;
                let wait = if args.background {
                    Duration::ZERO
                } else {
                    FOREGROUND_WAIT
                };
                let finished = self.follow(&status.id, wait, progress).await?;
                let note = finished.status.running().then(|| {
                    format!(
                        "Still running as {}. Check it with `run_status`, read what it prints with \
                         `run_output`, and stop it with `run_stop`.",
                        finished.status.id
                    )
                });
                let detail = ended(&finished.status, &command);
                let result = RunResult {
                    path: shown,
                    folder: root.label,
                    command,
                    network: root.network,
                    status: finished.status,
                    stdout: finished.stdout,
                    stderr: finished.stderr,
                    stdout_cut: finished.stdout_cut,
                    stderr_cut: finished.stderr_cut,
                    note,
                };
                Ok((
                    serde_json::to_value(result).unwrap_or(Value::Null),
                    Some(detail),
                ))
            }
            "run_status" => {
                let args: HandleArgs = parse(args)?;
                handle(&args.run)?;
                let finished = self.runner.finished(&args.run).await.map_err(tool_error)?;
                Ok((serde_json::to_value(finished).unwrap_or(Value::Null), None))
            }
            "run_output" => {
                let args: OutputArgs = parse(args)?;
                handle(&args.run)?;
                let max = args.max_chars.unwrap_or(16_000).clamp(1, 32_000);
                let output = self
                    .runner
                    .output(&args.run, args.offset, max)
                    .await
                    .map_err(tool_error)?;
                Ok((serde_json::to_value(output).unwrap_or(Value::Null), None))
            }
            "run_stop" => {
                let args: HandleArgs = parse(args)?;
                handle(&args.run)?;
                self.runner.stop(&args.run).await.map_err(tool_error)?;
                // It stops within a moment; say how it ended once it has.
                let finished = self.follow(&args.run, Duration::from_secs(5), None).await?;
                let detail = ended(&finished.status, "");
                Ok((
                    serde_json::to_value(finished).unwrap_or(Value::Null),
                    Some(detail),
                ))
            }
            other => Err(ToolError::invalid_argument(format!(
                "{other:?} is not a command tool"
            ))),
        }
    }

    /// Wait up to `wait` for a run to end, reporting its output as it
    /// comes, then say how it stands.
    async fn follow(
        &self,
        run: &str,
        wait: Duration,
        progress: Option<Progress>,
    ) -> Result<Finished, ToolError> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut offset = 0;
        loop {
            let status = self.runner.status(run).await.map_err(tool_error)?;
            if let Some(progress) = &progress {
                let output: Output = self
                    .runner
                    .output(run, offset, 64 * 1024)
                    .await
                    .map_err(tool_error)?;
                if output.next_offset > offset {
                    offset = output.next_offset;
                    progress(offset, last_line(&output.text));
                }
            }
            if !status.running() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        self.runner.finished(run).await.map_err(tool_error)
    }
}

/// How a run stands, for the audit log: "exit 0 · make test".
fn ended(status: &Status, command: &str) -> String {
    let state = match (status.state.as_str(), status.exit_code) {
        ("exited", Some(code)) => format!("exit {code}"),
        (state, _) => state.replace('_', " "),
    };
    let command: String = command.chars().take(400).collect();
    if command.is_empty() {
        state
    } else {
        format!("{state} · {command}")
    }
}

/// The last line with something on it, at most a progress report long.
fn last_line(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let count = line.chars().count();
    if count > PROGRESS_CHARS {
        line.chars().skip(count - PROGRESS_CHARS).collect()
    } else {
        line.to_string()
    }
}

fn handle(run: &str) -> Result<(), ToolError> {
    if super::valid_id(run) {
        Ok(())
    } else {
        Err(ToolError::invalid_argument(
            "`run` is the handle `run` returned",
        ))
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args)
        .map_err(|e| ToolError::invalid_argument(format!("invalid arguments: {e}")))
}

fn tool_error(err: RunError) -> ToolError {
    let code = match err.code.as_str() {
        "not_allowed" => ErrorCode::NotWritable,
        "invalid_path" => ErrorCode::InvalidPath,
        "not_found" => ErrorCode::NotFound,
        "invalid" => ErrorCode::InvalidArgument,
        "busy" => ErrorCode::RateLimited,
        _ => ErrorCode::Internal,
    };
    ToolError::new(code, err.message)
}

/// The tools' definitions: `(name, title, description, schema, kind)`,
/// where kind is `run`, `read` or `stop`.
pub fn definitions() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Value,
    &'static str,
)> {
    vec![
        (
            "run",
            "Run a command on the computer",
            "Run one line of shell (`sh -c`) in a shared folder that allows commands \
             (`allows_commands: true` in `roots`), such as `bin/rails test` or `npm run build`. \
             It runs in a sandbox of its own: it can change only that folder, reads the system \
             and the installed language toolchains, has no network unless the folder allows it \
             (`network` in `roots`), and has CPU, memory and time limits. `path` is the folder \
             to start in, `<root_id>:` or `<root_id>:dir`. Waits up to 15 seconds and returns \
             the exit code and the end of its output; a longer command keeps running, and the \
             result says how to follow it. `background: true` returns at once. The person \
             approves every command.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "maxLength": MAX_COMMAND_CHARS, "description": "One line of shell." },
                    "path": { "type": "string", "description": "`<root_id>:` or `<root_id>:dir`, the folder it starts in." },
                    "background": { "type": "boolean", "description": "Return at once and leave it running (a server, a watcher, a long build)." },
                    "dry_run": { "type": "boolean", "description": "Check the command and say where and how it would run, without running it." }
                },
                "required": ["command", "path"],
                "additionalProperties": false
            }),
            "run",
        ),
        (
            "run_status",
            "Check a command",
            "How a command `run` started stands: running, or how it ended, with the end of \
             its output.",
            handle_schema(),
            "read",
        ),
        (
            "run_output",
            "Read a command's output",
            "Read what a command `run` started printed, both streams as they came, from \
             `offset` (0 for the start). Pass `next_offset` back for more; `done` once it has \
             ended and nothing more will come.",
            json!({
                "type": "object",
                "properties": {
                    "run": { "type": "string", "description": "The handle `run` returned." },
                    "offset": { "type": "integer", "minimum": 0, "description": "Byte offset (default 0)." },
                    "max_chars": { "type": "integer", "minimum": 1, "maximum": 32000, "description": "Most characters to return (default 16000)." }
                },
                "required": ["run"],
                "additionalProperties": false
            }),
            "read",
        ),
        (
            "run_stop",
            "Stop a command",
            "Stop a command `run` started, and everything it started.",
            handle_schema(),
            "stop",
        ),
    ]
}

fn handle_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "run": { "type": "string", "description": "The handle `run` returned." } },
        "required": ["run"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_a_run_ended_without_the_whole_output() {
        let status = |state: &str, exit_code| Status {
            id: "0".into(),
            state: state.into(),
            exit_code,
            duration_ms: 1,
            output_bytes: 0,
            error: None,
        };
        assert_eq!(ended(&status("exited", Some(0)), "make"), "exit 0 · make");
        assert_eq!(
            ended(&status("timed_out", None), "make"),
            "timed out · make"
        );
        assert_eq!(last_line("one\ntwo\n\n"), "two");
        assert_eq!(last_line(&"x".repeat(500)).len(), PROGRESS_CHARS);
    }

    #[tokio::test]
    async fn a_dry_run_says_where_and_how_without_running() {
        let tools = CommandTools {
            runner: Runner::Off("not here".into()),
            roots: vec![CommandRoot {
                id: "app".into(),
                label: "App".into(),
                network: false,
            }],
            limits: Limits::from_config(&crate::config::Limits::default()),
        };
        let (preview, detail) = tools
            .call(
                "run",
                json!({ "command": "bin/rails test", "path": "app:api", "dry_run": true }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(preview["folder"], "App");
        assert_eq!(preview["dir"], "api");
        assert_eq!(preview["network"], false);
        assert_eq!(preview["limits"]["timeout_seconds"], 1800);
        assert_eq!(detail.as_deref(), Some("bin/rails test"));
        let err = tools
            .call("run", json!({ "command": "ls", "path": "docs:" }), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotWritable);
        let err = tools
            .call("run", json!({ "command": "ls", "path": "/etc" }), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidPath);
        let err = tools
            .call("run_stop", json!({ "run": "../x" }), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        let err = tools
            .call("run", json!({ "command": "ls", "path": "app:" }), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "the runner is off");
    }
}
