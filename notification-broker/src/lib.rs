//! Pipe-only native notification broker. Call `run_broker` on the process main
//! thread before UI, tray or logging initialization. stdout is JSON only.

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, BufRead, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use cww::notifications::protocol::{self, Command, Event, PermissionState};
use fastframe_notifications::{App, Client, Error, Notification, Permission};
use tokio::sync::{mpsc, oneshot};

const APP_ID: &str = "com.chatwithwork.local-agent";
const COMMAND_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 128;

type NativeEvent = fastframe_notifications::Event;
type Callback = Box<dyn Fn(NativeEvent) + Send + Sync>;
type PermissionQuery = Pin<Box<dyn Future<Output = std::result::Result<Permission, Error>> + Send>>;
type CompletionSender = oneshot::Sender<Result<()>>;

/// Run without a window. The main thread pumps macOS's native run loop while
/// bounded pipe input and notification policy run on separate threads.
pub fn run_broker() -> Result<()> {
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    std::thread::Builder::new()
        .name("notification-input".into())
        .spawn(move || read_commands(io::stdin().lock(), commands))
        .context("Cannot start notification input")?;

    let (completion, finished) = Completion::new();
    std::thread::Builder::new()
        .name("notification-broker".into())
        .spawn(move || {
            let output = io::stdout();
            let mut output = output.lock();
            let result = match Native::new() {
                Ok(backend) => {
                    pollster::block_on(serve(backend, receiver, &mut output, completion.clone()))
                }
                Err(error) => {
                    let _ = emit(&mut output, &failure(None, error.to_string()));
                    Err(error.into())
                }
            };
            completion.finish(result);
        })
        .context("Cannot start notification broker")?;

    fastframe_notifications::run_main(async {
        finished.await.context("Notification broker stopped")?
    })
}

/// A full native callback queue must fail the broker instead of silently
/// dropping a delivery ACK or blocking Fastframe's native worker. The parent
/// observes the process failure and retains its normal delivery fallback.
#[derive(Clone)]
struct Completion(Arc<Mutex<Option<CompletionSender>>>);

impl Completion {
    fn new() -> (Self, oneshot::Receiver<Result<()>>) {
        let (sender, receiver) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(sender)))), receiver)
    }

    fn finish(&self, result: Result<()>) {
        if let Some(sender) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sender.send(result);
        }
    }
}

enum Input {
    Command(Command),
    Invalid,
    Failed,
    End,
}

fn read_commands(mut reader: impl BufRead, sender: mpsc::Sender<Input>) {
    loop {
        let input = match read_frame(&mut reader) {
            Ok(Some(frame)) => match serde_json::from_slice(&frame) {
                Ok(command) => Input::Command(command),
                Err(_) => Input::Invalid,
            },
            Ok(None) => Input::End,
            Err(_) => Input::Failed,
        };
        let stop = matches!(
            input,
            Input::End | Input::Failed | Input::Command(Command::Exit)
        );
        if sender.blocking_send(input).is_err() || stop {
            return;
        }
    }
}

/// Unlike `read_line`, this never allocates beyond the wire limit while
/// waiting for a newline. An oversized frame terminates this private channel.
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((!frame.is_empty()).then_some(frame));
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if frame.len() + count > protocol::MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Notification frame too large",
            ));
        }
        // Avoid geometric Vec growth beyond the validated frame budget.
        frame.reserve_exact(count);
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}

trait Backend {
    type Handle;

    fn show(
        &self,
        id: &str,
        title: &str,
        body: Option<&str>,
        callback: Callback,
    ) -> std::result::Result<Self::Handle, Error>;
    fn close(&self, handle: &Self::Handle);
    fn withdraw(&self, id: &str) -> std::result::Result<(), Error>;
    fn permission(&self, request: bool) -> PermissionQuery;
}

struct Native(Client);

impl Native {
    fn new() -> std::result::Result<Self, Error> {
        Client::new(
            App::new(APP_ID, "Chat with Work")
                .desktop_entry("cww-app")
                .icon("cww-app"),
        )
        .map(Self)
    }
}

