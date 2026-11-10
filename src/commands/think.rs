//! Thinking steps (docs/own-plans.md), on the side that starts commands:
//! the supervisor on Linux and macOS, the daemon on Windows. The worker's
//! request names the agent and the bot; everything else that decides what
//! the step gets is read here, from the config on disk and the home folder.
//!
//! A step runs as a command does (`run.rs`): a sandbox of its own, its own
//! home, the agent's sign-in lent as a bot's coding run gets it, its
//! tokens taken out of what it prints, the limits. What differs: its folder
//! is a fresh one in Desktop's data folder, removed after, never a shared
//! one; it reaches the internet and nothing of this computer; the command
//! line is built here from the agent's kind; and it's kept apart from the
//! commands, so no `run_status` or window shows it. Its standard output is
//! read as the agent's events (`crate::plans::parse`), and the agent is
//! stopped as soon as it asked for Chat with Work's tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Cell, Limits, Plan, RunError, Runs, Sandbox, State, lock, new_id, run};
use crate::coding::{Agent, Login, RunCoding};
use crate::config::Config;
use crate::paths::Paths;
use crate::plans::{self, ThinkAsk, parse};

/// Most of a step's output kept: the events of a long answer fit many times.
pub const MAX_OUTPUT: usize = 8 * 1024 * 1024;
/// Most bytes of raw output one status call carries.
const RAW_CHUNK: usize = 64 * 1024;
/// How often a step's output is read.
const POLL: Duration = Duration::from_millis(50);
/// Finished steps kept for the worker to read their answer.
const FINISHED_KEPT: usize = 16;

/// The steps this process started.
#[derive(Clone, Default)]
pub struct Thinks {
    inner: Arc<Mutex<HashMap<String, Arc<Mutex<Think>>>>>,
    /// Steps being started: counted with the running ones, so two at once
    /// can't both take the last place.
    starting: Arc<Mutex<usize>>,
}

/// A place taken among the steps this computer runs at once, given back
/// when the step is started or wasn't.
struct Reservation(Arc<Mutex<usize>>);

impl Drop for Reservation {
    fn drop(&mut self) {
        *lock(&self.0) -= 1;
    }
}

struct Think {
    agent: Agent,
    raw: bool,
    events: Box<dyn parse::Events>,
    cell: Arc<Mutex<Cell>>,
    /// Its standard output, as the run kept it (tokens taken out).
    out: Arc<Mutex<Vec<u8>>>,
    /// How much of it the parser read, and a line not ended yet.
    read: usize,
    partial: Vec<u8>,
    /// How it ended: its state, its exit code, whether it was stopped.
    ended: Option<(String, Option<i32>, Option<String>)>,
    /// Stopped by a cancel, never by its answer.
    cancelled: bool,
    ended_at: Option<Instant>,
}

impl Thinks {
    fn running(&self) -> usize {
        let thinks: Vec<_> = lock(&self.inner).values().cloned().collect();
        thinks.iter().filter(|t| lock(t).ended.is_none()).count()
    }

    /// A place for one more step, unless `most` already run or start.
    fn reserve(&self, most: usize) -> Result<Reservation, usize> {
        let mut starting = lock(&self.starting);
        let taken = self.running() + *starting;
        if most > 0 && taken >= most {
            return Err(taken);
        }
        *starting += 1;
        Ok(Reservation(self.starting.clone()))
    }

    fn get(&self, id: &str) -> Result<Arc<Mutex<Think>>, RunError> {
        lock(&self.inner)
            .get(id)
            .cloned()
            .ok_or_else(|| RunError::new("not_found", "no such step"))
    }

    /// Stop every step (the worker restarted, the computer was paused or
    /// revoked, the daemon stops).
    pub fn stop_all(&self) {
        let thinks: Vec<_> = lock(&self.inner).values().cloned().collect();
        for think in thinks {
            let think = lock(&think);
            lock(&think.cell).stop = true;
        }
    }

    /// Forget the oldest finished steps past what's kept.
    fn prune(&self) {
        let mut inner = lock(&self.inner);
        let mut finished: Vec<(Instant, String)> = inner
            .iter()
            .filter_map(|(id, t)| lock(t).ended_at.map(|at| (at, id.clone())))
            .collect();
        if finished.len() <= FINISHED_KEPT {
            return;
        }
        finished.sort();
        for (_, id) in finished.iter().take(finished.len() - FINISHED_KEPT) {
            inner.remove(id);
        }
    }
}

