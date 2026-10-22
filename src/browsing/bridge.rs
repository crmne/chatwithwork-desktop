//! The daemon's end of the browser extension: the native messaging host
//! (`cww` started by the browser, see [`super::host`]) attaches over the
//! control socket with `browser_attach`, and that connection then carries
//! JSON lines both ways:
//!
//! - to the extension: requests `{"id", "method", "params"}`, and the
//!   policy as a notification `{"method": "policy", "params": {"sites",
//!   "stopped"}}`, sent on attach and whenever it changes;
//! - from the extension: answers `{"id", "result"}` or `{"id", "error":
//!   {"code", "message"}}`, and events (`hello`, `state`).
//!
//! One browser at a time: a second one is refused while the first is
//! attached.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::error::{ErrorCode, ToolError};

/// Longest line the extension may send: a screenshot is at most about
/// 180 KB, a page of text 32 000 characters.
const MAX_LINE: usize = 4 * 1024 * 1024;

/// How long a call waits for the extension, under the server's 20 seconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(18);

/// What the extension enforces, from `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Policy {
    pub sites: Vec<String>,
    pub stopped: bool,
}

/// The browser that is attached, as it said in its `hello`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Extension {
    pub origin: String,
    pub version: String,
    pub browser: String,
    /// The person pressed Stop in the extension.
    pub stopped: bool,
    pub since: String,
}

type Reply = Result<Value, ToolError>;

struct Session {
    id: u64,
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    extension: Extension,
}

pub struct Bridge {
    session: Mutex<Option<Session>>,
    next_session: AtomicU64,
    next_request: AtomicU64,
    policy: watch::Sender<Policy>,
    /// Extension origins allowed to attach: `chrome-extension://<id>/`.
    origins: Mutex<Vec<String>>,
    /// Bots the browser is lent to, by ID.
    lent: Mutex<Vec<String>>,
    /// Called when the attached browser changes, for status.
    changed: Box<dyn Fn() + Send + Sync>,
}

/// The published extension's ID (the Chrome Web Store keeps it from the
/// key the first version was uploaded with).
pub const EXTENSION_ID: &str = "ehjfihpogoacjodjnanclodinjnnpmcp";

pub fn origin(id: &str) -> String {
    format!("chrome-extension://{id}/")
}

/// The origins allowed to attach: ours, and any `extension_ids` the config
/// adds.
pub fn origins(extra: &[String]) -> Vec<String> {
    std::iter::once(EXTENSION_ID.to_string())
        .chain(extra.iter().cloned())
        .filter(|id| id.len() == 32 && id.chars().all(|c| ('a'..='p').contains(&c)))
        .map(|id| origin(&id))
        .collect()
}

