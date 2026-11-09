//! Commands run in a shared folder (`run`).
//!
//! The confined worker can't execute anything in a shared folder, by design
//! (`sandbox.rs`). So it asks the unconfined process above it (the
//! supervisor, see [`channel`]) to start each command in a sandbox of its
//! own, built for that one run with MXC: Bubblewrap on Linux (Landlock where
//! Bubblewrap can't create its namespaces), Seatbelt on macOS, a process
//! container on Windows, where the daemon isn't confined and starts them
//! itself (`windows.rs`). The run reads the system and the language
//! toolchains, writes only its folder and a home of its own, has no network
//! unless the folder allows it, gets a clean environment, and has CPU time,
//! memory and wall-clock limits. Whoever starts it checks the folder again
//! from the config on disk: the worker names a folder by its ID and a
//! relative path, never an absolute one.
//!
//! Output is kept as it arrives: a window of the latest bytes for
//! [`Runs::output`] (what a running command printed so far), and the end of
//! each stream for the result.

mod blocked;
#[cfg(target_os = "linux")]
mod cgroup;
#[cfg(unix)]
pub mod channel;
mod channel_call;
pub mod env;
pub mod follow;
mod plan;
mod run;
#[cfg(test)]
mod tests;
pub mod tools;
pub mod windows;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use plan::{Plan, Request, Sandbox, plan};

use crate::config::Config;
use crate::paths::Paths;

/// The language toolchains every run on this computer reads: a program
/// inside one (Claude Code installed by mise, say) is already readable.
pub fn toolchain_dirs() -> Vec<std::path::PathBuf> {
    #[cfg(windows)]
    {
        windows::Host::current().toolchains().read
    }
    #[cfg(not(windows))]
    {
        plan::toolchain_dirs(crate::paths::home_dir().ok().as_deref())
    }
}

/// How far a folder's commands reach on this computer: its setting, as far
/// as this computer's kind of sandbox holds it. Windows's process container
/// can't reach this computer's own services, so there a folder has the
/// internet or nothing. (A Linux computer whose commands run on Landlock
/// gives local services nothing either, which only a command's start finds
/// out: docs/commands.md says so.)
pub fn network_access(root: &crate::config::Root) -> crate::reach::Level {
    let level = root.network_level();
    if cfg!(windows) && level == crate::reach::Level::Local {
        return crate::reach::Level::None;
    }
    level
}

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
    /// The limits this computer's sandbox holds. Seatbelt can't hold a
    /// memory limit (macOS doesn't enforce memory rlimits, so MXC refuses
    /// `memoryMb` there) and nothing counts a run's processes on macOS yet,
    /// so both are 0 there, none, and the approval card leaves them out
    /// rather than promise them. On Linux the memory limit is a cgroup's
    /// (`cgroup.rs`), held where systemd delegated the daemon one.
    pub fn from_config(limits: &crate::config::Limits) -> Self {
        let held = !cfg!(target_os = "macos");
        Self {
            cpu_seconds: limits.command_cpu_seconds,
            memory_mb: if held { limits.command_memory_mb } else { 0 },
            timeout_seconds: limits.command_timeout_seconds,
            processes: if held { limits.command_processes } else { 0 },
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
    /// The limit that ended it: `cpu_time` (it `exited` with -1),
    /// `processes` (on Linux it was `stopped`; on Windows a process past
    /// the limit couldn't start, and it `exited` with an error) or `memory`
    /// (Linux: the kernel ended it at the limit, and it `exited` with -1 or
    /// 137; Windows: an allocation past the limit failed, and it `exited`
    /// with an error). Its wall clock is `timed_out`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// What the sandbox refused it, at most [`MAX_BLOCKED`], each once:
    /// connections it blocked, and paths outside its folder it couldn't
    /// write or read (those from what it printed, so a best guess).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<Blocked>,
    /// How far this run reached (`none`, `local`, `internet`), once its
    /// sandbox is known: what its folder allows, as far as the sandbox holds
    /// it, with its "Allow once". Between whoever runs it and the tools,
    /// which say it as the result's own `network_access`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_access: Option<String>,
}

/// Most entries a run's `blocked` lists.
pub const MAX_BLOCKED: usize = 5;

/// One thing the sandbox refused a run (PROTOCOL.md 8.15).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Blocked {
    Network {
        host: String,
        port: u16,
        protocol: String,
        #[serde(rename = "where")]
        place: crate::reach::Where,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        service: Option<String>,
    },
    Path {
        /// The home folder shown as `~`.
        path: String,
        /// `write` or `read`.
        access: String,
    },
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
    /// Take the run's secrets, if it has any, out of each stream, and out
    /// of the names it looked up.
    redact_out: crate::coding::Redactor,
    redact_err: crate::coding::Redactor,
    redact_names: crate::coding::Redactor,
    /// What the sandbox refused it so far.
    blocked: Vec<Blocked>,
    /// Where its sandbox logs the connections it blocked, how far that's
    /// read, and this computer's networks to place them.
    blocked_log: Option<(std::path::PathBuf, u64)>,
    host: crate::reach::HostNetwork,
    /// Its folder and home, whose paths are never "outside".
    folder: std::path::PathBuf,
    home: std::path::PathBuf,
    /// How far it reaches, once its sandbox is known.
    access: Option<crate::reach::Level>,
    /// Its folder's network when it started: narrowing it stops the run,
    /// whose sandbox can't be narrowed while it runs.
    started_with: (crate::reach::Level, Vec<std::net::SocketAddrV4>),
}

