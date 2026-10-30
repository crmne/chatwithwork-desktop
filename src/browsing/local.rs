//! Local browser clients use the running daemon's control channel. This
//! module never starts a daemon, connects to Chrome, or changes settings.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::control::{Client, ControlRequest};

// Leave room for the tool name and control envelope in the 16 KiB frame.
const MAX_ARGUMENT_BYTES: u64 = 12 * 1024;

/// An object from a file or stdin, bounded before parsing. No arguments
/// means an empty object, useful for browser_tabs.
pub fn arguments(input: Option<&Path>) -> Result<Map<String, Value>> {
    match input {
        None => Ok(Map::new()),
        Some(path) if path == Path::new("-") => read_arguments(std::io::stdin().lock()),
        Some(path) => read_arguments(std::fs::File::open(path).context("cannot open JSON input")?),
    }
}

fn read_arguments(input: impl Read) -> Result<Map<String, Value>> {
    let mut bytes = Vec::new();
    input.take(MAX_ARGUMENT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ARGUMENT_BYTES {
        bail!("browser arguments exceed 12 KiB");
    }
    serde_json::from_slice(&bytes).context("browser arguments must be a JSON object")
}

pub fn tools(socket: &Path) -> Result<Value> {
    let response = client(socket)?.call(&ControlRequest::BrowserTools)?;
    Ok(serde_json::json!({ "tools": response["tools"] }))
}

pub fn call(
    socket: &Path,
    tool: String,
    arguments: Map<String, Value>,
    approved: bool,
) -> Result<Value> {
    let request = ControlRequest::BrowserCall {
        tool,
        arguments,
        approved,
    };
    if serde_json::to_vec(&request)?.len() >= 16 * 1024 {
        bail!("browser request exceeds 16 KiB");
    }
    // A download plus its mandatory preflight can exceed the ordinary
    // control client's 30s timeout. The bridge bounds each operation.
    let response = client(socket)?.call_slow(&request)?;
    if response["ok"] != true {
        bail!(
            "{}",
            response["error"].as_str().unwrap_or("browser call refused")
        );
    }
    Ok(response["result"].clone())
}

fn client(socket: &Path) -> Result<Client> {
    Client::connect(socket)?
        .context("Chat with Work Desktop is not running; start it before using browser tools")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_a_bounded_object_and_never_an_expression() {
        assert_eq!(
            read_arguments(br#"{"tab":1}"#.as_slice()).unwrap()["tab"],
            1
        );
        for input in ["[]", "null", "\"text\"", "process.exit()", "{\"x\":"] {
            assert!(read_arguments(input.as_bytes()).is_err());
        }
        let oversized = vec![b' '; MAX_ARGUMENT_BYTES as usize + 1];
        assert!(read_arguments(oversized.as_slice()).is_err());
        assert!(arguments(None).unwrap().is_empty());
    }
}
