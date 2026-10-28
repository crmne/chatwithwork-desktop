//! Account-scoped notification delivery outside the file sandbox.
//!
//! Server offers, native observations and foreground views are separate. A
//! receipt is emitted only for a current offer's actual native acceptance.
//! Connections and worker restarts clear routing; nothing is replayed.

mod pipe;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::auth::client::ServerUrl;
use crate::notifications::{
    protocol::{self, Command, Event, PermissionState},
    supervisor,
};

#[cfg(unix)]
pub use pipe::Inherited;
pub use pipe::serve as serve_worker;

const CAPACITY: usize = 128;
const VIEW_TTL: Duration = Duration::from_secs(75);
const HEARTBEAT: Duration = Duration::from_secs(30);
const MAX_VIEWS: usize = 32;
/// How long the server gives an offer before falling back to the browser.
const OFFER_WINDOW: Duration = Duration::from_secs(15);

/// Returned by `notification_status`. Permission never implies acceptance.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub available: bool,
    pub permission: Permission,
    pub connected: bool,
    pub account: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            available: false,
            permission: Permission::Unavailable,
            connected: false,
            account: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Granted,
    NotDetermined,
    Denied,
    Unknown,
    Unavailable,
}

impl From<PermissionState> for Permission {
    fn from(value: PermissionState) -> Self {
        match value {
            PermissionState::Granted => Self::Granted,
            PermissionState::NotDetermined => Self::NotDetermined,
            PermissionState::Denied => Self::Denied,
            PermissionState::Unknown => Self::Unknown,
        }
    }
}

/// Trusted routing metadata, delivered only after a current native click.
/// A desktop launcher can replace the browser callback without accepting
/// executable paths or URLs from the helper or the server's notification.
pub struct Activation {
    pub server: ServerUrl,
    pub account: String,
    pub chat: u64,
}

impl Activation {
    pub fn browser_url(&self) -> String {
        self.server
            .endpoint(&format!("/{}/chats/{}", self.account, self.chat))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Input {
    Attach {
        generation: u64,
        server: String,
        device_id: String,
    },
    Detach {
        generation: u64,
    },
    Server {
        generation: u64,
        event: Value,
    },
    Permission {
        request: bool,
    },
    View {
        session: String,
        account: String,
        chat: u64,
        visible: bool,
    },
    Reset,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Output {
    Action { generation: u64, data: Value },
    Disconnected { generation: u64 },
    Status { status: Status },
    Reset,
}

/// Private channels to either the parent actor or its inherited pipe relay.
pub struct Endpoint {
    pub(crate) commands: mpsc::Sender<Input>,
    pub(crate) events: mpsc::Receiver<Output>,
    pub task: JoinHandle<()>,
    pub(crate) stop: CancellationToken,
}

pub fn spawn(helper: Option<PathBuf>, shutdown: CancellationToken) -> Endpoint {
    // This runs in the unsandboxed parent. Resolve only the desktop shipped
    // beside this executable, never an executable named by a notification.
    let app = crate::notifications::helper::installed_app();
    spawn_with_activation(
        helper,
        shutdown,
        Arc::new(move |activation| {
            let app = app.clone();
            // Reap the IPC launcher or first GUI process without blocking delivery.
            let _ = std::thread::Builder::new()
                .name("notification-navigation".into())
                .spawn(move || {
                    open_activation(app.as_deref(), &activation, |url| {
                        let _ = crate::browser::open(url);
                    });
                });
        }),
    )
}

fn open_activation(
    app: Option<&std::path::Path>,
    activation: &Activation,
    browser: impl FnOnce(&str),
) {
    open_activation_with_window(
        app,
        activation,
        browser,
        Duration::from_secs(5),
        Duration::from_millis(25),
    );
}

fn open_activation_with_window(
    app: Option<&std::path::Path>,
    activation: &Activation,
    browser: impl FnOnce(&str),
    window: Duration,
    poll: Duration,
) {
    if let Some(app) = app
        && let Ok(target) = crate::notifications::navigation::Target::new(
            activation.server.clone(),
            activation.account.clone(),
            activation.chat,
        )
        && let Ok(mut child) = std::process::Command::new(app)
            .arg("--open-chat")
            .arg(target.encode())
            .envs(session_display())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        && desktop_started(&mut child, window, poll)
    {
        // Includes IPC handover and intentional account-scope rejection.
        return;
    }
    browser(&activation.browser_url());
}

/// The desktop's display, for an app the daemon starts. A systemd user
/// service that starts with the user's manager, before the desktop session
/// exports its display (uwsm, GNOME and KDE all import it later), has no
/// `WAYLAND_DISPLAY` or `DISPLAY` of its own, and the manager doesn't pass
/// what it learns later to services already running. The app could then
/// open no window. When this process has neither, take them from the
/// manager's current environment; otherwise keep what it has.
fn session_display() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        let has = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        if !has("WAYLAND_DISPLAY")
            && !has("DISPLAY")
            && let Ok(output) = std::process::Command::new("systemctl")
                .args(["--user", "show-environment"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
            && output.status.success()
        {
            return display_variables(&String::from_utf8_lossy(&output.stdout));
        }
    }
    Vec::new()
}

/// The display's variables in `systemctl --user show-environment`'s output.
/// Values systemd had to quote (`$'...'`) are left out: a display's never
/// need it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn display_variables(environment: &str) -> Vec<(String, String)> {
    const DISPLAY: [&str; 5] = [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XAUTHORITY",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
    ];
    environment
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, value)| {
            DISPLAY.contains(name) && !value.is_empty() && !value.starts_with("$'")
        })
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn desktop_started(child: &mut std::process::Child, window: Duration, poll: Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        let remaining = window.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            // A first GUI launch stays alive. Reap it on this navigation
            // thread, but never reopen an old notification after a later crash.
            let _ = child.wait();
            return true;
        }
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(poll.min(remaining)),
            Err(_) => return false,
        }
    }
}