impl Cell {
    fn new(plan: &Plan, root: String, dir: String) -> Self {
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
            tail_bytes: plan.limits.output_chars.saturating_mul(4).max(1024),
            redact_out: plan.redactor(),
            redact_err: plan.redactor(),
            redact_names: plan.redactor(),
            blocked: Vec::new(),
            blocked_log: None,
            host: Default::default(),
            folder: plan.folder.clone(),
            home: plan.home.clone(),
            access: None,
            started_with: (
                crate::reach::Level::of(plan.local_services, plan.network),
                plan.reach.clone(),
            ),
        }
    }

    /// Follow what `plan`'s sandbox blocks, once its networks are known.
    fn watch_blocked(&mut self, plan: &Plan, sandbox: Sandbox) {
        self.access = Some(plan.network_access(sandbox));
        self.host = plan.networks.host.clone();
        self.blocked_log = plan.networks.log.clone().map(|log| (log, 0));
    }

    /// Read what the sandbox logged since last time.
    fn read_blocked(&mut self) {
        let Some((log, read)) = self.blocked_log.as_mut() else {
            return;
        };
        let Ok(text) = std::fs::read(&*log) else {
            return;
        };
        let start = usize::try_from(*read).unwrap_or(usize::MAX).min(text.len());
        let Some(end) = text[start..].iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let end = start + end + 1;
        *read = end as u64;
        let lines = String::from_utf8_lossy(&text[start..end]).into_owned();
        for line in lines.lines() {
            if let Some(mut blocked) = blocked::network(line, &self.host) {
                // A name looked up can carry a secret, as what it prints can.
                if let Blocked::Network { host, .. } = &mut blocked
                    && self.redact_names.found_in_name(host)
                {
                    *host = env::REDACTED.to_string();
                }
                self.add_blocked(blocked);
            }
        }
    }

    fn add_blocked(&mut self, blocked: Blocked) {
        // A lookup retried with the search domain ("rubygems.org.lan") says
        // nothing the first didn't.
        let retried = |b: &Blocked| match (b, &blocked) {
            (
                Blocked::Network {
                    host: first,
                    port: 53,
                    ..
                },
                Blocked::Network {
                    host: next,
                    port: 53,
                    ..
                },
            ) => next
                .strip_prefix(first.as_str())
                .is_some_and(|rest| rest.starts_with('.')),
            _ => false,
        };
        if self.blocked.len() < MAX_BLOCKED
            && !self.blocked.contains(&blocked)
            && !self.blocked.iter().any(retried)
        {
            self.blocked.push(blocked);
        }
    }

    /// Paths outside its folder the sandbox refused, from what it printed.
    fn find_blocked_paths(&mut self) {
        let person = crate::paths::home_dir().ok();
        let printed: String = [&self.out, &self.err]
            .iter()
            .map(|tail| {
                String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<u8>>()).into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for found in blocked::paths(&printed, &self.folder, &self.home, person.as_deref()) {
            self.add_blocked(found);
        }
    }

    /// Take out every secret `plan` has now, its folder's values included,
    /// once the run has worked them out.
    fn redact(&mut self, plan: &Plan) {
        self.redact_out = plan.redactor();
        self.redact_err = plan.redactor();
        self.redact_names = plan.redactor();
    }

    /// What a stream printed, with the run's secret taken out before
    /// anything keeps it.
    fn append(&mut self, stream: Stream, bytes: &[u8]) {
        let bytes = match stream {
            Stream::Out => self.redact_out.push(bytes),
            Stream::Err => self.redact_err.push(bytes),
        };
        self.keep(stream, &bytes);
    }

    fn keep(&mut self, stream: Stream, bytes: &[u8]) {
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
        let out = self.redact_out.finish();
        let err = self.redact_err.finish();
        self.keep(Stream::Out, &out);
        self.keep(Stream::Err, &err);
        self.read_blocked();
        self.find_blocked_paths();
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
            blocked: self.blocked.clone(),
            network_access: self.access.map(|level| level.as_str().to_string()),
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
    /// Runs that hold a place while they're made ready, which can be slow (a
    /// keychain's answer), so two starting at once never both take the last
    /// place, and neither waits for the other.
    starting: Arc<Mutex<usize>>,
    /// Repositories bots' first runs are preparing (`crate::coding::prepare`).
    preparations: crate::coding::prepare::Preparations,
    paths: Paths,
}

/// A run's place among those starting, given up when it's added or refused.
struct Place<'a>(&'a Mutex<usize>);

