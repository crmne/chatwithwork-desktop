//! Commands run in a shared folder (`run`).
//!
//! The confined worker can't execute anything in a shared folder, by design
//! (`sandbox.rs`). So it asks the unconfined process above it (the
//! supervisor, see [`channel`]) to start each command in a sandbox of its
//! own, built for that one run with MXC: Bubblewrap on Linux (Landlock where
//! Bubblewrap can't create its namespaces), Seatbelt on macOS. The run reads the system and the language toolchains, writes only
//! its folder and a home of its own, has no network unless the folder
//! allows it, gets a clean environment, and has CPU time, memory and
//! wall-clock limits. Whoever starts it checks the folder again from the
//! config on disk: the worker names a folder by its ID and a relative path,
//! never an absolute one.
//!
//! Output is kept as it arrives: a window of the latest bytes for
//! [`Runs::output`] (what a running command printed so far), and the end of
//! each stream for the result.

#[cfg(unix)]
pub mod channel;
mod channel_call;
pub mod follow;
mod plan;
#[cfg(unix)]
mod run;
#[cfg(all(test, unix))]
mod tests;
pub mod tools;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use plan::{Plan, Request, Sandbox, plan};

use crate::config::Config;
use crate::paths::Paths;

/// Longest command line accepted, in characters.
pub const MAX_COMMAND_CHARS: usize = 8_000;
/// Most bytes one [`Runs::output`] call returns.
pub const MAX_OUTPUT_CHUNK: usize = 64 * 1024;
/// Bytes of a run's latest output kept for [`Runs::output`].
const WINDOW: usize = 1024 * 1024;
/// Finished runs kept for their status and output.
const FINISHED_KEPT: usize = 32;

/// Why a run can't start, or a handle isn't known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RunError {
    /// `not_allowed`, `invalid_path`, `not_found`, `invalid`, `busy`,
    /// `unavailable`.
    pub code: String,
    pub message: String,
}

impl RunError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// The limits one run gets, from the config's `[limits]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub cpu_seconds: u64,
    pub memory_mb: u64,
    pub timeout_seconds: u64,
    /// Processes at once, itself included.
    #[serde(default)]
    pub processes: u32,
    pub output_chars: usize,
}

impl Limits {
    pub fn from_config(limits: &crate::config::Limits) -> Self {
        Self {
            cpu_seconds: limits.command_cpu_seconds,
            memory_mb: limits.command_memory_mb,
            timeout_seconds: limits.command_timeout_seconds,
            processes: limits.command_processes,
            output_chars: limits.command_output_chars,
        }
    }
}

/// Where a run stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: String,
    /// `running`, `exited`, `timed_out` (stopped at its time limit),
    /// `stopped` (by the person or the computer), or `failed` (the sandbox
    /// couldn't start it).
    pub state: String,
    /// While it's `exited`. A process ended by a signal, its CPU limit
    /// included, exits with -1.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// Bytes of output so far, both streams.
    pub output_bytes: u64,
    /// Why it `failed`.
    pub error: Option<String>,
    /// The limit that ended it: `cpu_time` (it `exited` with -1) or
    /// `processes` (it was `stopped`). Its wall clock is `timed_out`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
}

impl Status {
    pub fn running(&self) -> bool {
        self.state == "running"
    }
}

/// A piece of a run's output, both streams interleaved as they arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub id: String,
    /// Where `text` starts, in bytes since the run started.
    pub offset: u64,
    /// Where to ask from next.
    pub next_offset: u64,
    /// Bytes no longer kept between the offset asked for and `offset`.
    pub skipped: u64,
    pub text: String,
    /// The run has ended and nothing more will come.
    pub done: bool,
}

/// What a run printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finished {
    pub status: Status,
    /// The end of its standard output, at most `output_chars` characters.
    pub stdout: String,
    pub stderr: String,
    /// Characters cut from the start of each.
    pub stdout_cut: usize,
    pub stderr_cut: usize,
}

/// A run as this computer's own windows list it: what runs where, and how
/// it stands. Only over the control socket, never to the server, which has
/// its own record of what it asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    #[serde(flatten)]
    pub status: Status,
    /// The folder, by ID, and where in it.
    pub root: String,
    #[serde(default)]
    pub dir: String,
    pub command: String,
    #[serde(default)]
    pub network: bool,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Running,
    Exited(i32),
    TimedOut,
    Stopped,
    Failed(String),
}