/// Why a step can't run on this computer, or which sandbox it gets.
pub fn sandbox() -> Result<Sandbox, String> {
    match Sandbox::for_host() {
        // Landlock could only give the whole network, this computer's own
        // services with it.
        Ok(Sandbox::Landlock) => Err("this computer can't give a step the internet safely: \
             Bubblewrap can't create a sandbox here"
            .into()),
        #[cfg(target_os = "linux")]
        Ok(Sandbox::Bubblewrap) if !super::plan::own_network_available() => Err(
            "this computer can't give a step its own network: slirp4netns, util-linux or \
                 iptables is missing, or unprivileged namespaces are refused"
                .into(),
        ),
        Ok(sandbox) => Ok(sandbox),
        Err(e) => Err(e.message),
    }
}

#[cfg(test)]
thread_local! {
    /// A stand-in for `cww` in tests, where this program is the test binary.
    pub(crate) static TEST_SHIM: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The `cww` that serves a step's tools: this program.
fn shim_program() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(shim) = TEST_SHIM.with(|s| s.borrow().clone()) {
        return Some(shim);
    }
    #[cfg(test)]
    if let Ok(program) = std::env::var("CWW_TEST_CWW") {
        return Some(PathBuf::from(program));
    }
    let exe = crate::roots::canonical(&std::env::current_exe().ok()?).ok()?;
    // On Windows the daemon is `cww-agent.exe`, which has no commands of
    // its own: `cww.exe` ships beside it.
    if exe.file_stem().is_some_and(|stem| stem == "cww-agent") {
        let cww = exe.with_file_name(format!("cww{}", std::env::consts::EXE_SUFFIX));
        return cww.is_file().then_some(cww);
    }
    Some(exe)
}

/// Remove the folders of steps a killed supervisor or daemon left behind.
/// Only where steps start, and before any does.
pub fn sweep(paths: &Paths) {
    let Ok(entries) = std::fs::read_dir(paths.data_dir.join("thinking")) else {
        return;
    };
    for entry in entries.flatten() {
        remove_folder(&entry.path());
    }
}

/// Start a step as `config` allows it, checked here, and answer its ID.
pub fn start(
    runs: &Runs,
    config: &Config,
    paths: &Paths,
    ask: &ThinkAsk,
) -> Result<Value, RunError> {
    let agent = plans::validate(ask).map_err(|e| RunError::new("invalid", e))?;
    if config.paused {
        return Err(RunError::new(
            "paused",
            "this computer is paused; resume it to think here",
        ));
    }
    if !plans::offered(config, agent) {
        return Err(RunError::new(
            "unavailable",
            format!("{} isn't offered on this computer", plans::name(agent)),
        ));
    }
    let whom = plans::for_whom(ask.bot.as_deref());
    if !plans::allowed(config, agent, ask.bot.as_deref()) {
        return Err(RunError::new(
            "not_allowed",
            format!(
                "this computer didn't allow its {} plan for {whom}: the person can allow it in \
                 Chat with Work Desktop, or with `cww plans allow`",
                plans::plan_name(agent)
            ),
        ));
    }
    if let Some(why) = agent.unavailable() {
        return Err(RunError::new("unavailable", why));
    }
    let sandbox = sandbox().map_err(|e| RunError::new("unavailable", e))?;
    let Some(home) = paths.person_home.as_deref() else {
        return Err(RunError::new("unavailable", "no home folder to find it in"));
    };
    let login = Login::find_for(agent, home);
    let Some(agent_run) = crate::coding::agent_for_thinking(agent, config, paths, login.clone())
    else {
        return Err(RunError::new(
            "not_installed",
            format!("{} isn't installed on this computer", plans::name(agent)),
        ));
    };
    if !login.signed_in() {
        return Err(RunError::new(
            "not_signed_in",
            format!(
                "{} isn't signed in on this computer: {}",
                plans::name(agent),
                agent.sign_in_hint()
            ),
        ));
    }
    // Where it really is, for a step's Codex home to take the sign-in from.
    let sign_in = (agent == Agent::Codex).then(|| login.dir.clone()).flatten();
    let shim =
        shim_program().ok_or_else(|| RunError::new("unavailable", "couldn't find this program"))?;
    let reservation = runs
        .thinks
        .reserve(config.limits.commands_running)
        .map_err(|running| {
            RunError::new(
                "busy",
                format!(
                    "{running} steps are already thinking on this computer, as many as it runs \
                     at once"
                ),
            )
        })?;
    let id = new_id();
    let folder = make_folder(paths, &id)
        .map_err(|e| RunError::new("unavailable", format!("couldn't make its folder: {e}")))?;
    let started = (|| {
        plans::write_files(&folder, ask, agent, &shim, sign_in.as_deref())
            .map_err(|e| RunError::new("unavailable", format!("couldn't write its files: {e}")))?;
        let mut limits = Limits::from_config(&config.limits);
        limits.timeout_seconds = plans::STEP_SECONDS;
        // Windows grants folders, so `cww.exe`'s own, unless that's too
        // broad or holds something shared.
        let shim_read = if cfg!(windows) {
            crate::coding::grant(&shim, config, paths).ok_or_else(|| {
                RunError::new(
                    "unavailable",
                    "the folder this program is in holds too much to share with a step",
                )
            })?
        } else {
            shim.clone()
        };
        let plan = Plan {
            id: id.clone(),
            command: plans::command(agent, &folder, &shim, ask.model.as_deref()),
            folder: folder.clone(),
            cwd: folder.clone(),
            network: true,
            local_services: false,
            reach: Vec::new(),
            once: Vec::new(),
            networks: Default::default(),
            limits,
            home: paths.runs_dir().join(&id),
            coding: RunCoding {
                mirrors: Vec::new(),
                agents: vec![agent_run],
                tools: None,
                read: vec![shim_read],
                checkouts: Vec::new(),
            },
            env: Default::default(),
            cgroup: None,
            internet_only: true,
        };
        if let Some(e) = sandbox.refuses(&plan) {
            return Err(RunError::new("unavailable", e.message));
        }
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut cell = Cell::new(&plan, "thinking".into(), String::new());
        cell.tap = Some(out.clone());
        let cell = Arc::new(Mutex::new(cell));
        let think = Arc::new(Mutex::new(Think {
            agent,
            raw: ask.raw,
            events: parse::parser(agent, &id, ask.model.clone()),
            cell: cell.clone(),
            out,
            read: 0,
            partial: Vec::new(),
            ended: None,
            cancelled: false,
            ended_at: None,
        }));
        lock(&runs.thinks.inner).insert(id.clone(), think.clone());
        // Counted as running from here.
        drop(reservation);
        if let Err(e) = watch(think) {
            lock(&cell).stop = true;
            lock(&runs.thinks.inner).remove(&id);
            return Err(RunError::new(
                "unavailable",
                format!("couldn't follow the step: {}", e.kind()),
            ));
        }
        let (removing, returning) = (folder.clone(), sign_in.clone());
        run::spawn(plan, cell, move |_| {
            if let Some(returning) = &returning
                && let Err(e) = plans::return_codex_sign_in(&removing, returning)
            {
                tracing::warn!(error = %e.kind(), "a step's refreshed sign-in wasn't kept");
            }
            remove_folder(&removing);
        });
        Ok(())
    })();
    if let Err(e) = started {
        remove_folder(&folder);
        return Err(e);
    }
    runs.thinks.prune();
    tracing::info!(step = %id, agent = agent.key(), "a thinking step started");
    Ok(json!({ "id": id }))
}