impl Drop for Place<'_> {
    fn drop(&mut self) {
        *lock(self.0) -= 1;
    }
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
            starting: Arc::new(Mutex::new(0)),
            preparations: Default::default(),
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
        let mut starting = lock(&self.starting);
        let running = self.running() + *starting;
        let most = config.limits.commands_running;
        if most > 0 && running >= most {
            return Err(RunError::new(
                "busy",
                format!(
                    "{running} commands are already running on this computer, as many as it runs \
                     at once. Wait for one to end (follow it with run_status) and run this \
                     again, or stop one"
                ),
            ));
        }
        *starting += 1;
        drop(starting);
        let place = Place(&self.starting);
        let id = new_id();
        let mut plan = plan(config, &self.paths, request, &id)?;
        // Read here, by whoever starts the run, never by the worker that
        // asked for it.
        // Whether Desktop made the folder's checkouts, settled once for
        // those set up before 0.16.1.
        let settled =
            crate::coding::trust::settle(config, &self.paths, &request.root).map_err(|_| {
                RunError::new(
                    "unavailable",
                    "this computer couldn't record its settings for the folder's repositories; \
                     run this again",
                )
            })?;
        plan.coding = crate::coding::for_run(
            &settled,
            &self.paths,
            &request.root,
            request.bot.as_deref(),
            |agent| {
                self.paths
                    .person_home
                    .as_deref()
                    .map(|home| crate::coding::Login::find_for(agent, home))
                    .unwrap_or_default()
            },
        );
        // The agents trust the checkouts Desktop made there, as the person,
        // before the run reads its settings.
        crate::coding::trust::seed(&plan.coding.agents, &plan.coding.checkouts, &config.roots);
        // The folder's environment, read here too; its `$( )` run in the
        // run's own sandbox, by its thread.
        plan.env.variables = env::for_run(config, &self.paths, &request.root)
            .map_err(|message| RunError::new("unavailable", message))?;
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
        drop(place);
        self.launch(plan, cell.clone());
        let status = lock(&cell).status();
        Ok(status)
    }

    fn launch(&self, plan: Plan, cell: Arc<Mutex<Cell>>) {
        let runs = self.clone();
        run::spawn(plan, cell, move |id| runs.retire(id));
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
        self.preparations.stop_all();
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        for cell in cells {
            lock(&cell).stop = true;
        }
    }

    /// Stop the runs in folders that no longer allow commands, or whose
    /// network was narrowed since they started (a sandbox's network can't
    /// be narrowed while it runs), and all of them while the computer is
    /// paused.
    pub fn stop_outside(&self, config: &Config) {
        let cells: Vec<_> = lock(&self.inner).runs.values().cloned().collect();
        for cell in cells {
            let mut cell = lock(&cell);
            let (level, reach) = &cell.started_with;
            let allowed = !config.paused
                && config.roots.iter().any(|r| {
                    r.id == cell.root
                        && r.runs_commands()
                        && r.network_level().rank() >= level.rank()
                        && (r.network_level() == crate::reach::Level::Internet
                            || reach.iter().all(|a| r.reach.contains(&a.to_string())))
                });
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
    /// For a daemon that isn't confined (the sandbox off, the tests, and
    /// Windows, which has no sandbox for the daemon yet): runs start here,
    /// checked against the config on disk as the supervisor would, each in
    /// a sandbox of its own all the same.
    pub fn local(paths: &Paths) -> Self {
        Self::Local(Runs::new(paths.clone()))
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

    /// Start preparing what a bot's run named, or hear what's missing.
    pub async fn prepare(
        &self,
        ask: crate::coding::prepare::Ask,
    ) -> Result<crate::coding::prepare::View, RunError> {
        parse(self.call(channel_call::Call::Prepare { ask }).await?)
    }

    /// How preparing it stands.
    pub async fn prepared(
        &self,
        ask: crate::coding::prepare::Ask,
    ) -> Result<Option<crate::coding::prepare::View>, RunError> {
        parse(self.call(channel_call::Call::Prepared { ask }).await?)
    }

    /// What this computer has for the coding agents.
    pub async fn coding_look(
        &self,
        repository: Option<String>,
    ) -> Result<crate::coding::prepare::Look, RunError> {
        parse(
            self.call(channel_call::Call::CodingLook { repository })
                .await?,
        )
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

    /// The system's printers, and how many pages may still print today.
    pub async fn printers(
        &self,
    ) -> Result<(Vec<crate::printing::SystemPrinter>, Option<u32>), RunError> {
        #[derive(Deserialize)]
        struct Answer {
            printers: Vec<crate::printing::SystemPrinter>,
            pages_left: Option<u32>,
        }
        let answer: Answer = parse(self.call(channel_call::Call::Printers).await?)?;
        Ok((answer.printers, answer.pages_left))
    }

    /// Print a file that arrived. Windows draws the pages itself, here,
    /// which can take longer than a call may: `None` says it's still at it,
    /// and it goes on. A call on the supervisor's channel isn't given up on
    /// here: `lp` answers within the channel's own time.
    pub async fn print(
        &self,
        request: crate::printing::Request,
    ) -> Result<Option<crate::printing::Receipt>, RunError> {
        let call = self.call(channel_call::Call::Print { request });
        if matches!(self, Self::Local(_)) {
            match tokio::time::timeout(tools::FOREGROUND_WAIT, call).await {
                Ok(answer) => parse(answer?).map(Some),
                Err(_) => Ok(None),
            }
        } else {
            parse(call.await?).map(Some)
        }
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