impl Backend for Native {
    type Handle = fastframe_notifications::Handle;

    fn show(
        &self,
        id: &str,
        title: &str,
        body: Option<&str>,
        callback: Callback,
    ) -> std::result::Result<Self::Handle, Error> {
        self.0.show_with(
            Notification::new(title)
                .body(body.unwrap_or_default())
                .key(id)
                .category("im.received"),
            callback,
        )
    }

    fn close(&self, handle: &Self::Handle) {
        handle.close();
    }

    fn withdraw(&self, id: &str) -> std::result::Result<(), Error> {
        self.0.withdraw(id)
    }

    fn permission(&self, request: bool) -> PermissionQuery {
        let client = self.0.clone();
        Box::pin(async move {
            if request {
                client.request_permission().await
            } else {
                client.permission().await
            }
        })
    }
}

struct Active<H> {
    handle: H,
    generation: u64,
    attempt: Option<u64>,
}

struct Observation {
    id: String,
    generation: u64,
    event: NativeEvent,
}

struct Broker<B: Backend> {
    backend: B,
    active: HashMap<String, Active<B::Handle>>,
    next_generation: u64,
    events: mpsc::Sender<Observation>,
    completion: Completion,
}

impl<B: Backend> Broker<B> {
    fn command(&mut self, command: Command) -> std::result::Result<Option<PermissionQuery>, Event> {
        let attempt = match &command {
            Command::Show { attempt, .. } => *attempt,
            _ => None,
        };
        if let Err(message) = command.validate() {
            return Err(with_attempt(
                failure(command.id().map(str::to_owned), message),
                attempt,
            ));
        }
        match command {
            Command::Show {
                id,
                attempt,
                title,
                body,
            } => {
                if !self.active.contains_key(&id) && self.active.len() >= protocol::MAX_ACTIVE {
                    return Err(with_attempt(
                        failure(Some(id), "Too many active notifications"),
                        attempt,
                    ));
                }
                let Some(generation) = self.next_generation.checked_add(1) else {
                    return Err(with_attempt(
                        failure(Some(id), "Notification broker must restart"),
                        attempt,
                    ));
                };
                self.next_generation = generation;
                let event_id = id.clone();
                let events = self.events.clone();
                let completion = self.completion.clone();
                let handle = self
                    .backend
                    .show(
                        &id,
                        &title,
                        body.as_deref(),
                        Box::new(move |event| {
                            // No named buttons are currently posted. Still bound
                            // native action data before it enters the event queue.
                            let event = match event {
                                NativeEvent::Action(ref action)
                                    if protocol::validate_id(action).is_err() =>
                                {
                                    NativeEvent::Failed(Error::InvalidInput)
                                }
                                event => event,
                            };
                            let observation = Observation {
                                id: event_id.clone(),
                                generation,
                                event,
                            };
                            if matches!(
                                events.try_send(observation),
                                Err(mpsc::error::TrySendError::Full(_))
                            ) {
                                completion.finish(Err(anyhow::anyhow!(
                                    "Notification event queue overflow"
                                )));
                            }
                        }),
                    )
                    .map_err(|error| {
                        with_attempt(failure(Some(id.clone()), error.to_string()), attempt)
                    })?;
                // Fastframe releases the old observation and serializes a
                // same-key replacement. Never close the old handle here.
                self.active.insert(
                    id,
                    Active {
                        handle,
                        generation,
                        attempt,
                    },
                );
            }
            Command::Withdraw { id } => {
                if let Some(active) = self.active.remove(&id) {
                    // Retire immediately, so an oldest-ID withdrawal followed
                    // by a new show can make room at capacity. Fastframe's
                    // queued Close wins over dropping this handle. Removal is
                    // best effort; no native completion is claimed here.
                    self.backend.close(&active.handle);
                } else {
                    // Cross-restart withdrawal is available on macOS/Windows.
                    // The shared API queues it without a completion callback.
                    self.backend
                        .withdraw(&id)
                        .map_err(|error| failure(Some(id), error.to_string()))?;
                }
            }
            Command::Permission => return Ok(Some(self.backend.permission(false))),
            Command::RequestPermission => return Ok(Some(self.backend.permission(true))),
            Command::Exit => {}
        }
        Ok(None)
    }

