//! The worker asks the supervisor to run commands.
//!
//! The confined worker can't start a sandbox itself (Landlock forbids the
//! mounts Bubblewrap needs, and nothing may execute in a shared folder), so
//! the supervisor, which isn't confined, keeps the [`Runs`] and the worker
//! asks it over a socket pair it inherits as a file descriptor, one JSON
//! line each way per call. The supervisor trusts nothing it's sent: it
//! reads the config from disk for every start, finds the folder by its ID,
//! and checks the working folder itself. It stops every run when the worker
//! ends, so a restart (new folders, a new sandbox) leaves nothing behind.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

pub use super::channel_call::{Call, answer};
use super::{RunError, Runs};
use crate::paths::Paths;

/// The worker's end, as a descriptor number in this variable.
pub const ENV: &str = "CWW_RUN_CHANNEL";
/// Longest line either side accepts.
const MAX_LINE: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

/// The supervisor's side for one worker: a socket pair, one end of which
/// the worker inherits.
pub struct Pair {
    pub supervisor: UnixStream,
    /// Inherited by the worker as a descriptor, without close-on-exec.
    pub worker: OwnedFd,
}

impl Pair {
    pub fn new() -> std::io::Result<Self> {
        let (supervisor, worker) = UnixStream::pair()?;
        let worker = OwnedFd::from(worker);
        // The worker inherits this end: no close-on-exec.
        rustix::io::fcntl_setfd(&worker, rustix::io::FdFlags::empty())?;
        Ok(Self { supervisor, worker })
    }

    pub fn worker_fd(&self) -> RawFd {
        self.worker.as_raw_fd()
    }
}

/// Serve the worker's calls on `stream` until it closes, then stop every
/// run it started. Blocking: the supervisor runs it on a thread of its own.
pub fn serve(stream: UnixStream, runs: Runs, paths: Paths) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let mut writer = writer;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match std::io::Read::take(&mut reader, MAX_LINE as u64).read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if !line.ends_with('\n') => break,
            Ok(_) => {}
        }
        let reply: Result<serde_json::Value, RunError> = match serde_json::from_str::<Call>(&line) {
            Ok(call) => answer(&runs, &paths, call),
            Err(_) => Err(RunError::new("invalid", "not a call")),
        };
        let Ok(mut out) = serde_json::to_vec(&reply) else {
            break;
        };
        out.push(b'\n');
        if writer.write_all(&out).is_err() {
            break;
        }
    }
    runs.stop_all();
}

/// The worker's side.
#[derive(Clone)]
pub struct Client {
    inner: std::sync::Arc<tokio::sync::Mutex<Connection>>,
}

struct Connection {
    reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

/// The inherited descriptor, taken before confinement, if this is a worker
/// with one.
pub fn inherited() -> Option<OwnedFd> {
    let fd: RawFd = std::env::var(ENV).ok()?.parse().ok()?;
    // SAFETY: the variable is ours (the supervisor set it), and it's read
    // once: the variable is removed so nothing takes the descriptor twice.
    unsafe { std::env::remove_var(ENV) };
    if fd < 3 || rustix::io::fcntl_getfd(unsafe { rustix::fd::BorrowedFd::borrow_raw(fd) }).is_err()
    {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let _ = rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC);
    Some(fd)
}

impl Client {
    /// Within a runtime.
    pub fn new(fd: OwnedFd) -> std::io::Result<Self> {
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(true)?;
        let stream = tokio::net::UnixStream::from_std(stream)?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(Connection {
                reader: tokio::io::BufReader::new(reader),
                writer,
            })),
        })
    }

    pub async fn call(&self, call: &Call) -> Result<serde_json::Value, RunError> {
        let lost = || {
            RunError::new(
                "unavailable",
                "lost touch with the process that runs commands",
            )
        };
        let mut connection = self.inner.lock().await;
        let exchange = async {
            let mut line = serde_json::to_vec(call).map_err(|_| lost())?;
            line.push(b'\n');
            connection
                .writer
                .write_all(&line)
                .await
                .map_err(|_| lost())?;
            let mut reply = String::new();
            let read = tokio::io::AsyncReadExt::take(&mut connection.reader, MAX_LINE as u64)
                .read_line(&mut reply)
                .await
                .map_err(|_| lost())?;
            if read == 0 || !reply.ends_with('\n') {
                return Err(lost());
            }
            serde_json::from_str::<Result<serde_json::Value, RunError>>(&reply)
                .map_err(|_| lost())?
        };
        tokio::time::timeout(TIMEOUT, exchange)
            .await
            .map_err(|_| lost())?
    }
}