pub fn spawn_with_activation(
    helper: Option<PathBuf>,
    shutdown: CancellationToken,
    activated: Arc<dyn Fn(Activation) + Send + Sync>,
) -> Endpoint {
    let stop = shutdown.child_token();
    let shutdown = stop.clone();
    let (commands, mut receiver) = mpsc::channel(CAPACITY);
    let (sender, events) = mpsc::channel(CAPACITY);
    let task = tokio::spawn(async move {
        let mut native = Native::new(helper, shutdown.child_token());
        let mut bridge = Bridge::new(native.restart().await, sender, activated);
        loop {
            let result = tokio::select! {
                () = shutdown.cancelled() => break,
                result = bridge.run(&mut receiver, &mut native.events) => result,
            };
            match result {
                Ok(false) => break,
                Ok(true) => {}
                Err(_) => {
                    let generation = bridge
                        .connection
                        .as_ref()
                        .map(|connection| connection.generation);
                    bridge.clear();
                    // Stop all native work before waiting for a slow consumer.
                    native.stop().await;
                    let notice = generation.map_or_else(
                        || Output::Status {
                            status: Status::default(),
                        },
                        |generation| Output::Disconnected { generation },
                    );
                    if !matches!(
                        tokio::time::timeout(Duration::from_secs(5), bridge.output.send(notice))
                            .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
            }
            bridge.native = native.restart().await;
            bridge.restart_native = false;
            bridge.permission = Permission::Unavailable;
            bridge.permission_pending = false;
            if bridge.reset_acknowledge {
                bridge.reset_acknowledge = false;
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(5), bridge.output.send(Output::Reset))
                        .await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
            if bridge.connection.is_some() && bridge.permission_at <= Instant::now() {
                bridge.permission(false);
            }
        }
        bridge.clear();
        native.stop().await;
    });
    Endpoint {
        commands,
        events,
        task,
        stop,
    }
}

struct Native {
    helper: Option<PathBuf>,
    lifetime: CancellationToken,
    shutdown: CancellationToken,
    events: Option<mpsc::Receiver<Event>>,
    task: Option<JoinHandle<()>>,
}

impl Native {
    fn new(helper: Option<PathBuf>, lifetime: CancellationToken) -> Self {
        Self {
            helper,
            shutdown: lifetime.child_token(),
            lifetime,
            events: None,
            task: None,
        }
    }

    async fn stop(&mut self) {
        self.shutdown.cancel();
        self.events = None;
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    async fn restart(&mut self) -> Option<mpsc::Sender<Command>> {
        self.stop().await;
        self.shutdown = self.lifetime.child_token();
        let native = supervisor::spawn(self.helper.clone()?, self.shutdown.clone());
        self.events = Some(native.events);
        self.task = Some(native.task);
        Some(native.commands)
    }
}

struct Route {
    generation: u64,
    actions: mpsc::Sender<Value>,
}

/// The daemon-facing client. The WebSocket owns one session at a time;
/// opening the desktop or following a chat never creates a delivery route.
pub struct Client {
    commands: mpsc::Sender<Input>,
    status: Mutex<Status>,
    route: Mutex<Option<Route>>,
    sequence: AtomicU64,
}

impl Client {
    pub fn new(mut endpoint: Endpoint, shutdown: CancellationToken) -> Arc<Self> {
        let client = Arc::new(Self {
            commands: endpoint.commands.clone(),
            status: Mutex::new(Status::default()),
            route: Mutex::new(None),
            sequence: AtomicU64::new(1),
        });
        let consumer = Arc::clone(&client);
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    () = shutdown.cancelled() => break,
                    event = endpoint.events.recv() => event,
                };
                let Some(event) = event else { break };
                match event {
                    Output::Status { status } => *consumer.status.lock().unwrap() = status,
                    Output::Action { generation, data } => {
                        let mut route = consumer.route.lock().unwrap();
                        if let Some(current) = route
                            .as_ref()
                            .filter(|route| route.generation == generation)
                            && current.actions.try_send(data).is_err()
                        {
                            // A stalled socket gets no successful delivery receipt.
                            // Dropping its sender lets the session reconnect cleanly.
                            *route = None;
                            let _ = consumer.commands.try_send(Input::Detach { generation });
                        }
                    }
                    Output::Disconnected { generation } => {
                        let mut route = consumer.route.lock().unwrap();
                        if route
                            .as_ref()
                            .is_some_and(|route| route.generation == generation)
                        {
                            *route = None;
                        }
                    }
                    Output::Reset => {}
                }
            }
            *consumer.status.lock().unwrap() = Status::default();
            *consumer.route.lock().unwrap() = None;
            endpoint.stop.cancel();
            let _ = endpoint.task.await;
        });
        client
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn permission(&self, request: bool) -> Result<()> {
        self.send(Input::Permission { request })
    }

    pub fn view(&self, session: String, account: String, chat: u64, visible: bool) -> Result<()> {
        if !valid_session(&session) || !valid_account(&account) || chat == 0 {
            bail!("That notification view isn't valid");
        }
        self.send(Input::View {
            session,
            account,
            chat,
            visible,
        })
    }

    fn send(&self, command: Input) -> Result<()> {
        if !pipe::fits_frame(&command) {
            bail!("Notification frame too large");
        }
        // Keep room for connection boundaries even when view updates or
        // server offers arrive faster than the helper can accept them.
        if !matches!(command, Input::Attach { .. } | Input::Detach { .. })
            && self.commands.capacity() <= 8
        {
            bail!("Notifications are temporarily unavailable");
        }
        self.commands
            .try_send(command)
            .map_err(|_| anyhow!("Notifications are temporarily unavailable"))
    }

    pub fn attach(self: &Arc<Self>, server: &ServerUrl, device_id: &str) -> Result<Session> {
        let generation = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (actions, receiver) = mpsc::channel(64);
        *self.route.lock().unwrap() = Some(Route {
            generation,
            actions,
        });
        self.send(Input::Attach {
            generation,
            server: server.origin(),
            device_id: device_id.into(),
        })?;
        Ok(Session {
            client: Arc::clone(self),
            generation,
            actions: receiver,
        })
    }
}

