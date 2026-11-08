//! `cww plan-tools <tools.json>`: the MCP server a thinking step's agent
//! gets Chat with Work's tools from (docs/own-plans.md). It speaks MCP over
//! stdio, one JSON-RPC message a line, lists exactly the tools in the file,
//! and never runs one: a `tools/call` gets no answer, since the agent is
//! stopped as soon as it asks, and the call goes to Chat with Work. It reads
//! nothing else, and writes only its answers.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The protocol version it answers with when the client names none.
const DEFAULT_VERSION: &str = "2025-06-18";

/// Serve the tools in `file` on stdin and stdout until stdin closes.
pub fn serve(file: &std::path::Path) -> anyhow::Result<()> {
    let tools: Value = serde_json::from_slice(&std::fs::read(file)?)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if let Some(reply) = answer(&tools, &line) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// The answer to one message, if it gets one.
pub fn answer(tools: &Value, line: &str) -> Option<Value> {
    let message: Value = serde_json::from_str(line).ok()?;
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"]
                .as_str()
                .unwrap_or(DEFAULT_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "cww", "version": env!("CARGO_PKG_VERSION") },
        }),
        "tools/list" => json!({ "tools": tools }),
        "ping" => json!({}),
        // Chat with Work runs it: the agent is stopped before it hears.
        "tools/call" => return None,
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" },
            }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}
