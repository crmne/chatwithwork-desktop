//! Owns the native helper outside the daemon's filesystem sandbox.
//!
//! Callers supply a strictly increasing `Show.attempt` (starting at 1) and
//! retain it with their pending delivery. Only a matching `Shown` acknowledges
//! that attempt. Neither queue admission nor `Ready` acknowledges delivery.
//! On helper failure, pending shows fail and are never replayed; a later
//! command may start a fresh helper. Already delivered OS history remains.
//!
//! The executable must be an absolute installed helper path. No shell, PATH
//! search, public socket or content logging is used. Both channels, frame
//! buffers and retained observations are bounded. Withdrawal is best effort:
//! it immediately disables routing, without claiming the OS removed anything.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use super::protocol::{self, Command, Event};

const COMMAND_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 128;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const RESTART_DELAY: Duration = Duration::from_secs(1);

type Outcome<T> = Result<T, &'static str>;

/// Keep draining events while submitting commands. Closing the event receiver,
/// cancelling `shutdown`, sending `Exit` or closing all command senders stops
/// and reaps the child. The owner should await `task` before exiting.
pub struct Supervisor {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub task: JoinHandle<()>,
}

pub fn spawn(executable: PathBuf, shutdown: CancellationToken) -> Supervisor {
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (sender, events) = mpsc::channel(EVENT_CAPACITY);
    let task = tokio::spawn(async move {
        let mut manager = Manager {
            executable,
            session: None,
            state: State::default(),
            restart_after: Instant::now(),
            events: sender,
        };
        let consumer = manager.events.clone();
        tokio::select! {
            _ = shutdown.cancelled() => {},
            _ = consumer.closed() => {},
            _ = manager.run(receiver) => {},
        }
        if let Some(session) = manager.session.take() {
            session.stop().await;
        }
    });
    Supervisor {
        commands,
        events,
        task,
    }
}

struct Active {
    attempt: u64,
    awaiting: Option<Instant>,
}

#[derive(Default)]
struct State {
    active: HashMap<String, Active>,
    last_attempt: u64,
    // Some(None) is an explicit OS prompt, which can remain open indefinitely.
    permission: Option<Option<Instant>>,
}

impl State {
    fn validate(&mut self, command: &Command) -> Outcome<()> {
        command.validate()?;
        match command {
            Command::Show { id, attempt, .. } => {
                let attempt = attempt.ok_or("Missing notification attempt")?;
                if attempt <= self.last_attempt {
                    return Err("Notification attempts must increase");
                }
                self.last_attempt = attempt;
                if !self.active.contains_key(id) && self.active.len() >= protocol::MAX_ACTIVE {
                    return Err("Too many active notifications");
                }
            }
            Command::Permission | Command::RequestPermission if self.permission.is_some() => {
                return Err("Notification permission query already in progress");
            }
            _ => {}
        }
        Ok(())
    }

    fn sent(&mut self, command: &Command) {
        match command {
            Command::Show {
                id,
                attempt: Some(attempt),
                ..
            } => {
                self.active.insert(
                    id.clone(),
                    Active {
                        attempt: *attempt,
                        awaiting: Some(Instant::now() + IO_TIMEOUT),
                    },
                );
            }
            Command::Withdraw { id } => {
                self.active.remove(id);
            }
            Command::Permission => {
                self.permission = Some(Some(Instant::now() + IO_TIMEOUT));
            }
            Command::RequestPermission => {
                self.permission = Some(None);
            }
            _ => {}
        }
    }