impl Bridge {
    pub fn new(
        policy: Policy,
        origins: Vec<String>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            session: Mutex::new(None),
            next_session: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
            policy: watch::Sender::new(policy),
            origins: Mutex::new(origins),
            lent: Mutex::default(),
            changed: Box::new(changed),
        }
    }

    /// New rules from `config.toml`; the extension hears them at once.
    pub fn configure(&self, policy: Policy, origins: Vec<String>, lent: Vec<String>) {
        *self.origins.lock().unwrap() = origins;
        *self.lent.lock().unwrap() = lent;
        self.policy.send_if_modified(|current| {
            let changed = *current != policy;
            *current = policy;
            changed
        });
    }

    pub fn lent_to(&self, bot: &str) -> bool {
        self.lent.lock().unwrap().iter().any(|id| id == bot)
    }

    pub fn policy(&self) -> Policy {
        self.policy.borrow().clone()
    }

    /// The attached browser, if any.
    pub fn extension(&self) -> Option<Extension> {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.extension.clone())
    }

    /// Ask the extension to run `method`. Errors are tool errors the model
    /// can read.
    pub async fn call(&self, method: &str, params: Value) -> Reply {
        let (id, receiver) = {
            let session = self.session.lock().unwrap();
            let Some(session) = session.as_ref() else {
                return Err(ToolError::new(
                    ErrorCode::BrowserUnavailable,
                    "the browser with the Chat with Work extension isn't running on this computer, \
                     or the extension isn't installed",
                ));
            };
            if session.extension.stopped {
                return Err(stopped());
            }
            let id = self.next_request.fetch_add(1, Ordering::Relaxed);
            let (sender, receiver) = oneshot::channel();
            session.pending.lock().unwrap().insert(id, sender);
            let request = json!({ "id": id, "method": method, "params": params });
            if session.outgoing.send(request).is_err() {
                session.pending.lock().unwrap().remove(&id);
                return Err(gone());
            }
            (id, receiver)
        };
        match tokio::time::timeout(CALL_TIMEOUT, receiver).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(gone()),
            Err(_) => {
                if let Some(session) = self.session.lock().unwrap().as_ref() {
                    session.pending.lock().unwrap().remove(&id);
                }
                Err(ToolError::new(
                    ErrorCode::PageError,
                    "the browser didn't answer in time; the page may still be loading",
                ))
            }
        }
    }

    /// Serve one attached extension until either side hangs up. `write`
    /// and `lines` are the control connection, after its `browser_attach`
    /// request.
    pub async fn serve<W, R>(
        &self,
        origin: String,
        write: &mut W,
        lines: &mut R,
        shutdown: CancellationToken,
    ) -> anyhow::Result<()>
    where
        W: AsyncWrite + Unpin,
        R: AsyncBufRead + Unpin,
    {
        if !self.origins.lock().unwrap().contains(&origin) {
            return write_line(
                write,
                &json!({
                    "ok": false,
                    "code": "not_allowed",
                    "error": "this extension isn't the Chat with Work extension this computer allows",
                }),
            )
            .await;
        }
        let (outgoing, mut requests) = mpsc::unbounded_channel();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>> = Arc::default();
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let busy = {
            let mut session = self.session.lock().unwrap();
            let busy = session
                .as_ref()
                .is_some_and(|current| !current.outgoing.is_closed());
            if !busy {
                *session = Some(Session {
                    id,
                    outgoing,
                    pending: Arc::clone(&pending),
                    extension: Extension {
                        origin: origin.clone(),
                        since: crate::audit::now(),
                        ..Extension::default()
                    },
                });
            }
            busy
        };
        if busy {
            return write_line(
                write,
                &json!({
                    "ok": false,
                    "code": "busy",
                    "error": "another browser is already connected to Chat with Work Desktop",
                }),
            )
            .await;
        }
        (self.changed)();
        let result = self
            .run(id, write, lines, &mut requests, &pending, shutdown)
            .await;
        {
            let mut session = self.session.lock().unwrap();
            if session.as_ref().is_some_and(|s| s.id == id) {
                *session = None;
            }
        }
        for (_, waiting) in pending.lock().unwrap().drain() {
            let _ = waiting.send(Err(gone()));
        }
        (self.changed)();
        result
    }

    async fn run<W, R>(
        &self,
        id: u64,
        write: &mut W,
        lines: &mut R,
        requests: &mut mpsc::UnboundedReceiver<Value>,
        pending: &Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
        shutdown: CancellationToken,
    ) -> anyhow::Result<()>
    where
        W: AsyncWrite + Unpin,
        R: AsyncBufRead + Unpin,
    {
        write_line(write, &json!({ "ok": true })).await?;
        let mut policy = self.policy.subscribe();
        let first = policy.borrow_and_update().clone();
        write_line(write, &json!({ "method": "policy", "params": first })).await?;
        let mut line = Vec::new();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return Ok(()),
                request = requests.recv() => match request {
                    Some(request) => write_line(write, &request).await?,
                    None => return Ok(()),
                },
                changed = policy.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    let now = policy.borrow_and_update().clone();
                    write_line(write, &json!({ "method": "policy", "params": now })).await?;
                }
                read = read_line(lines, &mut line) => {
                    if !read? {
                        return Ok(());
                    }
                    let message = serde_json::from_slice::<Value>(&line);
                    line.clear();
                    let Ok(message) = message else {
                        tracing::debug!("browser extension sent a line that isn't JSON");
                        continue;
                    };
                    self.receive(id, message, pending);
                }
            }
        }
    }

    fn receive(
        &self,
        id: u64,
        message: Value,
        pending: &Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    ) {
        if let Some(request) = message.get("id").and_then(Value::as_u64) {
            let Some(waiting) = pending.lock().unwrap().remove(&request) else {
                return;
            };
            let reply = match message.get("error") {
                Some(error) => Err(tool_error(error)),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = waiting.send(reply);
            return;
        }
        let event = message
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut guard = self.session.lock().unwrap();
        let Some(session) = guard.as_mut().filter(|s| s.id == id) else {
            return;
        };
        let text = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_str)
                .map(|s| {
                    s.chars()
                        .filter(|c| !c.is_control())
                        .take(200)
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        match event {
            "hello" => {
                session.extension.version = text("version");
                session.extension.browser = browser_name(&text("browser"));
                session.extension.stopped = message.get("stopped") == Some(&Value::Bool(true));
            }
            "state" => {
                session.extension.stopped = message.get("stopped") == Some(&Value::Bool(true));
            }
            _ => return,
        }
        drop(guard);
        (self.changed)();
    }
}

/// Read one line into `line`, without its newline. `false` at the end.
///
/// Cancel-safe, as a branch of `select!` must be: bytes read before it is
/// dropped stay in `line` (the caller clears it after a whole line) or in
/// the reader, never lost.
async fn read_line<R: AsyncBufRead + Unpin>(
    lines: &mut R,
    line: &mut Vec<u8>,
) -> anyhow::Result<bool> {
    loop {
        let available = lines.fill_buf().await?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        if let Some(end) = available.iter().position(|b| *b == b'\n') {
            line.extend_from_slice(&available[..end]);
            lines.consume(end + 1);
            return Ok(true);
        }
        let n = available.len();
        line.extend_from_slice(available);
        lines.consume(n);
        if line.len() > MAX_LINE {
            anyhow::bail!("the browser extension sent a message over {MAX_LINE} bytes");
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(write: &mut W, value: &Value) -> anyhow::Result<()> {
    let mut out = serde_json::to_vec(value)?;
    out.push(b'\n');
    write.write_all(&out).await?;
    write.flush().await?;
    Ok(())
}

pub fn stopped() -> ToolError {
    ToolError::new(
        ErrorCode::BrowserStopped,
        "the person stopped Chat with Work from using the browser; they can resume it on their computer",
    )
}

fn gone() -> ToolError {
    ToolError::new(
        ErrorCode::BrowserUnavailable,
        "the browser closed or disconnected from Chat with Work Desktop",
    )
}

/// The extension's error, as a tool error with one of our codes.
fn tool_error(error: &Value) -> ToolError {
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    let message: String = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the browser couldn't do that")
        .chars()
        .filter(|c| !c.is_control())
        .take(500)
        .collect();
    let code = match code {
        "invalid_argument" => ErrorCode::InvalidArgument,
        "too_large" => ErrorCode::TooLarge,
        "site_not_allowed" => ErrorCode::SiteNotAllowed,
        "not_in_group" => ErrorCode::NotInGroup,
        "no_tab" => ErrorCode::NoTab,
        "sensitive_field" => ErrorCode::SensitiveField,
        "would_submit" => ErrorCode::WouldSubmit,
        "browser_stopped" => ErrorCode::BrowserStopped,
        "not_found" => ErrorCode::NotFound,
        "internal" => ErrorCode::Internal,
        _ => ErrorCode::PageError,
    };
    ToolError::new(code, message)
}

/// "Chrome 141", "Edge 141", "Brave", ... from a user agent.
fn browser_name(agent: &str) -> String {
    let version = |marker: &str| {
        agent.split(marker).nth(1).map(|rest| {
            rest.split(['.', ' '])
                .next()
                .unwrap_or_default()
                .to_string()
        })
    };
    if let Some(v) = version("Edg/") {
        return format!("Edge {v}");
    }
    if let Some(v) = version("OPR/") {
        return format!("Opera {v}");
    }
    if let Some(v) = version("Chrome/") {
        return format!("Chrome {v}");
    }
    "a Chromium browser".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_line_read_across_a_cancelled_read_keeps_every_byte() {
        use tokio::io::AsyncWriteExt;
        let (mut extension, daemon) = tokio::io::duplex(64);
        let mut lines = tokio::io::BufReader::new(daemon);
        let mut line = Vec::new();
        extension.write_all(b"{\"id\":1,\"result\":").await.unwrap();
        // Another branch of select! wins while the line is half read.
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            read_line(&mut lines, &mut line),
        )
        .await;
        assert!(cancelled.is_err());
        extension.write_all(b"\"ok\"}\n").await.unwrap();
        assert!(read_line(&mut lines, &mut line).await.unwrap());
        assert_eq!(line, br#"{"id":1,"result":"ok"}"#);
    }

    #[test]
    fn names_the_browser() {
        assert_eq!(
            browser_name(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"
            ),
            "Chrome 141"
        );
        assert_eq!(
            browser_name(
                "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.3537.71"
            ),
            "Edge 141"
        );
    }

    #[test]
    fn allows_only_well_formed_extension_ids() {
        let all = origins(&["abcdefghijklmnopabcdefghijklmnop".into(), "bad".into()]);
        assert_eq!(
            all,
            vec![
                origin(EXTENSION_ID),
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/".to_string()
            ]
        );
    }

    #[test]
    fn maps_unknown_extension_codes_to_page_errors() {
        let error = tool_error(&json!({ "code": "weird", "message": "x\ny" }));
        assert_eq!(error.code, ErrorCode::PageError);
        assert_eq!(error.message, "xy");
        assert_eq!(
            tool_error(&json!({ "code": "site_not_allowed", "message": "no" })).code,
            ErrorCode::SiteNotAllowed
        );
    }
}
