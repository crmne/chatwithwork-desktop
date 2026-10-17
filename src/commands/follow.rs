//! Follows the commands this computer runs, for its own windows: the
//! desktop app's chat page and `cww tui` list them under the conversation,
//! with what the chosen one prints as it prints it, and a way to stop it.
//!
//! One thread asks the daemon over the control socket (`commands`,
//! `command_output`, CONTROL.md): often while something runs, rarely while
//! nothing does, and at once when woken. It reports only what changed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Summary;
use crate::control::{self, ControlRequest};

/// How often to look while a command runs, and while none does.
const BUSY: Duration = Duration::from_millis(750);
const IDLE: Duration = Duration::from_secs(3);
/// Bytes of the chosen command's latest output kept to show.
pub const KEPT: usize = 64 * 1024;
/// Pieces of output read in one look, at most.
const PIECES: usize = 4;

/// A command as the windows list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    #[serde(flatten)]
    pub summary: Summary,
    /// The folder's label.
    #[serde(default)]
    pub folder: String,
}

impl Listed {
    pub fn id(&self) -> &str {
        &self.summary.status.id
    }

    pub fn running(&self) -> bool {
        self.summary.status.running()
    }

    /// How it stands, in a few words: "running", "exit 0", "stopped",
    /// "CPU limit"...
    pub fn state_label(&self) -> String {
        let status = &self.summary.status;
        match (
            status.limit.as_deref(),
            status.state.as_str(),
            status.exit_code,
        ) {
            (Some("cpu_time"), ..) => "CPU limit".into(),
            (Some("processes"), ..) => "process limit".into(),
            (_, "exited", Some(code)) => format!("exit {code}"),
            (_, "timed_out", _) => "timed out".into(),
            (_, state, _) => state.replace('_', " "),
        }
    }

    /// Whether it ended well: exited with 0.
    pub fn succeeded(&self) -> bool {
        self.summary.status.state == "exited" && self.summary.status.exit_code == Some(0)
    }

    /// Where it runs: the folder's label, and the folder inside it.
    pub fn place(&self) -> String {
        if self.summary.dir.is_empty() {
            self.folder.clone()
        } else {
            format!("{} › {}", self.folder, self.summary.dir)
        }
    }
}

/// What the chosen command printed: the latest [`KEPT`] bytes of both
/// streams, as they came.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Printed {
    pub id: String,
    pub text: String,
    /// Bytes left out before `text`.
    pub cut: u64,
    /// It ended and everything it printed is here.
    pub done: bool,
}

/// What the windows show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Running first, then the latest to end.
    pub commands: Vec<Listed>,
    pub printed: Option<Printed>,
}

impl Snapshot {
    pub fn running(&self) -> usize {
        self.commands.iter().filter(|c| c.running()).count()
    }
}

struct Shared {
    focus: Mutex<Option<String>>,
    stop: AtomicBool,
    woken: Mutex<bool>,
    bell: Condvar,
}

impl Shared {
    fn wake(&self) {
        *self.woken.lock().expect("woken") = true;
        self.bell.notify_all();
    }

    fn sleep(&self, how_long: Duration) {
        let woken = self.woken.lock().expect("woken");
        let (mut woken, _) = self
            .bell
            .wait_timeout_while(woken, how_long, |w| !*w)
            .expect("woken");
        *woken = false;
    }
}

/// Follows the commands until dropped.
pub struct Follow {
    shared: Arc<Shared>,
    socket: PathBuf,
}

impl Follow {
    /// Start following on the daemon at `socket`. `on_change` hears each
    /// new snapshot, from the thread.
    pub fn spawn(socket: PathBuf, on_change: impl Fn(Snapshot) + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            focus: Mutex::new(None),
            stop: AtomicBool::new(false),
            woken: Mutex::new(false),
            bell: Condvar::new(),
        });
        {
            let (shared, socket) = (shared.clone(), socket.clone());
            let _ = std::thread::Builder::new()
                .name("cww-commands".into())
                .spawn(move || follow(&socket, &shared, &on_change));
        }
        Self { shared, socket }
    }

    /// Choose the command whose output to follow, or none.
    pub fn focus(&self, run: Option<String>) {
        *self.shared.focus.lock().expect("focus") = run;
        self.shared.wake();
    }

    /// Look again now (a command may have started).
    pub fn wake(&self) {
        self.shared.wake();
    }

    /// Stop a command, and look again. Blocks for one call to the daemon.
    pub fn stop_run(&self, run: &str) -> Result<(), String> {
        let result = control::request(
            &self.socket,
            ControlRequest::CommandStop { run: run.into() },
        );
        self.shared.wake();
        match result {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err("The Local Agent isn't running.".into()),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake();
    }
}