    fn accept(&mut self, event: &Event) -> Outcome<bool> {
        match event {
            Event::Ready { .. } => Err("Unexpected notification handshake"),
            Event::Permission { .. } => Ok(self.permission.take().is_some()),
            Event::Failed {
                id: None,
                attempt: None,
                ..
            } => {
                self.permission = None;
                Ok(true)
            }
            Event::Failed { id: None, .. } => Err("Invalid notification event"),
            Event::Shown { id, attempt }
            | Event::Activated { id, attempt, .. }
            | Event::Dismissed { id, attempt }
            | Event::Closed { id, attempt }
            | Event::Failed {
                id: Some(id),
                attempt,
                ..
            } => {
                protocol::validate_id(id)?;
                let Some(active) = self.active.get_mut(id) else {
                    return Ok(false);
                };
                if *attempt != Some(active.attempt) {
                    return Ok(false);
                }
                if matches!(event, Event::Shown { .. }) {
                    // Native implementations may repeat an acceptance callback.
                    Ok(active.awaiting.take().is_some())
                } else {
                    self.active.remove(id);
                    Ok(true)
                }
            }
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.active
            .values()
            .filter_map(|active| active.awaiting)
            .chain(self.permission.flatten())
            .min()
    }

    fn fail(&mut self, message: &'static str) -> Vec<Event> {
        let mut events: Vec<_> = self
            .active
            .drain()
            .filter_map(|(id, active)| {
                active
                    .awaiting
                    .map(|_| failure(Some(id), Some(active.attempt), message))
            })
            .collect();
        self.permission = None;
        events.push(failure(None, None, message));
        events
    }
}

struct Manager {
    executable: PathBuf,
    session: Option<Session>,
    state: State,
    restart_after: Instant,
    events: mpsc::Sender<Event>,
}

impl Manager {
    async fn emit(&self, event: Event) -> Outcome<()> {
        timeout(IO_TIMEOUT, self.events.send(event))
            .await
            .map_err(|_| "Notification consumer stalled")?
            .map_err(|_| "Notification consumer stopped")
    }

    async fn fault(&mut self, message: &'static str) -> Outcome<()> {
        if let Some(session) = self.session.take() {
            session.stop().await;
        }
        self.restart_after = Instant::now() + RESTART_DELAY;
        for event in self.state.fail(message) {
            self.emit(event).await?;
        }
        Ok(())
    }

    async fn command(&mut self, command: Command) -> Outcome<()> {
        if let Err(message) = self.state.validate(&command) {
            return self.emit(command_failure(&command, message)).await;
        }
        if self.session.is_none() {
            if Instant::now() < self.restart_after {
                return self
                    .emit(command_failure(&command, "Notification helper restarting"))
                    .await;
            }
            match Session::start(&self.executable).await {
                Ok(session) => {
                    self.session = Some(session);
                    self.emit(Event::Ready {
                        protocol: protocol::VERSION,
                    })
                    .await?;
                }
                Err(message) => {
                    self.restart_after = Instant::now() + RESTART_DELAY;
                    return self.emit(command_failure(&command, message)).await;
                }
            }
        }
        self.state.sent(&command);
        let result = self
            .session
            .as_mut()
            .expect("started helper")
            .send(&command)
            .await;
        if let Err(message) = result {
            self.fault(message).await?;
        }
        Ok(())
    }