    fn observation(&mut self, observation: Observation) -> Option<Event> {
        let Observation {
            id,
            generation,
            event,
        } = observation;
        let active = self.active.get(&id)?;
        if active.generation != generation {
            return None;
        }
        let attempt = active.attempt;
        if !matches!(event, NativeEvent::Shown) {
            self.active.remove(&id);
        }
        Some(match event {
            NativeEvent::Shown => Event::Shown { id, attempt },
            NativeEvent::Activated => Event::Activated {
                id,
                attempt,
                action: None,
            },
            NativeEvent::Action(action) => Event::Activated {
                id,
                attempt,
                action: Some(action),
            },
            NativeEvent::Dismissed => Event::Dismissed { id, attempt },
            NativeEvent::Closed => Event::Closed { id, attempt },
            NativeEvent::Failed(error) => {
                with_attempt(failure(Some(id), error.to_string()), attempt)
            }
        })
    }
}

async fn serve<B: Backend>(
    backend: B,
    mut commands: mpsc::Receiver<Input>,
    output: &mut impl Write,
    completion: Completion,
) -> Result<()> {
    let (sender, mut events) = mpsc::channel(EVENT_CAPACITY);
    let mut broker = Broker {
        backend,
        active: HashMap::new(),
        next_generation: 0,
        events: sender,
        completion,
    };
    let mut permission: Option<PermissionQuery> = None;
    emit(
        output,
        &Event::Ready {
            protocol: protocol::VERSION,
        },
    )?;
    loop {
        tokio::select! {
            // Acceptance/failure already queued for a command is sent before
            // reading another command. Each native observation is finite.
            biased;
            Some(observation) = events.recv() => {
                if let Some(event) = broker.observation(observation) {
                    emit(output, &event)?;
                }
            }
            result = async { permission.as_mut().expect("guarded permission query").await }, if permission.is_some() => {
                permission = None;
                emit(output, &match result {
                    Ok(state) => Event::Permission { state: permission_state(state) },
                    Err(error) => failure(None, error.to_string()),
                })?;
            }
            input = commands.recv() => {
                match input {
                    None | Some(Input::End | Input::Command(Command::Exit)) => return Ok(()),
                    Some(Input::Invalid) => emit(output, &failure(None, "Invalid notification command"))?,
                    Some(Input::Failed) => {
                        emit(output, &failure(None, "Cannot read notification command"))?;
                        anyhow::bail!("Notification input failed");
                    }
                    Some(Input::Command(command)) => {
                        if permission.is_some() && matches!(command, Command::Permission | Command::RequestPermission) {
                            emit(output, &failure(None, "Notification permission query already in progress"))?;
                            continue;
                        }
                        match broker.command(command) {
                            Ok(Some(query)) => permission = Some(query),
                            Ok(None) => {}
                            Err(event) => emit(output, &event)?,
                        }
                    }
                }
            }
        }
    }
}

fn permission_state(permission: Permission) -> PermissionState {
    match permission {
        Permission::Granted => PermissionState::Granted,
        Permission::NotDetermined => PermissionState::NotDetermined,
        Permission::Denied => PermissionState::Denied,
        Permission::Unknown => PermissionState::Unknown,
    }
}

fn failure(id: Option<String>, message: impl Into<String>) -> Event {
    Event::Failed {
        id,
        attempt: None,
        message: message.into(),
    }
}

fn with_attempt(mut event: Event, value: Option<u64>) -> Event {
    if let Event::Failed { attempt, .. } = &mut event {
        *attempt = value;
    }
    event
}