pub struct Session {
    client: Arc<Client>,
    generation: u64,
    pub actions: mpsc::Receiver<Value>,
}

impl Session {
    pub fn receive(&self, event: Value) -> Result<()> {
        // Validate and normalize before the sandbox boundary. An oversized
        // or future notification cannot tear down private pipes or MCP.
        let Ok(event) = serde_json::from_value::<ServerEvent>(event) else {
            return Ok(());
        };
        if !event.valid() {
            return Ok(());
        }
        let command = Input::Server {
            generation: self.generation,
            event: serde_json::to_value(event)?,
        };
        if !pipe::fits_frame(&command) {
            return Ok(());
        }
        self.client.send(command)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut route = self.client.route.lock().unwrap();
        if route
            .as_ref()
            .is_some_and(|route| route.generation == self.generation)
        {
            *route = None;
        }
        let _ = self.client.send(Input::Detach {
            generation: self.generation,
        });
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerEvent {
    Ready {
        account: String,
        device_id: u64,
        heartbeat_after: u64,
        visibility_ttl: u64,
    },
    Show(Offer),
    Withdraw {
        notification_id: String,
        delivery_token: String,
    },
    Unavailable,
}

impl ServerEvent {
    fn valid(&self) -> bool {
        match self {
            Self::Ready {
                account, device_id, ..
            } => valid_account(account) && *device_id > 0,
            Self::Show(offer) => valid_account(&offer.account) && valid_offer(offer),
            Self::Withdraw {
                notification_id,
                delivery_token,
            } => valid_offer_id(notification_id, delivery_token),
            Self::Unavailable => true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Offer {
    notification_id: String,
    delivery_token: String,
    account: String,
    chat: u64,
    kind: String,
    title: String,
    body: String,
    thread: String,
    expires_at: String,
}

struct Connection {
    generation: u64,
    server: ServerUrl,
    device_id: u64,
    account: Option<String>,
}

struct Pending {
    offer: Offer,
    attempt: u64,
    deadline: Instant,
    shown: bool,
}

struct View {
    chat: u64,
    until: Instant,
}

struct Bridge {
    native: Option<mpsc::Sender<Command>>,
    output: mpsc::Sender<Output>,
    activated: Arc<dyn Fn(Activation) + Send + Sync>,
    connection: Option<Connection>,
    active: HashMap<String, Pending>,
    seen: HashMap<String, Instant>,
    views: HashMap<String, View>,
    attempt: u64,
    permission: Permission,
    permission_pending: bool,
    heartbeat_at: Instant,
    permission_at: Instant,
    restart_native: bool,
    reset_acknowledge: bool,
}

impl Bridge {
    fn new(
        native: Option<mpsc::Sender<Command>>,
        output: mpsc::Sender<Output>,
        activated: Arc<dyn Fn(Activation) + Send + Sync>,
    ) -> Self {
        Self {
            native,
            output,
            activated,
            connection: None,
            active: HashMap::new(),
            seen: HashMap::new(),
            views: HashMap::new(),
            attempt: 0,
            permission: Permission::Unavailable,
            permission_pending: false,
            heartbeat_at: Instant::now(),
            permission_at: Instant::now(),
            restart_native: false,
            reset_acknowledge: false,
        }
    }

    fn status(&self) -> Status {
        let account = self
            .connection
            .as_ref()
            .and_then(|connection| connection.account.clone());
        Status {
            // Freedesktop has no permission-query API. Unknown permits an
            // attempt, whose actual acceptance still needs a matching Shown.
            available: self.native.is_some()
                && matches!(self.permission, Permission::Granted | Permission::Unknown),
            permission: self.permission,
            connected: account.is_some(),
            account,
        }
    }

    fn emit(&self, output: Output) -> Result<()> {
        self.output
            .try_send(output)
            .map_err(|_| anyhow!("Notification consumer stopped"))
    }

    fn action(&self, data: Value) -> Result<()> {
        if let Some(connection) = &self.connection
            && connection.account.is_some()
        {
            self.emit(Output::Action {
                generation: connection.generation,
                data,
            })?;
        }
        Ok(())
    }

    fn status_changed(&mut self) -> Result<()> {
        self.emit(Output::Status {
            status: self.status(),
        })?;
        self.heartbeat_at = Instant::now() + HEARTBEAT;
        self.action(json!({"action": "heartbeat", "available": self.status().available }))
    }

    fn native(&self, command: Command) -> bool {
        self.native
            .as_ref()
            .is_some_and(|native| native.try_send(command).is_ok())
    }

    fn clear(&mut self) {
        for id in self.active.keys() {
            self.native(Command::Withdraw { id: id.clone() });
        }
        self.active.clear();
        self.seen.clear();
        self.views.clear();
        self.connection = None;
        self.restart_native = true;
    }

    fn permission(&mut self, request: bool) {
        if self.permission_pending {
            return;
        }
        self.permission_pending = self.native(if request {
            Command::RequestPermission
        } else {
            Command::Permission
        });
        self.permission_at = Instant::now() + HEARTBEAT;
    }

    async fn run(
        &mut self,
        inputs: &mut mpsc::Receiver<Input>,
        native: &mut Option<mpsc::Receiver<Event>>,
    ) -> Result<bool> {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        self.emit(Output::Status {
            status: self.status(),
        })?;
        loop {
            if self.restart_native {
                return Ok(true);
            }
            tokio::select! {
                biased;
                command = inputs.recv() => match command {
                    Some(command) => self.command(command)?,
                    None => return Ok(false),
                },
                event = native_event(native) => match event {
                    Some(event) => self.observation(event)?,
                    None => { self.failed()?; *native = None; self.native = None; },
                },
                _ = tick.tick() => self.tick()?,
            }
        }
    }

    fn command(&mut self, command: Input) -> Result<()> {
        match command {
            Input::Reset => {
                self.clear();
                self.reset_acknowledge = true;
            }
            Input::Attach {
                generation,
                server,
                device_id,
            } => {
                self.clear();
                if let (Ok(server), Ok(device_id)) =
                    (ServerUrl::parse(&server), device_id.parse::<u64>())
                    && generation > 0
                    && device_id > 0
                {
                    self.connection = Some(Connection {
                        generation,
                        server,
                        device_id,
                        account: None,
                    });
                }
                self.permission_at = Instant::now();
            }
            Input::Detach { generation } => {
                if self
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.generation == generation)
                {
                    self.clear();
                }
            }
            Input::Server { generation, event } => {
                if self
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.generation == generation)
                {
                    match serde_json::from_value(event) {
                        Ok(event) => self.server(event)?,
                        Err(_) => {
                            self.clear();
                        }
                    }
                }
            }
            Input::Permission { request } => self.permission(request),
            Input::View {
                session,
                account,
                chat,
                visible,
            } => self.view(session, account, chat, visible)?,
        }
        Ok(())
    }

    fn server(&mut self, event: ServerEvent) -> Result<()> {
        match event {
            ServerEvent::Ready {
                account,
                device_id,
                heartbeat_after,
                visibility_ttl,
            } => {
                let Some(connection) = &mut self.connection else {
                    return Ok(());
                };
                if !valid_account(&account)
                    || device_id != connection.device_id
                    || heartbeat_after != 30
                    || visibility_ttl != 75
                    || connection.account.is_some()
                {
                    self.clear();
                } else {
                    connection.account = Some(account);
                }
                self.status_changed()?;
            }
            ServerEvent::Show(offer) => self.show(offer)?,
            ServerEvent::Withdraw {
                notification_id,
                delivery_token,
            } => {
                let id = self
                    .active
                    .iter()
                    .find(|(_, pending)| {
                        pending.offer.notification_id == notification_id
                            && pending.offer.delivery_token == delivery_token
                    })
                    .map(|(id, _)| id.clone());
                if let Some(id) = id {
                    self.active.remove(&id);
                    if !self.native(Command::Withdraw { id }) {
                        self.failed()?;
                    }
                }
            }
            ServerEvent::Unavailable => {
                self.clear();
                self.status_changed()?;
            }
        }
        Ok(())
    }

    fn receipt(&self, offer: &Offer, status: &str) -> Result<()> {
        self.action(json!({ "action": "receipt", "notification_id": offer.notification_id, "delivery_token": offer.delivery_token, "status": status }))
    }

    fn show(&mut self, offer: Offer) -> Result<()> {
        let Some(account) = self
            .connection
            .as_ref()
            .and_then(|connection| connection.account.as_ref())
        else {
            return Ok(());
        };
        if offer.account != *account || !valid_offer(&offer) {
            return Ok(());
        }
        // Repeated wire frames cannot replace or replay an accepted attempt.
        if self.seen.contains_key(&offer.notification_id) {
            return Ok(());
        }
        if self.seen.len() >= 256 {
            return self.receipt(&offer, "failed");
        }
        self.seen.insert(
            offer.notification_id.clone(),
            Instant::now() + Duration::from_secs(60),
        );
        // The server gives an offer 15 seconds. On a clock even a little
        // behind the server's, that reads as more than 15, and turning such
        // offers down failed every notification on that computer. Wait at
        // most 15 seconds instead: the server refuses a late receipt anyway.
        let deadline = OffsetDateTime::parse(&offer.expires_at, &Rfc3339)
            .ok()
            .and_then(|expiry| (expiry - OffsetDateTime::now_utc()).try_into().ok())
            .filter(|remaining: &Duration| !remaining.is_zero())
            .map(|remaining: Duration| remaining.min(OFFER_WINDOW));
        let id = format!("{}:{}", offer.account, offer.thread);
        if let Some(previous) = self.active.remove(&id) {
            if !previous.shown {
                self.receipt(&previous.offer, "failed")?;
            }
            if !self.native(Command::Withdraw { id: id.clone() }) {
                self.receipt(&offer, "failed")?;
                return self.failed();
            }
        }
        let Some(remaining) = deadline else {
            return self.receipt(&offer, "failed");
        };
        if !self.status().available {
            return self.receipt(&offer, "failed");
        }
        if self.active.len() >= protocol::MAX_ACTIVE
            && let Some(oldest) = self
                .active
                .iter()
                .min_by_key(|(_, pending)| pending.attempt)
                .map(|(id, _)| id.clone())
        {
            let retired = self.active.remove(&oldest).unwrap();
            if !retired.shown {
                self.receipt(&retired.offer, "failed")?;
            }
            if !self.native(Command::Withdraw { id: oldest }) {
                self.receipt(&offer, "failed")?;
                return self.failed();
            }
        }
        let Some(attempt) = self.attempt.checked_add(1) else {
            return self.receipt(&offer, "failed");
        };
        self.attempt = attempt;
        let command = Command::Show {
            id: id.clone(),
            attempt: Some(attempt),
            title: offer.title.clone(),
            body: Some(offer.body.clone()),
        };
        if !self.native(command) {
            self.receipt(&offer, "failed")?;
            return self.failed();
        }
        self.active.insert(
            id,
            Pending {
                offer,
                attempt,
                deadline: Instant::now() + remaining,
                shown: false,
            },
        );
        Ok(())
    }

    fn observation(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Ready { .. } => {}
            Event::Permission { state } => {
                self.permission_pending = false;
                self.permission = state.into();
                self.status_changed()?;
            }
            Event::Failed { id: None, .. } => self.failed()?,
            Event::Shown { id, attempt } => {
                if let Some(pending) = self.active.get_mut(&id)
                    && Some(pending.attempt) == attempt
                    && !pending.shown
                    && pending.deadline > Instant::now()
                {
                    pending.shown = true;
                    let data = json!({"action":"receipt", "notification_id": pending.offer.notification_id, "delivery_token": pending.offer.delivery_token, "status":"shown"});
                    self.action(data)?;
                }
            }
            Event::Activated {
                id,
                attempt,
                action,
            } => {
                if let Some(pending) = self.active.get(&id)
                    && Some(pending.attempt) == attempt
                    && pending.shown
                    && action.as_deref().is_none_or(|action| action == "default")
                {
                    if let Some(connection) = &self.connection {
                        (self.activated)(Activation {
                            server: connection.server.clone(),
                            account: pending.offer.account.clone(),
                            chat: pending.offer.chat,
                        });
                    }
                    self.active.remove(&id);
                }
            }
            Event::Failed {
                id: Some(id),
                attempt,
                ..
            } => {
                if self
                    .active
                    .get(&id)
                    .is_some_and(|pending| Some(pending.attempt) == attempt)
                {
                    let pending = self.active.remove(&id).unwrap();
                    if !pending.shown {
                        self.receipt(&pending.offer, "failed")?;
                    }
                    self.permission = Permission::Unavailable;
                    self.permission_pending = false;
                    self.permission_at = Instant::now() + Duration::from_secs(2);
                    self.status_changed()?;
                }
            }
            Event::Dismissed { id, attempt } | Event::Closed { id, attempt } => {
                if self
                    .active
                    .get(&id)
                    .is_some_and(|pending| Some(pending.attempt) == attempt)
                {
                    let pending = self.active.remove(&id).unwrap();
                    if !pending.shown {
                        self.receipt(&pending.offer, "failed")?;
                    }
                }
            }
        }
        Ok(())
    }

    fn failed(&mut self) -> Result<()> {
        for pending in self.active.values() {
            if !pending.shown {
                self.receipt(&pending.offer, "failed")?;
            }
        }
        for id in self.active.keys() {
            self.native(Command::Withdraw { id: id.clone() });
        }
        self.active.clear();
        self.permission = Permission::Unavailable;
        self.permission_pending = false;
        self.permission_at = Instant::now() + Duration::from_secs(2);
        self.restart_native = true;
        self.status_changed()
    }

    fn view(&mut self, session: String, account: String, chat: u64, visible: bool) -> Result<()> {
        if !valid_session(&session)
            || chat == 0
            || self
                .connection
                .as_ref()
                .and_then(|connection| connection.account.as_deref())
                != Some(account.as_str())
        {
            return Ok(());
        }
        if !visible
            && self
                .views
                .get(&session)
                .is_some_and(|view| view.chat != chat)
        {
            return Ok(());
        }
        let previous = self.views.remove(&session);
        if visible && self.views.len() < MAX_VIEWS {
            self.views.insert(
                session,
                View {
                    chat,
                    until: Instant::now() + VIEW_TTL,
                },
            );
            self.action(json!({"action": "view", "chat": chat, "visible": true}))?;
        }
        if let Some(previous) = previous
            && !self.views.values().any(|view| view.chat == previous.chat)
        {
            self.action(json!({"action":"view", "chat": previous.chat, "visible": false}))?;
        }
        Ok(())
    }

    fn tick(&mut self) -> Result<()> {
        let now = Instant::now();
        self.seen.retain(|_, until| *until > now);
        let expired: Vec<_> = self
            .active
            .iter()
            .filter(|(_, pending)| !pending.shown && pending.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            let pending = self.active.remove(&id).unwrap();
            self.receipt(&pending.offer, "failed")?;
            if !self.native(Command::Withdraw { id }) {
                self.failed()?;
                break;
            }
        }
        let hidden: Vec<_> = self
            .views
            .values()
            .filter(|view| view.until <= now)
            .map(|view| view.chat)
            .collect();
        self.views.retain(|_, view| view.until > now);
        for chat in hidden {
            if !self.views.values().any(|view| view.chat == chat) {
                self.action(json!({"action":"view", "chat": chat, "visible": false}))?;
            }
        }
        if self.heartbeat_at <= now {
            self.status_changed()?;
        }
        if self.connection.is_some() && self.permission_at <= now {
            self.permission(false);
        }
        Ok(())
    }
}

async fn native_event(events: &mut Option<mpsc::Receiver<Event>>) -> Option<Event> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

fn valid_account(account: &str) -> bool {
    (7..=20).contains(&account.len()) && account.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_session(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 64
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn valid_offer(offer: &Offer) -> bool {
    valid_offer_id(&offer.notification_id, &offer.delivery_token)
        && offer.expires_at.len() <= 64
        && offer.chat > 0
        && matches!(
            offer.kind.as_str(),
            "answered" | "approval_waiting" | "input_requested" | "failed"
        )
        && !offer.thread.is_empty()
        && offer.thread.len() <= 200
        && offer
            .thread
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        && Command::Show {
            id: format!("{}:{}", offer.account, offer.thread),
            attempt: Some(1),
            title: offer.title.clone(),
            body: Some(offer.body.clone()),
        }
        .validate()
        .is_ok()
}

fn valid_offer_id(uuid: &str, delivery_token: &str) -> bool {
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
        && delivery_token.len() == 48
        && delivery_token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