    async fn run(&mut self, mut commands: mpsc::Receiver<Command>) -> Outcome<()> {
        loop {
            let deadline = self.state.deadline();
            tokio::select! {
                _ = async { tokio::time::sleep_until(deadline.expect("guarded deadline")).await }, if deadline.is_some() => {
                    self.fault("Notification helper timed out").await?;
                }
                command = commands.recv() => {
                    match command {
                        None | Some(Command::Exit) => return Ok(()),
                        Some(command) => self.command(command).await?,
                    }
                }
                event = async { self.session.as_mut().expect("guarded session").reader.next().await }, if self.session.is_some() => {
                    match event {
                        Ok(event) => match self.state.accept(&event) {
                            Ok(true) => self.emit(event).await?,
                            Ok(false) => {},
                            Err(message) => self.fault(message).await?,
                        },
                        Err(message) => self.fault(message).await?,
                    }
                }
            }
        }
    }
}

struct Session {
    child: Child,
    input: ChildStdin,
    reader: Frames<BufReader<ChildStdout>>,
}

impl Session {
    async fn start(executable: &std::path::Path) -> Outcome<Self> {
        if !executable.is_absolute() {
            return Err("Notification helper path must be absolute");
        }
        let mut command = tokio::process::Command::new(executable);
        command
            .arg("--notification-broker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command
            .spawn()
            .map_err(|_| "Cannot start notification helper")?;
        let input = child.stdin.take().ok_or("Missing notification input")?;
        let output = child.stdout.take().ok_or("Missing notification output")?;
        let mut session = Self {
            child,
            input,
            reader: Frames::new(BufReader::new(output)),
        };
        let ready = timeout(IO_TIMEOUT, session.reader.next()).await;
        match ready {
            Ok(Ok(Event::Ready {
                protocol: protocol::VERSION,
            })) => Ok(session),
            _ => {
                session.stop().await;
                Err("Notification helper handshake failed")
            }
        }
    }

    async fn send(&mut self, command: &Command) -> Outcome<()> {
        let mut frame = serde_json::to_vec(command).map_err(|_| "Invalid notification command")?;
        frame.push(b'\n');
        if frame.len() > protocol::MAX_FRAME_BYTES {
            return Err("Notification command too large");
        }
        timeout(IO_TIMEOUT, self.input.write_all(&frame))
            .await
            .map_err(|_| "Notification helper input timed out")?
            .map_err(|_| "Notification helper input closed")
    }

    async fn stop(self) {
        let Self {
            mut child,
            input,
            reader: _reader,
        } = self;
        // EOF releases observation state without removing delivered OS history.
        drop(input);
        if timeout(Duration::from_secs(1), child.wait()).await.is_err() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

/// Partial frames live in the reader, not a cancelled `next` future, because
/// the main select loop can handle a command while a frame is incomplete.
struct Frames<R> {
    reader: R,
    frame: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin> Frames<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            frame: Vec::new(),
        }
    }

    async fn next(&mut self) -> Outcome<Event> {
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .map_err(|_| "Cannot read notification event")?;
            if available.is_empty() {
                return Err("Notification helper output closed");
            }
            let newline = available.iter().position(|&byte| byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if self.frame.len() + count > protocol::MAX_FRAME_BYTES {
                return Err("Notification event too large");
            }
            self.frame.reserve_exact(count);
            self.frame.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                let event: Event = serde_json::from_slice(&self.frame)
                    .map_err(|_| "Invalid notification event")?;
                self.frame.clear();
                if let Event::Activated {
                    action: Some(action),
                    ..
                } = &event
                {
                    protocol::validate_id(action)?;
                }
                if let Event::Failed { message, .. } = &event
                    && (message.len() > protocol::MAX_TITLE_BYTES
                        || message.chars().any(char::is_control))
                {
                    return Err("Invalid notification error");
                }
                return Ok(event);
            }
        }
    }
}

fn command_failure(command: &Command, message: &'static str) -> Event {
    failure(
        command.id().map(str::to_owned),
        match command {
            Command::Show { attempt, .. } => *attempt,
            _ => None,
        },
        message,
    )
}

fn failure(id: Option<String>, attempt: Option<u64>, message: &'static str) -> Event {
    Event::Failed {
        id,
        attempt,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::PermissionState;
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn show(id: &str, attempt: u64) -> Command {
        Command::Show {
            id: id.into(),
            attempt: Some(attempt),
            title: "Synthetic notice".into(),
            body: None,
        }
    }

    fn sent(state: &mut State, command: &Command) {
        state.validate(command).unwrap();
        state.sent(command);
    }

    #[test]
    fn buffered_callbacks_cannot_acknowledge_or_remove_a_replacement() {
        let mut state = State::default();
        sent(&mut state, &show("same", 1));
        sent(&mut state, &show("same", 2));
        for stale in [
            Event::Shown {
                id: "same".into(),
                attempt: Some(1),
            },
            Event::Shown {
                id: "same".into(),
                attempt: None,
            },
            Event::Dismissed {
                id: "same".into(),
                attempt: Some(1),
            },
            failure(Some("same".into()), Some(1), "Old failure"),
        ] {
            assert!(!state.accept(&stale).unwrap());
            assert!(state.active["same"].awaiting.is_some());
        }
        let shown = Event::Shown {
            id: "same".into(),
            attempt: Some(2),
        };
        assert!(state.accept(&shown).unwrap());
        assert!(!state.accept(&shown).unwrap());
        assert!(state.deadline().is_none());
        assert!(
            state
                .accept(&Event::Activated {
                    id: "same".into(),
                    attempt: Some(2),
                    action: None
                })
                .unwrap()
        );
        assert!(state.active.is_empty());
    }

    #[test]
    fn only_pending_shows_fail_on_disconnect_and_attempts_survive_restart() {
        let mut state = State::default();
        sent(&mut state, &show("delivered", 1));
        assert!(
            state
                .accept(&Event::Shown {
                    id: "delivered".into(),
                    attempt: Some(1)
                })
                .unwrap()
        );
        sent(&mut state, &show("pending", 2));
        let failed = state.fail("Helper stopped");
        assert_eq!(
            failed,
            vec![
                failure(Some("pending".into()), Some(2), "Helper stopped"),
                failure(None, None, "Helper stopped"),
            ]
        );
        assert!(state.active.is_empty());
        assert!(state.validate(&show("reused", 2)).is_err());
        sent(&mut state, &show("new", 3));
    }

    #[test]
    fn capacity_and_permission_are_bounded_and_withdrawal_disables_click_routing() {
        let mut state = State::default();
        for attempt in 1..=protocol::MAX_ACTIVE as u64 {
            sent(&mut state, &show(&attempt.to_string(), attempt));
        }
        assert!(state.validate(&show("overflow", 40)).is_err());
        sent(&mut state, &show("1", 41));
        sent(&mut state, &Command::Withdraw { id: "1".into() });
        assert!(
            !state
                .accept(&Event::Activated {
                    id: "1".into(),
                    attempt: Some(41),
                    action: None
                })
                .unwrap()
        );
        sent(&mut state, &show("new", 42));
        sent(&mut state, &Command::RequestPermission);
        assert!(state.validate(&Command::Permission).is_err());
        assert!(
            state
                .accept(&Event::Permission {
                    state: PermissionState::Granted
                })
                .unwrap()
        );
        assert!(
            !state
                .accept(&Event::Permission {
                    state: PermissionState::Denied
                })
                .unwrap()
        );
    }

    #[test]
    fn explicit_permission_prompt_has_no_automatic_deadline() {
        let mut state = State::default();
        sent(&mut state, &Command::RequestPermission);
        assert!(state.deadline().is_none());
        state
            .accept(&Event::Permission {
                state: PermissionState::Granted,
            })
            .unwrap();
        sent(&mut state, &Command::Permission);
        assert!(state.deadline().is_some());
    }

    #[tokio::test]
    async fn interrupted_reads_preserve_partial_frames() {
        let (mut write, read) = tokio::io::duplex(256);
        let mut frames = Frames::new(BufReader::new(read));
        write.write_all(b"{\"event\":\"shown\",").await.unwrap();
        assert!(
            timeout(Duration::from_millis(10), frames.next())
                .await
                .is_err()
        );
        write
            .write_all(
                b"\"id\":\"n\",\"attempt\":2}\n{\"event\":\"permission\",\"state\":\"granted\"}\n",
            )
            .await
            .unwrap();
        assert_eq!(
            frames.next().await.unwrap(),
            Event::Shown {
                id: "n".into(),
                attempt: Some(2)
            }
        );
        assert_eq!(
            frames.next().await.unwrap(),
            Event::Permission {
                state: PermissionState::Granted
            }
        );
    }

    #[tokio::test]
    async fn frames_are_bounded_and_truncated_or_malformed_events_fail() {
        for bytes in [
            vec![b'x'; protocol::MAX_FRAME_BYTES + 1],
            b"{\"event\":\"permission\"}\n".to_vec(),
            b"{\"event\":\"ready\",\"protocol\":2}".to_vec(),
        ] {
            let mut frames = Frames::new(BufReader::with_capacity(7, bytes.as_slice()));
            assert!(frames.next().await.is_err());
            assert!(frames.frame.capacity() <= protocol::MAX_FRAME_BYTES);
        }
    }

    #[cfg(unix)]
    fn helper(script: &str) -> (tempfile::TempDir, PathBuf) {
        // A tiny executable fixture, without a desktop session or native APIs.
        let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let path = dir.path().join("helper");
        crate::test_process::write_script(
            &path,
            &format!("[ \"$1\" = --notification-broker ] || exit 9\n{script}"),
        );
        (dir, path)
    }

    async fn event(supervisor: &mut Supervisor) -> Event {
        timeout(Duration::from_secs(5), supervisor.events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_uses_private_pipes_and_stops_on_owner_cancellation() {
        let (_dir, path) = helper(
            r#"
            printf '%s\n' '{"event":"ready","protocol":2}'
            IFS= read -r command
            printf '%s\n' '{"event":"shown","id":"n","attempt":1}'
            while IFS= read -r command; do :; done
        "#,
        );
        let shutdown = CancellationToken::new();
        let mut supervisor = spawn(path, shutdown.clone());
        supervisor.commands.send(show("n", 1)).await.unwrap();
        assert_eq!(
            event(&mut supervisor).await,
            Event::Ready {
                protocol: protocol::VERSION
            }
        );
        assert_eq!(
            event(&mut supervisor).await,
            Event::Shown {
                id: "n".into(),
                attempt: Some(1)
            }
        );
        shutdown.cancel();
        timeout(Duration::from_secs(2), supervisor.task)
            .await
            .unwrap()
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unacknowledged_delivery_times_out_without_inventing_acceptance() {
        let (_dir, path) = helper(
            r#"
            printf '%s\n' '{"event":"ready","protocol":2}'
            while IFS= read -r command; do :; done
        "#,
        );
        let mut supervisor = spawn(path, CancellationToken::new());
        supervisor.commands.send(show("n", 1)).await.unwrap();
        assert!(matches!(event(&mut supervisor).await, Event::Ready { .. }));
        let failed = timeout(
            IO_TIMEOUT + Duration::from_secs(2),
            supervisor.events.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            failed,
            failure(Some("n".into()), Some(1), "Notification helper timed out")
        );
        supervisor.commands.send(Command::Exit).await.unwrap();
        supervisor.task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_handshake() {
        let (_dir, path) = helper("while IFS= read -r command; do :; done");
        let shutdown = CancellationToken::new();
        let supervisor = spawn(path, shutdown.clone());
        supervisor.commands.send(show("n", 1)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        timeout(Duration::from_secs(2), supervisor.task)
            .await
            .unwrap()
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wrong_version_cannot_accept_a_show() {
        let (_dir, path) = helper("printf '%s\\n' '{\"event\":\"ready\",\"protocol\":1}'");
        let mut supervisor = spawn(path, CancellationToken::new());
        supervisor.commands.send(show("n", 7)).await.unwrap();
        assert_eq!(
            event(&mut supervisor).await,
            failure(
                Some("n".into()),
                Some(7),
                "Notification helper handshake failed"
            )
        );
        supervisor.commands.send(Command::Exit).await.unwrap();
        supervisor.task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn crash_fails_pending_delivery_and_only_new_work_restarts_helper() {
        let (_dir, path) = helper(
            r#"
            printf '%s\n' '{"event":"ready","protocol":2}'
            IFS= read -r command
            exit 1
        "#,
        );
        let mut supervisor = spawn(path, CancellationToken::new());
        supervisor.commands.send(show("n", 1)).await.unwrap();
        assert!(matches!(event(&mut supervisor).await, Event::Ready { .. }));
        assert_eq!(
            event(&mut supervisor).await,
            failure(
                Some("n".into()),
                Some(1),
                "Notification helper output closed"
            )
        );
        assert!(matches!(
            event(&mut supervisor).await,
            Event::Failed { id: None, .. }
        ));
        assert!(
            timeout(
                RESTART_DELAY + Duration::from_millis(30),
                supervisor.events.recv()
            )
            .await
            .is_err()
        );
        supervisor.commands.send(show("new", 2)).await.unwrap();
        assert!(matches!(event(&mut supervisor).await, Event::Ready { .. }));
        assert_eq!(
            event(&mut supervisor).await,
            failure(
                Some("new".into()),
                Some(2),
                "Notification helper output closed"
            )
        );
        supervisor.commands.send(Command::Exit).await.unwrap();
        supervisor.task.await.unwrap();
    }

    #[tokio::test]
    async fn relative_helper_path_never_searches_path() {
        let mut supervisor = spawn("cww-app".into(), CancellationToken::new());
        supervisor.commands.send(show("n", 1)).await.unwrap();
        assert_eq!(
            event(&mut supervisor).await,
            failure(
                Some("n".into()),
                Some(1),
                "Notification helper path must be absolute"
            )
        );
        supervisor.commands.send(Command::Exit).await.unwrap();
        supervisor.task.await.unwrap();
    }
}
