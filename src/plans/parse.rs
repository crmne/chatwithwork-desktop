//! Reading an agent's own event stream (`codex exec --json`, Claude Code's
//! `stream-json`) into one step's answer: its text, the calls of Chat with
//! Work's tools it asked for, why it stopped, and what it used.
//!
//! Each agent's parser is behind [`Events`], so a step can just as well
//! send the events back unparsed (`think`'s `"output": "raw"`) without
//! touching anything else. The lines read are what the run printed, its
//! tokens already taken out.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::coding::Agent;

/// How long Codex is given, after it starts a call of Chat with Work's
/// tools, to start the others the model asked for with it: a model's
/// parallel calls start within milliseconds of each other, and its next
/// request takes far longer than this.
pub const CODEX_GRACE: Duration = Duration::from_millis(1500);

/// Longest error an agent's own words give, kept as it said it.
const MAX_ERROR: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    ToolUse,
    EndTurn,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: String,
    /// Chat with Work's own name for the tool, without the agent's prefix.
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Every input token, those read from the cache included.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The input tokens read from the provider's cache.
    pub cached_tokens: u64,
}

/// One step's answer so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop: Option<Stop>,
    pub usage: Option<Usage>,
    pub model: Option<String>,
    /// The agent's own error, when it ended with one.
    pub error: Option<String>,
}

impl Step {
    /// `think`'s result.
    pub fn result(&self) -> Value {
        let stop = self.stop.unwrap_or(if self.tool_calls.is_empty() {
            Stop::EndTurn
        } else {
            Stop::ToolUse
        });
        json!({
            "text": self.text,
            "tool_calls": self.tool_calls,
            "stop": stop,
            "usage": self.usage,
            "model": self.model,
        })
    }

    fn push_text(&mut self, text: &str, new_block: bool) {
        if new_block && !self.text.is_empty() && !text.is_empty() {
            self.text.push_str("\n\n");
        }
        self.text.push_str(text);
    }
}

/// An agent's event stream, line by line.
pub trait Events: Send {
    /// One line the agent printed on its standard output.
    fn line(&mut self, line: &str, now: Instant);
    /// The answer so far.
    fn step(&self) -> &Step;
    /// Whether the step has its answer, and the agent should stop.
    fn done(&self, now: Instant) -> bool;
}

/// The parser for `agent`'s events, for the step `id` (which Codex's call
/// IDs, its items', are made unique with). `model` is the one asked for.
pub fn parser(agent: Agent, id: &str, model: Option<String>) -> Box<dyn Events> {
    match agent {
        Agent::Codex => Box::new(Codex {
            id: id.to_string(),
            step: Step {
                model,
                ..Step::default()
            },
            ..Codex::default()
        }),
        _ => Box::new(Claude::default()),
    }
}

/// `codex exec --json`: `item.completed` with an `agent_message` carries
/// text, whole; `item.started` with an `mcp_tool_call` of server `cww`
/// carries a call and its arguments; `turn.completed` the usage.
#[derive(Default)]
struct Codex {
    id: String,
    step: Step,
    seen: Vec<String>,
    /// When to stop after the latest call started.
    stop_at: Option<Instant>,
    ended: bool,
}

impl Events for Codex {
    fn line(&mut self, line: &str, now: Instant) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event["type"].as_str().unwrap_or_default() {
            "item.started" | "item.completed" | "item.updated" => {
                let item = &event["item"];
                match item["type"].as_str().unwrap_or_default() {
                    "agent_message" if event["type"] == "item.completed" => {
                        let text = item["text"].as_str().unwrap_or_default();
                        self.step.push_text(text, true);
                    }
                    "mcp_tool_call" if item["server"] == super::SERVER => {
                        let item_id = item["id"].as_str().unwrap_or_default().to_string();
                        if self.seen.contains(&item_id) {
                            return;
                        }
                        let Some(name) = item["tool"].as_str() else {
                            return;
                        };
                        self.seen.push(item_id.clone());
                        self.step.tool_calls.push(ToolCall {
                            id: format!("{}-{item_id}", self.id),
                            name: name.to_string(),
                            arguments: object(&item["arguments"]),
                        });
                        self.step.stop = Some(Stop::ToolUse);
                        self.stop_at = Some(now + CODEX_GRACE);
                    }
                    _ => {}
                }
            }
            "turn.completed" => {
                let usage = &event["usage"];
                self.step.usage = Some(Usage {
                    input_tokens: number(&usage["input_tokens"]),
                    output_tokens: number(&usage["output_tokens"]),
                    cached_tokens: number(&usage["cached_input_tokens"]),
                });
                if self.step.stop.is_none() {
                    self.step.stop = Some(Stop::EndTurn);
                }
                self.ended = true;
            }
            "turn.failed" => {
                self.step.error = Some(cut(event["error"]["message"].as_str().unwrap_or("")));
                self.ended = true;
            }
            "error" => {
                self.step.error = Some(cut(event["message"].as_str().unwrap_or("")));
            }
            _ => {}
        }
    }

    fn step(&self) -> &Step {
        &self.step
    }

    fn done(&self, now: Instant) -> bool {
        self.ended || self.stop_at.is_some_and(|at| now >= at)
    }
}