fn follow(socket: &std::path::Path, shared: &Shared, on_change: &dyn Fn(Snapshot)) {
    let mut last = Snapshot::default();
    let mut printed: Option<(Printed, u64)> = None;
    while !shared.stop.load(Ordering::SeqCst) {
        let commands = list(socket);
        let focus = shared.focus.lock().expect("focus").clone();
        if printed.as_ref().map(|(p, _)| Some(&p.id)) != Some(focus.as_ref()) {
            printed = focus.map(|id| {
                (
                    Printed {
                        id,
                        ..Printed::default()
                    },
                    0,
                )
            });
        }
        if let Some((shown, offset)) = &mut printed
            && !shown.done
        {
            read_more(socket, shown, offset);
        }
        let snapshot = Snapshot {
            commands,
            printed: printed.as_ref().map(|(p, _)| p.clone()),
        };
        if snapshot != last {
            on_change(snapshot.clone());
            last = snapshot;
        }
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        shared.sleep(if last.running() > 0 { BUSY } else { IDLE });
    }
}

fn list(socket: &std::path::Path) -> Vec<Listed> {
    match control::request(socket, ControlRequest::Commands) {
        Ok(Some(value)) => serde_json::from_value(value["commands"].clone()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Read what the chosen command printed since `offset`, keeping the latest
/// [`KEPT`] bytes.
fn read_more(socket: &std::path::Path, shown: &mut Printed, offset: &mut u64) {
    for _ in 0..PIECES {
        let request = ControlRequest::CommandOutput {
            run: shown.id.clone(),
            offset: *offset,
            max: None,
        };
        let Ok(Some(value)) = control::request(socket, request) else {
            // Forgotten, or no daemon: nothing more will come.
            shown.done = true;
            return;
        };
        let Ok(piece) = serde_json::from_value::<super::Output>(value) else {
            shown.done = true;
            return;
        };
        if piece.skipped > 0 {
            shown.cut += piece.skipped;
        }
        shown.text.push_str(&piece.text);
        *offset = piece.next_offset;
        trim(shown);
        if piece.done {
            shown.done = true;
            return;
        }
        if piece.text.is_empty() {
            return;
        }
    }
}

/// Keep the latest [`KEPT`] bytes, cut at a line where one is near.
fn trim(shown: &mut Printed) {
    if shown.text.len() <= KEPT {
        return;
    }
    let mut start = shown.text.len() - KEPT;
    while !shown.text.is_char_boundary(start) {
        start += 1;
    }
    if let Some(newline) = shown.text[start..].find('\n')
        && newline < 1024
    {
        start += newline + 1;
    }
    shown.cut += start as u64;
    shown.text.drain(..start);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(state: &str, exit_code: Option<i32>, limit: Option<&str>) -> Listed {
        Listed {
            summary: Summary {
                status: super::super::Status {
                    id: "0123456789ab".into(),
                    state: state.into(),
                    exit_code,
                    duration_ms: 0,
                    output_bytes: 0,
                    error: None,
                    limit: limit.map(String::from),
                },
                root: "app".into(),
                dir: String::new(),
                command: "make".into(),
                network: false,
                started_at: 1,
            },
            folder: "App".into(),
        }
    }

    #[test]
    fn says_how_a_command_stands_in_a_few_words() {
        assert_eq!(listed("running", None, None).state_label(), "running");
        assert_eq!(listed("exited", Some(2), None).state_label(), "exit 2");
        assert_eq!(listed("timed_out", None, None).state_label(), "timed out");
        assert_eq!(
            listed("exited", Some(137), Some("cpu_time")).state_label(),
            "CPU limit"
        );
        assert!(listed("exited", Some(0), None).succeeded());
        assert!(!listed("stopped", None, None).succeeded());
        let mut inside = listed("running", None, None);
        assert_eq!(inside.place(), "App");
        inside.summary.dir = "api".into();
        assert_eq!(inside.place(), "App › api");
    }

    #[test]
    fn keeps_the_latest_output_from_a_line() {
        let text: String = (0..10_000).map(|i| format!("line {i:05}\n")).collect();
        let mut shown = Printed {
            text: text.clone(),
            ..Printed::default()
        };
        trim(&mut shown);
        assert!(shown.text.len() <= KEPT);
        assert!(shown.text.starts_with("line "), "{}", &shown.text[..20]);
        assert!(shown.text.ends_with("line 09999\n"));
        assert_eq!(shown.cut as usize + shown.text.len(), text.len());
    }

    #[test]
    fn follows_nothing_without_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let follow = Follow::spawn(dir.path().join("none.sock"), move |s| {
            let _ = tx.send(s);
        });
        follow.focus(Some("0123456789ab".into()));
        let snapshot = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(snapshot.commands.is_empty());
        assert!(snapshot.printed.unwrap().done);
        assert!(follow.stop_run("0123456789ab").is_err());
    }
}