/// The step's own folder, in Desktop's data folder, private to this user:
/// new, so nothing already there is written through.
fn make_folder(paths: &Paths, id: &str) -> std::io::Result<PathBuf> {
    let parent = paths.data_dir.join("thinking");
    std::fs::create_dir_all(&parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let folder = parent.join(id);
    std::fs::create_dir(&folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    }
    crate::roots::canonical(&folder).map_err(std::io::Error::other)
}

fn remove_folder(folder: &Path) {
    match std::fs::remove_dir_all(folder) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e.kind(), "a thinking step's folder wasn't removed"),
    }
}

/// Read the step's output as it comes, stop it once it has its answer, and
/// note how it ended.
fn watch(think: Arc<Mutex<Think>>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("cww-think".into())
        .spawn(move || {
            loop {
                std::thread::sleep(POLL);
                let mut think = lock(&think);
                think.read_events(Instant::now());
                let cell = think.cell.clone();
                let mut cell = lock(&cell);
                let running = cell.state == State::Running;
                if running && !think.raw && think.events.done(Instant::now()) {
                    cell.stop = true;
                }
                if lock(&think.out).len() >= MAX_OUTPUT && running {
                    cell.stop = true;
                }
                if !running {
                    drop(cell);
                    think.read_events(Instant::now());
                    let mut cell = lock(&think.cell);
                    let (state, exit, error) = match &cell.state {
                        State::Exited(code) => ("exited", Some(*code), None),
                        State::TimedOut => ("timed_out", None, None),
                        State::Stopped => ("stopped", None, None),
                        State::Failed(e) => ("failed", None, Some(e.clone())),
                        State::Running => ("running", None, None),
                    };
                    cell.stop = true;
                    drop(cell);
                    think.ended = Some((state.to_string(), exit, error));
                    think.ended_at = Some(Instant::now());
                    return;
                }
            }
        })
        .map(|_| ())
}

