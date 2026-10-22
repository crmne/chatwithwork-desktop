//! The way back from the system browser (RFC 8252's loopback redirect).
//!
//! Google refuses to sign in inside a web view, so signing in with a
//! provider and connecting a service run in the default browser. When the
//! page starts one, it asks for a callback: a one-shot listener on
//! `127.0.0.1` at a random port, under a random path. The server sends the
//! browser there at the end with a one-time token (or a status), and the
//! listener hands the query to the page, which redeems the token with the
//! PKCE verifier only it holds. A token read by anything else is useless.
//!
//! Only the latest callback listens: starting another stops the one before.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// How long a callback waits for the browser.
pub const LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long one connection may take to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the listener checks whether it has been replaced or expired.
const POLL: Duration = Duration::from_millis(200);
/// Longest request line read; the server's tokens and messages are short.
const MAX_LINE: usize = 8 * 1024;

/// The page the browser shows once it has come back.
const DONE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Chat with Work</title><style>:root{color-scheme:light dark}\
body{margin:0;min-height:100vh;display:grid;place-items:center;\
font:16px/1.5 system-ui,sans-serif;background:light-dark(#fafafa,#0b0b0c);color:light-dark(#18181b,#ededed)}\
main{max-width:28rem;padding:2rem;text-align:center}h1{font-size:1.25rem;margin:0 0 .5rem}\
p{margin:0;color:light-dark(#52525b,#a1a1aa)}</style></head><body><main>\
<h1>You can close this tab</h1><p>Return to Chat with Work to carry on.</p></main></body></html>";

/// Callbacks started so far; a listener stops when a newer one starts.
#[derive(Default, Clone)]
pub struct Callbacks {
    generation: Arc<AtomicU64>,
}

pub struct Started {
    /// `http://127.0.0.1:<port>/<nonce>`, for the server to send the
    /// browser back to.
    pub callback: String,
}

impl Callbacks {
    /// Starts listening, and calls `deliver` once with the callback's URL and
    /// its query when the browser comes back.
    pub fn start(
        &self,
        deliver: impl FnOnce(String, BTreeMap<String, String>) + Send + 'static,
    ) -> std::io::Result<Started> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let nonce = nonce();
        let callback = format!("http://127.0.0.1:{port}/{nonce}");
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current = self.generation.clone();
        let url = callback.clone();
        std::thread::Builder::new()
            .name("cww-desktop-callback".into())
            .spawn(move || {
                let deadline = Instant::now() + LIFETIME;
                let still_current = || current.load(Ordering::SeqCst) == generation;
                if let Some(query) = listen(&listener, &nonce, deadline, still_current) {
                    deliver(url, query);
                }
            })?;
        Ok(Started { callback })
    }
}

/// 32 characters of URL-safe base64 from 24 random bytes.
fn nonce() -> String {
    let mut bytes = [0u8; 24];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .expect("the system has randomness");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Accepts connections until one asks for `/<nonce>`, the deadline passes
/// or `still_current` turns false. Other requests (a favicon, a guess) get a
/// 404 and the listener carries on.
fn listen(
    listener: &TcpListener,
    nonce: &str,
    deadline: Instant,
    still_current: impl Fn() -> bool,
) -> Option<BTreeMap<String, String>> {
    while Instant::now() < deadline && still_current() {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(query) = answer(stream, nonce) {
                    return Some(query);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(_) => std::thread::sleep(POLL),
        }
    }
    None
}

/// Reads one request and answers it; the query when it's the callback.
fn answer(mut stream: TcpStream, nonce: &str) -> Option<BTreeMap<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let mut line = String::new();
    BufReader::new(Read::take(&stream, MAX_LINE as u64))
        .read_line(&mut line)
        .ok()?;
    let query = callback_query(&line, nonce);
    let response = match &query {
        Some(_) => format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\n\
             referrer-policy: no-referrer\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{DONE_PAGE}",
            DONE_PAGE.len()
        ),
        None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    query
}

/// The query of `GET /<nonce>?... HTTP/1.1`, as string pairs (the last of a
/// repeated name wins), or `None` for any other request.
pub fn callback_query(request_line: &str, nonce: &str) -> Option<BTreeMap<String, String>> {
    let mut parts = request_line.trim_end().split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if method != "GET" || !version.starts_with("HTTP/1.") || parts.next().is_some() {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path.strip_prefix('/')? != nonce {
        return None;
    }
    Some(
        url::form_urlencoded::parse(query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn nonces_are_32_url_safe_characters_and_differ() {
        let a = nonce();
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(a, nonce());
    }

    #[test]
    fn only_a_get_of_the_exact_path_is_the_callback() {
        let q = callback_query("GET /abc?token=T%2B1&x=1 HTTP/1.1\r\n", "abc").unwrap();
        assert_eq!(q["token"], "T+1");
        assert_eq!(q["x"], "1");
        assert!(
            callback_query("GET /abc HTTP/1.1", "abc")
                .unwrap()
                .is_empty()
        );
        for other in [
            "GET /abcd?token=T HTTP/1.1",
            "GET /ab?token=T HTTP/1.1",
            "GET /favicon.ico HTTP/1.1",
            "POST /abc?token=T HTTP/1.1",
            "GET /abc?token=T",
            "GET /abc?token=T HTTP/1.1 extra",
            "GET abc?token=T HTTP/1.1",
            "",
        ] {
            assert!(callback_query(other, "abc").is_none(), "{other:?}");
        }
    }

    fn get(callback: &str, path_and_query: &str) -> String {
        let address = callback
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET {path_and_query} HTTP/1.1\r\nHost: {address}\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn the_browser_comes_back_once_and_the_page_gets_the_query() {
        let callbacks = Callbacks::default();
        let (tx, rx) = mpsc::channel();
        let started = callbacks
            .start(move |url, query| tx.send((url, query)).unwrap())
            .unwrap();
        let path = started.callback.splitn(4, '/').nth(3).unwrap().to_string();
        assert!(started.callback.starts_with("http://127.0.0.1:"));

        let missed = get(&started.callback, "/favicon.ico");
        assert!(missed.starts_with("HTTP/1.1 404"));
        let shown = get(
            &started.callback,
            &format!("/{path}?status=connected&return_to=%2Fx"),
        );
        assert!(shown.starts_with("HTTP/1.1 200"));
        assert!(shown.contains("You can close this tab"));

        let (url, query) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(url, started.callback);
        assert_eq!(query["status"], "connected");
        assert_eq!(query["return_to"], "/x");
    }

    #[test]
    fn a_newer_callback_stops_the_older_one() {
        let callbacks = Callbacks::default();
        let (tx, rx) = mpsc::channel::<()>();
        let first = callbacks.start(move |_, _| tx.send(()).unwrap()).unwrap();
        let _second = callbacks.start(|_, _| {}).unwrap();
        std::thread::sleep(POLL * 3);
        let address = first
            .callback
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        // The first listener is gone: nothing answers, and nothing is delivered.
        assert!(TcpStream::connect(address).is_err() || rx.recv_timeout(POLL * 3).is_err());
    }
}
