//! The native messaging host: the browser starts `cww` with the
//! extension's origin as its only argument (`chrome-extension://<id>/`,
//! and `--parent-window=<n>` on Windows) when the extension connects, and
//! talks to it over stdin and stdout, each message a JSON object after its
//! length as a 32-bit integer in the platform's byte order.
//!
//! The host only relays. It attaches to the running daemon over the control
//! socket (`browser_attach`), passes the daemon's lines to the extension
//! and the extension's messages to the daemon, and tells the extension how
//! the daemon stands: `{"event": "daemon", "state": "connected" |
//! "not_running" | "refused", "message"?}`. It retries while the daemon
//! isn't running, and exits when the browser closes its stdin.

use std::io::{BufRead, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::control::{self, BrowserAttach};
use crate::paths::Paths;

/// Largest message the browser accepts from a host.
const TO_BROWSER_MAX: usize = 1024 * 1024;
/// Largest message we take from the extension (the browser allows 64 MiB).
const FROM_BROWSER_MAX: usize = 4 * 1024 * 1024;

/// The origin when the browser started this process as its native host.
pub fn invoked_as_host(args: &[std::ffi::OsString]) -> Option<String> {
    let first = args.get(1)?.to_str()?;
    (first.starts_with("chrome-extension://") && first.len() < 128).then(|| first.to_string())
}

/// Read one message from the browser. `None` when it closed stdin.
pub fn read_message(input: &mut impl Read) -> Result<Option<Value>> {
    let mut length = [0u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let length = u32::from_ne_bytes(length) as usize;
    if length > FROM_BROWSER_MAX {
        anyhow::bail!("the browser sent a message of {length} bytes");
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

/// Write one message to the browser. Messages over its 1 MB limit are
/// replaced by an error for the request they answer, if any.
pub fn write_message(output: &mut impl Write, message: &Value) -> Result<()> {
    let mut body = serde_json::to_vec(message)?;
    if body.len() > TO_BROWSER_MAX {
        body = serde_json::to_vec(&json!({
            "event": "dropped",
            "id": message.get("id"),
            "message": "a message for the browser was too large",
        }))?;
    }
    output.write_all(&(body.len() as u32).to_ne_bytes())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

/// Run as the native messaging host for `origin` until the browser closes
/// stdin.
pub fn run(paths: &Paths, origin: &str) -> Result<()> {
    let to_browser = Arc::new(Mutex::new(std::io::stdout()));
    let say = {
        let to_browser = Arc::clone(&to_browser);
        move |message: &Value| {
            let mut out = to_browser.lock().unwrap();
            if write_message(&mut *out, message).is_err() {
                // The browser is gone.
                std::process::exit(0);
            }
        }
    };

    // The extension's messages, read on a thread of their own.
    let (from_browser, messages) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            match read_message(&mut input) {
                Ok(Some(message)) => {
                    if from_browser.send(message).is_err() {
                        break;
                    }
                }
                // Closed, or a message we can't read: either way the
                // browser is done with us.
                Ok(None) | Err(_) => std::process::exit(0),
            }
        }
    });

    let socket = paths.socket_path();
    let mut last_state = String::new();
    let mut tell = |state: &str, message: Option<&str>| {
        let key = format!("{state}:{}", message.unwrap_or_default());
        if key != last_state {
            say(&json!({ "event": "daemon", "state": state, "message": message }));
            last_state = key;
        }
    };
    loop {
        let wait = match control::attach_browser(&socket, origin) {
            Ok(BrowserAttach::Attached { reader, mut writer }) => {
                tell("connected", None);
                relay(reader, &mut writer, &messages, &to_browser);
                tell("not_running", Some("Chat with Work Desktop stopped"));
                Duration::from_secs(2)
            }
            Ok(BrowserAttach::NotRunning) => {
                tell("not_running", None);
                Duration::from_secs(2)
            }
            Ok(BrowserAttach::Refused { message, .. }) => {
                tell("refused", Some(&message));
                Duration::from_secs(30)
            }
            Err(e) => {
                tell("not_running", Some(&format!("{e:#}")));
                Duration::from_secs(5)
            }
        };
        // Drop what the extension sends meanwhile; it only answers the
        // daemon's requests, which are gone.
        let until = std::time::Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
            match messages.recv_timeout(left) {
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
}

/// Pass lines both ways until the daemon hangs up.
fn relay(
    mut reader: impl BufRead + Send + 'static,
    writer: &mut impl Write,
    messages: &mpsc::Receiver<Value>,
    to_browser: &Arc<Mutex<std::io::Stdout>>,
) {
    let (closed_tx, closed) = mpsc::channel::<()>();
    let to_browser = Arc::clone(to_browser);
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let mut out = to_browser.lock().unwrap();
            if write_message(&mut *out, &message).is_err() {
                std::process::exit(0);
            }
        }
        let _ = closed_tx.send(());
    });
    loop {
        if closed.try_recv().is_ok() {
            return;
        }
        match messages.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => {
                let Ok(mut line) = serde_json::to_vec(&message) else {
                    continue;
                };
                line.push(b'\n');
                if writer
                    .write_all(&line)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::process::exit(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &json!({ "id": 1, "result": "é" })).unwrap();
        assert_eq!(&buffer[..4], &(buffer.len() as u32 - 4).to_ne_bytes());
        let read = read_message(&mut buffer.as_slice()).unwrap().unwrap();
        assert_eq!(read, json!({ "id": 1, "result": "é" }));
        assert!(read_message(&mut [].as_slice()).unwrap().is_none());
    }

    #[test]
    fn refuses_huge_messages_and_shrinks_big_answers() {
        let mut big = (FROM_BROWSER_MAX as u32 + 1).to_ne_bytes().to_vec();
        big.extend_from_slice(b"{}");
        assert!(read_message(&mut big.as_slice()).is_err());
        let mut buffer = Vec::new();
        write_message(
            &mut buffer,
            &json!({ "id": 7, "x": "a".repeat(TO_BROWSER_MAX) }),
        )
        .unwrap();
        let read = read_message(&mut buffer.as_slice()).unwrap().unwrap();
        assert_eq!(read["event"], "dropped");
        assert_eq!(read["id"], 7);
    }

    #[test]
    fn knows_when_the_browser_started_it() {
        let args = |list: &[&str]| {
            list.iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            invoked_as_host(&args(&[
                "cww",
                "chrome-extension://abc/",
                "--parent-window=0"
            ]))
            .as_deref(),
            Some("chrome-extension://abc/")
        );
        assert_eq!(invoked_as_host(&args(&["cww", "status"])), None);
        assert_eq!(invoked_as_host(&args(&["cww"])), None);
    }
}
