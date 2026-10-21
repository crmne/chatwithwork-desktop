//! The daemon's control channel (CONTROL.md): one JSON object per line
//! over a Unix socket only this user can open. The prototype speaks it
//! directly instead of linking the `cww` crate, to keep its build small;
//! a real port would use `cww::control::Client` and `cww::paths`, which
//! also cover Windows' named pipe and the long-path fallback.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

/// Where the daemon listens: `$CWW_HOME/run/cww.sock` (also the egui
/// app's `--demo` daemon), else the XDG runtime directory on Linux, else
/// `~/.local/state/cww/cww.sock` (macOS).
pub fn socket_path() -> PathBuf {
    if let Some(home) = std::env::var_os("CWW_HOME") {
        return PathBuf::from(home).join("run/cww.sock");
    }
    if cfg!(target_os = "linux")
        && let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        return PathBuf::from(runtime).join("cww/cww.sock");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".local/state/cww/cww.sock")
}

fn connect() -> Result<UnixStream, String> {
    UnixStream::connect(socket_path()).map_err(|e| format!("The Local Agent isn't running ({e})."))
}

fn send(stream: &mut UnixStream, request: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|e| e.to_string())
}

/// One request, one answer, on a connection of its own.
pub fn request(request: &Value) -> Result<Value, String> {
    let mut stream = connect()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    send(&mut stream, request)?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if answer["ok"] == Value::Bool(true) {
        Ok(answer)
    } else {
        Err(answer["error"].as_str().unwrap_or("The Local Agent refused that.").to_string())
    }
}

/// A subscription: events arrive on a thread of their own until it's
/// dropped, which hangs up and so ends it at the daemon too.
pub struct Subscription {
    stream: UnixStream,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

pub fn subscribe(
    request: &Value,
    mut on_event: impl FnMut(Value) + Send + 'static,
) -> Result<Subscription, String> {
    let mut stream = connect()?;
    send(&mut stream, request)?;
    let reader = stream.try_clone().map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(reader).lines();
    let first: Value = match lines.next() {
        Some(Ok(line)) => serde_json::from_str(&line).map_err(|e| e.to_string())?,
        _ => return Err("The Local Agent hung up.".into()),
    };
    if first["ok"] != Value::Bool(true) {
        return Err(first["error"].as_str().unwrap_or("refused").to_string());
    }
    std::thread::Builder::new()
        .name("cww-subscription".into())
        .spawn(move || {
            for line in lines.map_while(Result::ok) {
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    on_event(event);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Subscription { stream })
}