/// Claude Code's `stream-json` with partial messages: `stream_event`s carry
/// the text as it comes (`text_delta`) and how a message ends
/// (`message_delta`, `message_stop`); each `assistant` event a whole block,
/// its tool calls with their arguments; `result` the end and the usage.
#[derive(Default)]
struct Claude {
    step: Step,
    /// Whether text came as deltas in the message being read.
    deltas: bool,
    /// A text block started, and its first text is still to come.
    new_block: bool,
    /// How the message being read ends, and what it used.
    stop_reason: Option<String>,
    message_usage: Option<Usage>,
    /// Every message's usage before it, summed.
    used: Usage,
    ended: bool,
}

impl Events for Claude {
    fn line(&mut self, line: &str, _now: Instant) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        // Only the step's own conversation, never a subagent's.
        if !event["parent_tool_use_id"].is_null() {
            return;
        }
        match event["type"].as_str().unwrap_or_default() {
            "system" if event["subtype"] == "init" => {
                if let Some(model) = event["model"].as_str() {
                    self.step.model = Some(model.to_string());
                }
            }
            "stream_event" => self.stream_event(&event["event"]),
            "assistant" => {
                let message = &event["message"];
                if let Some(model) = message["model"].as_str() {
                    self.step.model = Some(model.to_string());
                }
                for block in message["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str().unwrap_or_default() {
                        // Text that came without deltas.
                        "text" if !self.deltas => {
                            self.step
                                .push_text(block["text"].as_str().unwrap_or_default(), true);
                        }
                        "tool_use" => {
                            let Some(name) = block["name"]
                                .as_str()
                                .and_then(|n| n.strip_prefix(CLAUDE_PREFIX))
                            else {
                                continue;
                            };
                            let id = block["id"].as_str().unwrap_or_default().to_string();
                            if self.step.tool_calls.iter().any(|c| c.id == id) {
                                continue;
                            }
                            self.step.tool_calls.push(ToolCall {
                                id,
                                name: name.to_string(),
                                arguments: object(&block["input"]),
                            });
                        }
                        _ => {}
                    }
                }
            }
            "result" => {
                let usage = &event["usage"];
                if usage.is_object() {
                    self.step.usage = Some(claude_usage(usage));
                }
                if event["is_error"] == Value::Bool(true) {
                    let said = event["result"]
                        .as_str()
                        .or_else(|| event["subtype"].as_str())
                        .unwrap_or("");
                    self.step.error = Some(cut(said));
                }
                if self.step.stop.is_none() {
                    self.step.stop = Some(Stop::EndTurn);
                }
                self.ended = true;
            }
            _ => {}
        }
    }

    fn step(&self) -> &Step {
        &self.step
    }

    fn done(&self, _now: Instant) -> bool {
        self.ended
    }
}

/// What Claude Code calls an MCP server's tools: `mcp__<server>__<tool>`.
pub const CLAUDE_PREFIX: &str = "mcp__cww__";

impl Claude {
    fn stream_event(&mut self, event: &Value) {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                self.deltas = false;
                self.stop_reason = None;
                self.message_usage = None;
                if let Some(model) = event["message"]["model"].as_str() {
                    self.step.model = Some(model.to_string());
                }
            }
            "content_block_start" if event["content_block"]["type"] == "text" => {
                self.new_block = true;
            }
            "content_block_delta" if event["delta"]["type"] == "text_delta" => {
                self.deltas = true;
                let text = event["delta"]["text"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    self.step
                        .push_text(text, std::mem::take(&mut self.new_block));
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                if event["usage"].is_object() {
                    self.message_usage = Some(claude_usage(&event["usage"]));
                }
            }
            "message_stop" => {
                if let Some(usage) = self.message_usage.take() {
                    self.used.input_tokens += usage.input_tokens;
                    self.used.output_tokens += usage.output_tokens;
                    self.used.cached_tokens += usage.cached_tokens;
                    self.step.usage = Some(self.used);
                }
                // A message that asked for Chat with Work's tools: the step
                // has its answer, whatever Claude Code would do next.
                if self.stop_reason.as_deref() == Some("tool_use")
                    && !self.step.tool_calls.is_empty()
                {
                    self.step.stop = Some(Stop::ToolUse);
                    self.ended = true;
                }
            }
            _ => {}
        }
    }
}

fn claude_usage(usage: &Value) -> Usage {
    let cached = number(&usage["cache_read_input_tokens"]);
    Usage {
        input_tokens: number(&usage["input_tokens"])
            + number(&usage["cache_creation_input_tokens"])
            + cached,
        output_tokens: number(&usage["output_tokens"]),
        cached_tokens: cached,
    }
}

fn number(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

/// A call's arguments: an object, `{}` when the agent gave none.
fn object(value: &Value) -> Value {
    if value.is_object() {
        value.clone()
    } else {
        json!({})
    }
}

fn cut(text: &str) -> String {
    text.trim().chars().take(MAX_ERROR).collect()
}