/// One run's output and state, shared between the thread that waits for it
/// and the callers.
struct Cell {
    id: String,
    /// The folder it runs in, by ID, where in it, and what.
    root: String,
    dir: String,
    command: String,
    network: bool,
    started: Instant,
    started_at: u64,
    /// The limit that ended it, set by its thread.
    limit: Option<&'static str>,
    ended: Option<Instant>,
    state: State,
    stop: bool,
    /// The latest output, both streams, and the offset of its first byte.
    window: VecDeque<u8>,
    window_start: u64,
    total: u64,
    /// The end of each stream, for the result.
    out: VecDeque<u8>,
    err: VecDeque<u8>,
    out_total: u64,
    err_total: u64,
    tail_bytes: usize,
}

impl Cell {
    fn new(plan: &Plan, root: String, dir: String) -> Self {
        let limits = &plan.limits;
        Self {
            id: plan.id.clone(),
            root,
            dir,
            command: plan.command.clone(),
            network: plan.network,
            started: Instant::now(),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            limit: None,
            ended: None,
            state: State::Running,
            stop: false,
            window: VecDeque::new(),
            window_start: 0,
            total: 0,
            out: VecDeque::new(),
            err: VecDeque::new(),
            out_total: 0,
            err_total: 0,
            // UTF-8 takes at most 4 bytes a character.
            tail_bytes: limits.output_chars.saturating_mul(4).max(1024),
        }
    }

    fn append(&mut self, stream: Stream, bytes: &[u8]) {
        self.window.extend(bytes);
        self.total += bytes.len() as u64;
        while self.window.len() > WINDOW {
            let drop = self.window.len() - WINDOW;
            self.window.drain(..drop);
            self.window_start += drop as u64;
        }
        let (tail, total) = match stream {
            Stream::Out => (&mut self.out, &mut self.out_total),
            Stream::Err => (&mut self.err, &mut self.err_total),
        };
        tail.extend(bytes);
        *total += bytes.len() as u64;
        if tail.len() > self.tail_bytes {
            let drop = tail.len() - self.tail_bytes;
            tail.drain(..drop);
        }
    }

    fn finish(&mut self, state: State) {
        if self.state == State::Running {
            self.state = state;
            self.ended = Some(Instant::now());
        }
    }

    fn status(&self) -> Status {
        let (state, exit_code, error) = match &self.state {
            State::Running => ("running", None, None),
            State::Exited(code) => ("exited", Some(*code), None),
            State::TimedOut => ("timed_out", None, None),
            State::Stopped => ("stopped", None, None),
            State::Failed(error) => ("failed", None, Some(error.clone())),
        };
        let duration = self.ended.unwrap_or_else(Instant::now) - self.started;
        Status {
            id: self.id.clone(),
            state: state.into(),
            exit_code,
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            output_bytes: self.total,
            error,
            limit: self.limit.map(String::from),
        }
    }

    fn summary(&self) -> Summary {
        Summary {
            status: self.status(),
            root: self.root.clone(),
            dir: self.dir.clone(),
            command: self.command.clone(),
            network: self.network,
            started_at: self.started_at,
        }
    }

    fn output(&self, offset: u64, max: usize) -> Output {
        let start = offset.clamp(self.window_start, self.total);
        let skipped = start - offset.min(start);
        let from = usize::try_from(start - self.window_start).unwrap_or(usize::MAX);
        let mut bytes: Vec<u8> = self
            .window
            .iter()
            .skip(from)
            .take(max.min(MAX_OUTPUT_CHUNK))
            .copied()
            .collect();
        // Never end in the middle of a character, unless that's all there is.
        let whole = utf8_prefix(&bytes);
        if whole > 0 {
            bytes.truncate(whole);
        }
        let next = start + bytes.len() as u64;
        Output {
            id: self.id.clone(),
            offset: start,
            next_offset: next,
            skipped,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            done: self.state != State::Running && next >= self.total,
        }
    }

    fn finished(&self, chars: usize) -> Finished {
        let (stdout, stdout_cut) = tail_text(&self.out, self.out_total, chars);
        let (stderr, stderr_cut) = tail_text(&self.err, self.err_total, chars);
        Finished {
            status: self.status(),
            stdout,
            stderr,
            stdout_cut,
            stderr_cut,
        }
    }
}

/// The longest prefix of `bytes` that doesn't end inside a character.
fn utf8_prefix(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(),
    }
}