fn emit(output: &mut impl Write, event: &Event) -> Result<()> {
    serde_json::to_writer(&mut *output, event).context("Cannot encode notification event")?;
    output
        .write_all(b"\n")
        .context("Cannot write notification event")?;
    output.flush().context("Cannot flush notification event")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Default)]
    struct Fake {
        callbacks: Arc<Mutex<Vec<Callback>>>,
        closed: Arc<Mutex<Vec<usize>>>,
        withdrawn: Arc<Mutex<Vec<String>>>,
        permissions: Arc<Mutex<Vec<bool>>>,
        fail_show: Arc<AtomicBool>,
        pending_permission: bool,
        accept_immediately: bool,
    }

    impl Fake {
        fn send(&self, index: usize, event: NativeEvent) {
            self.callbacks.lock().unwrap()[index](event);
        }
    }

    impl Backend for Fake {
        type Handle = usize;

        fn show(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            callback: Callback,
        ) -> std::result::Result<usize, Error> {
            if self.fail_show.load(Ordering::Relaxed) {
                return Err(Error::Busy);
            }
            let mut callbacks = self.callbacks.lock().unwrap();
            let index = callbacks.len();
            if self.accept_immediately {
                callback(NativeEvent::Shown);
            }
            callbacks.push(callback);
            Ok(index)
        }

        fn close(&self, handle: &usize) {
            self.closed.lock().unwrap().push(*handle);
        }

        fn withdraw(&self, id: &str) -> std::result::Result<(), Error> {
            self.withdrawn.lock().unwrap().push(id.into());
            Ok(())
        }

        fn permission(&self, request: bool) -> PermissionQuery {
            self.permissions.lock().unwrap().push(request);
            if self.pending_permission {
                Box::pin(std::future::pending())
            } else {
                Box::pin(async { Ok(Permission::Granted) })
            }
        }
    }

    fn broker(
        fake: Fake,
    ) -> (
        Broker<Fake>,
        mpsc::Receiver<Observation>,
        oneshot::Receiver<Result<()>>,
    ) {
        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        let (completion, finished) = Completion::new();
        (
            Broker {
                backend: fake,
                active: HashMap::new(),
                next_generation: 0,
                events,
                completion,
            },
            receiver,
            finished,
        )
    }

    fn show(id: &str) -> Command {
        Command::Show {
            id: id.into(),
            attempt: None,
            title: "Synthetic notice".into(),
            body: Some("Synthetic body".into()),
        }
    }

    #[test]
    fn shown_is_not_emitted_until_native_acceptance_and_failures_release_capacity() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        assert!(broker.command(show("a")).unwrap().is_none());
        assert!(events.try_recv().is_err());
        assert!(fake.permissions.lock().unwrap().is_empty());
        fake.send(0, NativeEvent::Shown);
        assert_eq!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Shown {
                id: "a".into(),
                attempt: None
            })
        );
        assert_eq!(broker.active.len(), 1);
        fake.send(0, NativeEvent::Failed(Error::PermissionDenied));
        assert!(
            matches!(broker.observation(events.try_recv().unwrap()), Some(Event::Failed { id: Some(id), .. }) if id == "a")
        );
        assert!(broker.active.is_empty());
        fake.fail_show.store(true, Ordering::Relaxed);
        assert!(
            matches!(broker.command(show("b")), Err(Event::Failed { id: Some(id), .. }) if id == "b")
        );
        assert!(broker.active.is_empty());
    }

    #[test]
    fn replacement_ignores_stale_callbacks_without_closing_the_new_notice() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        assert!(broker.command(show("a")).is_ok());
        assert!(broker.command(show("a")).is_ok());
        for event in [
            NativeEvent::Shown,
            NativeEvent::Activated,
            NativeEvent::Closed,
            NativeEvent::Failed(Error::Delivery),
        ] {
            fake.send(0, event);
            assert_eq!(broker.observation(events.try_recv().unwrap()), None);
            assert_eq!(broker.active.len(), 1);
        }
        assert!(fake.closed.lock().unwrap().is_empty());
        fake.send(1, NativeEvent::Shown);
        assert_eq!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Shown {
                id: "a".into(),
                attempt: None
            })
        );
        fake.send(1, NativeEvent::Action("open".into()));
        assert_eq!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Activated {
                id: "a".into(),
                attempt: None,
                action: Some("open".into())
            })
        );
        assert!(broker.active.is_empty());
    }

    #[test]
    fn withdrawal_retires_the_observation_without_a_fake_native_completion() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        assert!(broker.command(show("a")).is_ok());
        assert!(broker.command(Command::Withdraw { id: "a".into() }).is_ok());
        assert_eq!(*fake.closed.lock().unwrap(), vec![0]);
        assert!(broker.active.is_empty());
        assert!(events.try_recv().is_err());
        for late in [
            NativeEvent::Shown,
            NativeEvent::Closed,
            NativeEvent::Dismissed,
            NativeEvent::Activated,
            NativeEvent::Failed(Error::Delivery),
        ] {
            fake.send(0, late);
            assert_eq!(broker.observation(events.try_recv().unwrap()), None);
        }
    }

    #[test]
    fn attempt_is_echoed_on_acceptance_actions_and_rejected_replacements() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        let mut first = show("a");
        if let Command::Show { attempt, .. } = &mut first {
            *attempt = Some(7);
        }
        assert!(broker.command(first).is_ok());
        fake.send(0, NativeEvent::Shown);
        assert_eq!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Shown {
                id: "a".into(),
                attempt: Some(7)
            })
        );
        let mut replacement = show("a");
        if let Command::Show { attempt, .. } = &mut replacement {
            *attempt = Some(8);
        }
        fake.fail_show.store(true, Ordering::Relaxed);
        assert!(matches!(
            broker.command(replacement),
            Err(Event::Failed {
                attempt: Some(8),
                ..
            })
        ));
        fake.send(0, NativeEvent::Activated);
        assert_eq!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Activated {
                id: "a".into(),
                attempt: Some(7),
                action: None
            })
        );
    }

    #[test]
    fn unknown_withdrawal_does_not_claim_native_removal() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        assert!(
            broker
                .command(Command::Withdraw {
                    id: "old-process".into()
                })
                .unwrap()
                .is_none()
        );
        assert_eq!(*fake.withdrawn.lock().unwrap(), vec!["old-process"]);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn retained_state_is_bounded_and_replacement_still_works_at_capacity() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        for id in 0..protocol::MAX_ACTIVE {
            assert!(broker.command(show(&id.to_string())).is_ok());
        }
        assert!(matches!(
            broker.command(show("overflow")),
            Err(Event::Failed { .. })
        ));
        assert!(broker.command(show("0")).is_ok());
        assert_eq!(broker.active.len(), protocol::MAX_ACTIVE);
        fake.send(1, NativeEvent::Dismissed);
        assert!(matches!(
            broker.observation(events.try_recv().unwrap()),
            Some(Event::Dismissed { .. })
        ));
        assert!(broker.command(show("new")).is_ok());
        assert_eq!(broker.active.len(), protocol::MAX_ACTIVE);
        // Eviction must make room before an asynchronous native close returns.
        assert!(broker.command(Command::Withdraw { id: "0".into() }).is_ok());
        assert!(broker.command(show("next")).is_ok());
        assert_eq!(broker.active.len(), protocol::MAX_ACTIVE);
        fake.send(protocol::MAX_ACTIVE, NativeEvent::Closed);
        assert_eq!(broker.observation(events.try_recv().unwrap()), None);
        assert_eq!(broker.active.len(), protocol::MAX_ACTIVE);
    }

    #[test]
    fn native_event_overflow_fails_instead_of_losing_an_ack_silently() {
        let fake = Fake::default();
        let (mut broker, _events, mut finished) = broker(fake.clone());
        assert!(broker.command(show("a")).is_ok());
        for _ in 0..=EVENT_CAPACITY {
            fake.send(0, NativeEvent::Shown);
        }
        assert!(finished.try_recv().unwrap().is_err());
    }

    #[test]
    fn oversized_native_action_is_not_retained_or_echoed() {
        let fake = Fake::default();
        let (mut broker, mut events, _) = broker(fake.clone());
        assert!(broker.command(show("a")).is_ok());
        fake.send(0, NativeEvent::Action("private".repeat(1000)));
        let event = broker.observation(events.try_recv().unwrap()).unwrap();
        assert_eq!(
            event,
            failure(Some("a".into()), "invalid notification configuration")
        );
    }

    #[test]
    fn frame_reader_bounds_unterminated_input_and_preserves_framing() {
        let mut reader = BufReader::with_capacity(
            7,
            Cursor::new(b"{\"command\":\"permission\"}\n{\"command\":\"exit\"}"),
        );
        assert_eq!(
            serde_json::from_slice::<Command>(&read_frame(&mut reader).unwrap().unwrap()).unwrap(),
            Command::Permission
        );
        assert_eq!(
            serde_json::from_slice::<Command>(&read_frame(&mut reader).unwrap().unwrap()).unwrap(),
            Command::Exit
        );
        assert_eq!(read_frame(&mut reader).unwrap(), None);
        let mut oversized =
            BufReader::with_capacity(11, Cursor::new(vec![b'x'; protocol::MAX_FRAME_BYTES + 1]));
        assert_eq!(
            read_frame(&mut oversized).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn exercise(fake: Fake, inputs: Vec<Input>) -> Vec<Event> {
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        for input in inputs {
            sender.try_send(input).ok().unwrap();
        }
        drop(sender);
        let (completion, _finished) = Completion::new();
        let mut output = Vec::new();
        pollster::block_on(serve(fake, receiver, &mut output, completion)).unwrap();
        output
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect()
    }

    #[test]
    fn json_output_and_permission_requests_are_explicit() {
        let fake = Fake {
            accept_immediately: true,
            ..Fake::default()
        };
        let events = exercise(
            fake.clone(),
            vec![
                Input::Invalid,
                Input::Command(show("a")),
                Input::Command(Command::Permission),
                Input::Command(Command::RequestPermission),
                Input::Command(Command::Exit),
            ],
        );
        assert_eq!(
            events,
            vec![
                Event::Ready {
                    protocol: protocol::VERSION
                },
                failure(None, "Invalid notification command"),
                Event::Shown {
                    id: "a".into(),
                    attempt: None
                },
                Event::Permission {
                    state: PermissionState::Granted
                },
                Event::Permission {
                    state: PermissionState::Granted
                },
            ]
        );
        assert_eq!(*fake.permissions.lock().unwrap(), vec![false, true]);
        assert!(fake.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn pending_permission_does_not_block_delivery_withdrawal_or_exit() {
        let fake = Fake {
            pending_permission: true,
            accept_immediately: true,
            ..Fake::default()
        };
        let events = exercise(
            fake.clone(),
            vec![
                Input::Command(Command::RequestPermission),
                Input::Command(Command::Permission),
                Input::Command(show("a")),
                Input::Command(Command::Withdraw { id: "a".into() }),
                Input::Command(Command::Exit),
            ],
        );
        assert!(events.contains(&Event::Shown {
            id: "a".into(),
            attempt: None
        }));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Failed { id: None, .. }))
        );
        assert_eq!(*fake.permissions.lock().unwrap(), vec![true]);
        assert_eq!(*fake.closed.lock().unwrap(), vec![0]);
    }

    #[test]
    fn invalid_content_never_reaches_backend_and_output_failure_stops_service() {
        let fake = Fake::default();
        let events = exercise(
            fake.clone(),
            vec![Input::Command(Command::Show {
                id: "a".into(),
                attempt: None,
                title: "private".repeat(100),
                body: None,
            })],
        );
        assert_eq!(
            events[1],
            failure(Some("a".into()), "Invalid notification title")
        );
        assert!(fake.callbacks.lock().unwrap().is_empty());
        let (_sender, receiver) = mpsc::channel(1);
        let (completion, _finished) = Completion::new();
        // A zero-capacity slice reports WriteZero instead of accepting bytes.
        assert!(pollster::block_on(serve(fake, receiver, &mut &mut [][..], completion)).is_err());
    }
}