impl Think {
    /// Give the parser the whole lines printed since it last read.
    fn read_events(&mut self, now: Instant) {
        if self.raw {
            return;
        }
        let new: Vec<u8> = {
            let out = lock(&self.out);
            let new = out[self.read.min(out.len())..].to_vec();
            self.read = out.len();
            new
        };
        self.partial.extend(new);
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            self.events.line(line.trim_end(), now);
        }
        // What's left at the end, once it ended.
        if self.ended.is_none() && lock(&self.cell).state != State::Running {
            let rest = std::mem::take(&mut self.partial);
            if !rest.is_empty() {
                self.events
                    .line(String::from_utf8_lossy(&rest).trim_end(), now);
            }
        }
    }

    /// Why it ended without an answer, if it did: never what it said.
    fn failure(&self) -> Option<String> {
        let (state, exit, error) = self.ended.as_ref()?;
        let step = self.events.step();
        let name = plans::name(self.agent);
        if !step.tool_calls.is_empty() {
            return None;
        }
        if let Some(said) = step.error.as_deref().filter(|e| !e.is_empty()) {
            return Some(format!("{name} said: {said}"));
        }
        if step.stop == Some(parse::Stop::EndTurn) {
            return None;
        }
        Some(match (state.as_str(), exit, error) {
            ("timed_out", _, _) => format!(
                "the step took longer than {} minutes",
                plans::STEP_SECONDS / 60
            ),
            ("failed", _, Some(e)) if e.starts_with("commands can't run") => {
                "this computer can't run it in a sandbox".into()
            }
            ("failed", _, _) => "the sandbox couldn't start it".into(),
            (_, Some(code), _) => format!("{name} stopped without answering (exit {code})"),
            _ => format!("{name} stopped without answering"),
        })
    }
}

/// How a step stands: its text from character `from` (parsed), or its
/// output from byte `from` (raw), and its answer once it has one.
pub fn status(runs: &Runs, id: &str, from: u64) -> Result<Value, RunError> {
    let think = runs.thinks.get(id)?;
    let think = lock(&think);
    let done = think.ended.is_some();
    if think.raw {
        let out = lock(&think.out);
        let start = usize::try_from(from).unwrap_or(usize::MAX).min(out.len());
        let end = raw_end(&out, start, RAW_CHUNK);
        let chunk = String::from_utf8_lossy(&out[start..end]).into_owned();
        let all = done && end >= out.len();
        let mut value = json!({ "raw": chunk, "next": end, "done": all });
        if all && let Some((state, exit, _)) = &think.ended {
            value["result"] = json!({ "exit": exit, "stopped": state == "stopped" });
        }
        return Ok(value);
    }
    let step = think.events.step();
    let text: String = step
        .text
        .chars()
        .skip(usize::try_from(from).unwrap_or(usize::MAX))
        .collect();
    let mut value = json!({
        "text": text,
        "chars": step.text.chars().count(),
        "done": done,
    });
    if done {
        match think.failure() {
            Some(message) if !think.cancelled => {
                value["error"] = json!({ "code": "failed", "message": message });
            }
            _ => value["result"] = step.result(),
        }
    }
    Ok(value)
}

/// Where a chunk of at most `chunk` raw bytes from `start` ends: before a
/// character cut at the end of the chunk, and always past `start`, whatever
/// the bytes are (a character cut at the beginning, invalid bytes).
fn raw_end(out: &[u8], start: usize, chunk: usize) -> usize {
    let end = (start + chunk).min(out.len());
    if end < out.len() {
        // A character is at most 4 bytes, so only the last 3 can be cut.
        for back in (1..=3).take_while(|back| end > start + back) {
            if let Err(e) = std::str::from_utf8(&out[end - back..end])
                && e.valid_up_to() == 0
                && e.error_len().is_none()
            {
                return end - back;
            }
        }
    }
    end
}

/// Stop a step at once (the server cancelled it).
pub fn stop(runs: &Runs, id: &str) -> Result<Value, RunError> {
    let think = runs.thinks.get(id)?;
    let mut think = lock(&think);
    think.cancelled = true;
    lock(&think.cell).stop = true;
    Ok(Value::Null)
}

#[cfg(test)]
mod raw_tests {
    use super::raw_end;

    #[test]
    fn a_raw_chunk_moves_past_cut_and_invalid_bytes() {
        // Stops before a character the chunk would cut.
        let text = "aé".as_bytes();
        assert_eq!(raw_end(text, 0, 2), 1);
        // Starting in the middle of a character, or at invalid bytes, it
        // still moves on: a stalled `next` would never end.
        let bad = [0xa9, 0xff, b'x', b'y'];
        assert!(raw_end(&bad, 0, 2) > 0);
        assert_eq!(raw_end(&bad, 0, 10), bad.len());
    }
}