/// The last `chars` characters of a stream, and how many were cut (an
/// estimate when the kept bytes are already only its end).
fn tail_text(tail: &VecDeque<u8>, total: u64, chars: usize) -> (String, usize) {
    let bytes: Vec<u8> = tail.iter().copied().collect();
    let text = String::from_utf8_lossy(&bytes);
    // A cut start can split a character: drop the replacement it leaves.
    let text = if (bytes.len() as u64) < total {
        text.trim_start_matches('\u{FFFD}')
    } else {
        &text
    };
    let count = text.chars().count();
    let dropped_bytes = usize::try_from(total).unwrap_or(usize::MAX) - bytes.len();
    if count <= chars {
        return (text.to_string(), dropped_bytes);
    }
    let skip = count - chars;
    let kept: String = text.chars().skip(skip).collect();
    (kept, skip + dropped_bytes)
}

/// The runs this process started, running and recently finished.
#[derive(Clone)]
pub struct Runs {
    inner: Arc<Mutex<Inner>>,
    paths: Paths,
}

struct Inner {
    runs: HashMap<String, Arc<Mutex<Cell>>>,
    /// Finished runs, oldest first.
    finished: VecDeque<String>,
    limits: Limits,
}

impl Runs {
    pub fn new(paths: Paths) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                runs: HashMap::new(),
                finished: VecDeque::new(),
                limits: Limits::from_config(&crate::config::Limits::default()),
            })),
            paths,
        }
    }

    /// Start `request` as `config` allows it, checked here, and return its
    /// status at once.
    pub fn start(&self, config: &Config, request: &Request) -> Result<Status, RunError> {
        if config.paused {
            return Err(RunError::new(
                "paused",
                "this computer is paused; resume it to run commands",
            ));
        }
        let running = self.running();
        if running >= config.limits.commands_running {
            return Err(RunError::new(
                "busy",
                format!(
                    "{running} commands are already running here; wait for one to end, or stop one"
                ),
            ));
        }
        let id = new_id();
        let plan = plan(config, &self.paths, request, &id)?;
        let cell = Arc::new(Mutex::new(Cell::new(
            &plan,
            request.root.clone(),
            request.dir.clone(),
        )));
        {
            let mut inner = lock(&self.inner);
            inner.limits = plan.limits;
            inner.runs.insert(id.clone(), cell.clone());
        }
        self.launch(plan, cell.clone());
        let status = lock(&cell).status();
        Ok(status)
    }

    #[cfg(unix)]
    fn launch(&self, plan: Plan, cell: Arc<Mutex<Cell>>) {
        let runs = self.clone();
        run::spawn(plan, cell, move |id| runs.retire(id));
    }

    #[cfg(not(unix))]
    fn launch(&self, _plan: Plan, cell: Arc<Mutex<Cell>>) {
        let id = {
            let mut cell = lock(&cell);
            cell.finish(State::Failed(
                "commands can't run on this system yet".into(),
            ));
            cell.id.clone()
        };
        self.retire(&id);
    }

    /// A run ended: keep it for a while, forgetting the oldest.
    fn retire(&self, id: &str) {
        let mut inner = lock(&self.inner);
        inner.finished.push_back(id.to_string());
        while inner.finished.len() > FINISHED_KEPT {
            if let Some(old) = inner.finished.pop_front() {
                inner.runs.remove(&old);
            }
        }
    }

    fn cell(&self, id: &str) -> Result<Arc<Mutex<Cell>>, RunError> {
        lock(&self.inner)
            .runs
            .get(id)
            .cloned()
            .ok_or_else(|| RunError::new("not_found", format!("no command {id} here")))
    }

    pub fn status(&self, id: &str) -> Result<Status, RunError> {
        let cell = self.cell(id)?;
        let status = lock(&cell).status();
        Ok(status)
    }

    pub fn output(&self, id: &str, offset: u64, max: usize) -> Result<Output, RunError> {
        let cell = self.cell(id)?;
        let output = lock(&cell).output(offset, max);
        Ok(output)
    }

    /// How it ended, or how it stands, with the end of each stream.
    pub fn finished(&self, id: &str) -> Result<Finished, RunError> {
        let chars = lock(&self.inner).limits.output_chars;
        let cell = self.cell(id)?;
        let finished = lock(&cell).finished(chars);
        Ok(finished)
    }

    /// Ask a run to stop. Its status says `stopped` once it has.
    pub fn stop(&self, id: &str) -> Result<Status, RunError> {
        let cell = self.cell(id)?;
        let mut cell = lock(&cell);
        cell.stop = true;
        Ok(cell.status())
    }

    /// Stop every run and wait, up to `limit`, until each has ended.
    pub fn stop_all_and_wait(&self, limit: std::time::Duration) {
        self.stop_all();
        let deadline = Instant::now() + limit;
        while self.running() > 0 && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Stop every run (the worker restarted, or the daemon stops).
    pub fn stop_all(&self) {
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        for cell in cells {
            lock(&cell).stop = true;
        }
    }

    /// Stop the runs in folders that no longer allow commands, and all of
    /// them while the computer is paused.
    pub fn stop_outside(&self, config: &Config) {
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        for cell in cells {
            let mut cell = lock(&cell);
            let allowed = !config.paused
                && config
                    .roots
                    .iter()
                    .any(|r| r.id == cell.root && r.runs_commands());
            if cell.state == State::Running && !allowed {
                cell.stop = true;
            }
        }
    }

    /// Every run kept, running ones first, then the latest to start.
    pub fn list(&self) -> Vec<Summary> {
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        let mut list: Vec<(Instant, Summary)> = cells
            .iter()
            .map(|c| {
                let cell = lock(c);
                (cell.started, cell.summary())
            })
            .collect();
        list.sort_by(|(a, x), (b, y)| y.status.running().cmp(&x.status.running()).then(b.cmp(a)));
        list.into_iter().map(|(_, s)| s).collect()
    }

    pub fn running(&self) -> usize {
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        cells
            .iter()
            .filter(|c| lock(c).state == State::Running)
            .count()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A run's handle: 12 random hex digits.
fn new_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 6];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system's random source works");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `id` looks like a handle this module made.
pub fn valid_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Where this process's commands run: here (an unconfined daemon), through
/// the supervisor (the confined worker), or nowhere.
#[derive(Clone)]
pub enum Runner {
    Local(Runs),
    #[cfg(unix)]
    Supervisor(channel::Client),
    /// Why commands can't run here.
    Off(String),
}

impl Runner {
    /// For a daemon that isn't confined: runs start here, checked against
    /// the config on disk as the supervisor would.
    pub fn local(paths: &Paths) -> Self {
        if cfg!(windows) {
            Self::Off("Commands can't run on Windows yet: it has no sandbox for them".into())
        } else {
            Self::Local(Runs::new(paths.clone()))
        }
    }

    async fn call(&self, call: channel_call::Call) -> Result<serde_json::Value, RunError> {
        match self {
            Self::Local(runs) => {
                let runs = runs.clone();
                tokio::task::spawn_blocking(move || {
                    let paths = runs.paths.clone();
                    channel_call::answer(&runs, &paths, call)
                })
                .await
                .map_err(|_| RunError::new("unavailable", "the command runner stopped"))?
            }
            #[cfg(unix)]
            Self::Supervisor(client) => client.call(&call).await,
            Self::Off(reason) => Err(RunError::new("unavailable", reason.clone())),
        }
    }

    pub async fn start(&self, request: Request) -> Result<Status, RunError> {
        parse(self.call(channel_call::Call::Start { request }).await?)
    }

    pub async fn status(&self, run: &str) -> Result<Status, RunError> {
        parse(
            self.call(channel_call::Call::Status { run: run.into() })
                .await?,
        )
    }

    pub async fn output(&self, run: &str, offset: u64, max: usize) -> Result<Output, RunError> {
        parse(
            self.call(channel_call::Call::Output {
                run: run.into(),
                offset,
                max,
            })
            .await?,
        )
    }

    pub async fn finished(&self, run: &str) -> Result<Finished, RunError> {
        parse(
            self.call(channel_call::Call::Finished { run: run.into() })
                .await?,
        )
    }

    pub async fn stop(&self, run: &str) -> Result<Status, RunError> {
        parse(
            self.call(channel_call::Call::Stop { run: run.into() })
                .await?,
        )
    }

    /// Folders changed, or the computer was paused: stop the runs where
    /// commands aren't allowed now.
    pub async fn prune(&self) {
        let _ = self.call(channel_call::Call::Prune).await;
    }

    /// Every run kept, for this computer's own windows; none where
    /// commands can't run.
    pub async fn list(&self) -> Result<Vec<Summary>, RunError> {
        if matches!(self, Self::Off(_)) {
            return Ok(Vec::new());
        }
        parse(self.call(channel_call::Call::List).await?)
    }

    /// The device was revoked: stop every run, without waiting.
    pub async fn stop_everything(&self) {
        let _ = self.call(channel_call::Call::StopAll).await;
    }

    /// Stop everything this daemon started, and wait a moment for it to
    /// end (the daemon is stopping).
    pub fn stop_all(&self) {
        if let Self::Local(runs) = self {
            runs.stop_all_and_wait(std::time::Duration::from_secs(5));
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T, RunError> {
    serde_json::from_value(value)
        .map_err(|_| RunError::new("unavailable", "an answer this version doesn't understand"))
}
