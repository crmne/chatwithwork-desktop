//! `cww daemon run`: the reader, the control socket and the tunnel.

mod organizations;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditEntry, AuditLog};
use crate::auth::load_credentials;
use crate::auth::organizations::SavedOrganization;

use crate::chats::{ChatClient, ChatHub, MAX_UPLOAD_BYTES, Question, Upload, chat_number};
use crate::config::Config;
use crate::control::{
    self, ControlEvent, ControlHandler, ControlRequest, PROTOCOL_VERSION, Refusal,
};
use crate::limits::Limiter;
use crate::paths::Paths;
use crate::policy::DEFAULT_DENY;
use crate::reader::Reader;
use crate::roots::{NewRoot, add_root, remove_root, set_writable};
use crate::status::{Changes, Connection, SharedStatus};
use crate::tools::LocalFiles;
use crate::tunnel::{Tunnel, TunnelExit, TunnelSettings};
use crate::writer::Writer;

#[derive(Clone)]
struct OrganizationContext {
    primary: Paths,
    profile: SavedOrganization,
    proxy: Option<String>,
}

struct Daemon {
    organization: Option<OrganizationContext>,
    /// Where `run` starts commands.
    runner: crate::commands::Runner,
    paths: Paths,
    config: Mutex<Config>,
    reader: Arc<Reader>,
    writer: Arc<Writer>,
    limiter: Arc<Limiter>,
    audit: Arc<AuditLog>,
    paused: Arc<AtomicBool>,
    status: SharedStatus,
    /// Cancelled to restart the tunnel with new credentials.
    tunnel_token: Mutex<CancellationToken>,
    /// Wakes the supervisor after a reload.
    reloaded: Notify,
    /// Held while a request edits config.toml, so concurrent edits don't
    /// save over each other.
    config_edits: Mutex<()>,
    /// Events for control-channel subscribers.
    events: broadcast::Sender<ControlEvent>,
    /// Bumped whenever something in `status` changes.
    changes: Changes,
    /// Cancelled by a `shutdown` request.
    shutdown: CancellationToken,
    /// The chats terminal UIs follow over the tunnel.
    chats: Arc<ChatHub>,
    /// The chat API as this device, while it's paired.
    chat_client: Mutex<Option<Arc<ChatClient>>>,
    notifications: Arc<crate::notification_delivery::Client>,
}

/// The longest question the terminal may send, as the server allows.
const MAX_QUESTION_CHARS: usize = 20_000;

/// The longest reason for denying a change, as the server keeps it.
const MAX_REASON_CHARS: usize = 500;

/// The most files one question carries.
const MAX_ATTACHMENTS: usize = 10;

const MAX_TITLE_CHARS: usize = 200;

/// An ID or type passed through to the server: printable ASCII without
/// spaces, at most `max` bytes.
fn plain_token<'a>(value: &'a str, max: usize, what: &str) -> Result<&'a str, Refusal> {
    if !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_graphic()) {
        Ok(value)
    } else {
        Err(Refusal::new(
            "bad_request",
            format!("bad request: that {what} isn't valid"),
        ))
    }
}

/// A file's name as the server sees it: its last component, without
/// control characters or quotes, at most 255 characters.
fn upload_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '"')
        .take(255)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        "file".into()
    } else {
        clean.to_string()
    }
}

/// Set when a newly shared folder is outside the sandbox: the worker exits
/// with `sandbox::RESTART_CODE` and its supervisor starts it again.
static RESTART_FOR_SANDBOX: AtomicBool = AtomicBool::new(false);

/// How `cww daemon run` (or `cww-agent` on Windows) was started.
pub struct Foreground {
    pub log_file: Option<std::path::PathBuf>,
    /// Confine the daemon, unless the config turns the sandbox off.
    pub sandbox: bool,
    /// This process is the confined worker of a supervisor.
    pub worker: bool,
}

/// `cww daemon run`: set up logging, then run the daemon until a signal or
/// a `shutdown` request. With the sandbox on (Linux, macOS), this process
/// supervises a confined worker instead of running the daemon itself.
pub fn run_foreground(paths: Paths, options: &Foreground) -> Result<()> {
    crate::logging::init(options.log_file.as_deref())?;
    crate::crash_reports::install(crate::crash_reports::Component::Daemon, &paths);
    let config = Config::load(&paths)?;
    let sandboxed = options.sandbox
        && config.sandbox.enabled
        && cfg!(any(target_os = "linux", target_os = "macos"));
    if sandboxed && !options.worker {
        return supervise_worker(&paths, options);
    }
    #[cfg(unix)]
    let notification_pipes = options
        .worker
        .then(crate::notification_delivery::Inherited::take)
        .transpose()?;
    // The supervisor's channel for commands, taken before confinement.
    #[cfg(unix)]
    let run_channel = options
        .worker
        .then(crate::commands::channel::inherited)
        .flatten();
    let notification_helper = (!options.worker)
        .then(crate::notifications::helper::installed_helper)
        .flatten();
    // Trash records use local times; `time` only finds the offset while
    // the process has one thread.
    crate::trash::remember_local_offset();
    if sandboxed {
        confine(&config, &paths);
    } else {
        crate::sandbox::disabled(if options.sandbox && config.sandbox.enabled {
            "no sandbox on this platform yet"
        } else {
            "turned off"
        });
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let shutdown = CancellationToken::new();
        #[cfg(unix)]
        let notifications = match notification_pipes {
            Some(pipes) => pipes.connect(shutdown.child_token())?,
            None => {
                crate::notification_delivery::spawn(notification_helper, shutdown.child_token())
            }
        };
        #[cfg(not(unix))]
        let notifications =
            crate::notification_delivery::spawn(notification_helper, shutdown.child_token());
        let profiles = (!options.worker).then(|| {
            organizations::spawn(
                paths.clone(),
                crate::notifications::helper::installed_helper(),
                shutdown.child_token(),
            )
        });
        #[cfg(unix)]
        let runner = match run_channel {
            Some(fd) => crate::commands::channel::Client::new(fd)
                .map(crate::commands::Runner::Supervisor)
                .unwrap_or_else(|e| {
                    crate::commands::Runner::Off(format!("no channel to run commands: {e}"))
                }),
            None if options.worker => {
                crate::commands::Runner::Off("this daemon's supervisor doesn't run commands".into())
            }
            None => crate::commands::Runner::local(&paths),
        };
        #[cfg(not(unix))]
        let runner = crate::commands::Runner::local(&paths);
        let result =
            run_with_notifications(paths, shutdown.clone(), true, notifications, None, runner)
                .await;
        shutdown.cancel();
        if let Some(task) = profiles {
            let _ = task.await;
        }
        result
    })?;
    drop(runtime);
    if RESTART_FOR_SANDBOX.load(Ordering::SeqCst) {
        std::process::exit(crate::sandbox::RESTART_CODE);
    }
    Ok(())
}

/// Confine this process as the daemon would, for `cww debug`. `keychain`
/// overrides whether the plan allows the OS keychain.
pub fn confine_for_check(paths: &Paths, keychain: Option<bool>) -> Result<crate::sandbox::Status> {
    let mut config = Config::load(paths)?;
    if let Some(keychain) = keychain {
        config.secret_store = Some(if keychain { "keyring" } else { "file" }.into());
    }
    confine(&config, paths);
    Ok(crate::sandbox::status())
}

/// Make one change as the daemon would, for `cww debug change`, after
/// [`confine_for_check`].
pub fn change_for_check(
    paths: &Paths,
    tool: &str,
    arguments: Value,
) -> Result<Result<crate::writer::ChangeResult, crate::error::ToolError>> {
    let mut config = Config::load(paths)?;
    config.index.enabled = false;
    let reader = Arc::new(Reader::new(&config, paths, Changes::default())?);
    let writer = Writer::new(
        reader,
        Arc::new(Limiter::new(config.limits.clone())),
        crate::trash::Trash::new(paths.home_trash.clone()),
    );
    Ok(crate::tools::run_change(&writer, tool, arguments))
}

/// Apply the sandbox for the configured roots. Runs before any thread
/// starts: Landlock only confines the calling thread and its children.
fn confine(config: &Config, paths: &Paths) {
    // Everything the sandbox would block later has to happen now: the OS
    // certificate store is read once, here, and cww's directories must exist
    // for rules to attach to them.
    let _ = crate::tls::root_certificates();
    let _ = crate::paths::documents_dir();
    let _ = paths.ensure();
    if let Some(dir) = paths.socket_path().parent() {
        let _ = crate::paths::ensure_private_dir(dir);
    }
    let keychain = crate::auth::secrets::SecretStore::for_config(config, paths)
        == crate::auth::secrets::SecretStore::Keyring;
    // Folders that allow changes, and the trash their files go to: made
    // now, since the sandbox won't let the daemon make them later.
    let writable = writable_roots(config);
    let trash = crate::trash::prepare(
        &crate::trash::Trash::new(paths.home_trash.clone()),
        &writable,
    );
    let plan =
        crate::sandbox::Plan::new(config.roots.iter().map(|r| r.path.clone()), paths, keychain)
            .with_changes(writable, trash);
    let status = crate::sandbox::confine(plan);
    match status.state {
        "enforced" => tracing::info!("sandbox: {} enforced", status.kind),
        _ => tracing::warn!(
            "sandbox: {} {}{}",
            status.kind,
            status.state,
            status.detail.map(|d| format!(" ({d})")).unwrap_or_default()
        ),
    }
}

/// The folders that allow commands, and whether they may use the network.
/// A command call the runner refused, as the control channel says it.
fn command_refusal(e: crate::commands::RunError) -> anyhow::Error {
    Refusal::new(&e.code, e.message).into()
}

fn command_roots(config: &Config) -> Vec<(String, String, bool)> {
    config
        .roots
        .iter()
        .filter(|r| r.runs_commands())
        .map(|r| (r.id.clone(), r.label.clone(), r.network))
        .collect()
}

/// The command tools for `config`'s folders.
fn command_tools(
    config: &Config,
    runner: &crate::commands::Runner,
) -> crate::commands::tools::CommandTools {
    crate::commands::tools::CommandTools {
        runner: runner.clone(),
        roots: command_roots(config)
            .into_iter()
            .map(|(id, label, network)| crate::commands::tools::CommandRoot { id, label, network })
            .collect(),
        limits: crate::commands::Limits::from_config(&config.limits),
    }
}

/// The folders that allow changes.
fn writable_roots(config: &Config) -> Vec<PathBuf> {
    config
        .roots
        .iter()
        .filter(|r| r.writable)
        .map(|r| r.path.clone())
        .collect()
}

/// Run the confined worker, starting it again whenever it asks to be (to
/// apply a new sandbox), and pass signals on to it.
fn supervise_worker(paths: &Paths, options: &Foreground) -> Result<()> {
    let exe = std::env::current_exe().context("finding the cww binary")?;
    let helper = crate::notifications::helper::installed_helper();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let parent_shutdown = CancellationToken::new();
        let profiles = organizations::spawn(paths.clone(), helper.clone(), parent_shutdown.child_token());
        let mut notifications = crate::notification_delivery::spawn(helper, parent_shutdown.child_token());
        loop {
            // Each worker's commands, stopped when it ends.
            #[cfg(unix)]
            let runs = crate::commands::Runs::new(paths.clone());
            let mut command = tokio::process::Command::new(&exe);
            command.args(["daemon", "run", "--worker"]);
            // Commands the worker runs start here, outside its sandbox,
            // each in one of their own (`commands::channel`).
            #[cfg(unix)]
            let run_channel = match crate::commands::channel::Pair::new() {
                Ok(pair) => {
                    command.env(crate::commands::channel::ENV, pair.worker_fd().to_string());
                    Some(pair)
                }
                Err(e) => {
                    tracing::warn!("no channel for commands: {e}");
                    None
                }
            };
            command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true);
            if let Some(log) = &options.log_file {
                command.arg("--log-file").arg(log);
            }
            let mut child = command.spawn().context("starting the daemon")?;
            #[cfg(unix)]
            if let Some(pair) = run_channel {
                // The worker has its end now; this process keeps only its
                // own, so the channel closes when the worker goes.
                let crate::commands::channel::Pair { supervisor, worker } = pair;
                drop(worker);
                let (runs, paths) = (runs.clone(), paths.clone());
                let _ = std::thread::Builder::new()
                    .name("cww-runs".into())
                    .spawn(move || crate::commands::channel::serve(supervisor, runs, paths));
            }
            let child_input = child.stdin.take().context("opening private notification input")?;
            let child_output = child.stdout.take().context("opening private notification output")?;
            let worker_shutdown = CancellationToken::new();
            let (status, stopping) = {
                let relay = crate::notification_delivery::serve_worker(child_output, child_input, &mut notifications, worker_shutdown.clone());
                tokio::pin!(relay);
                let mut relay_finished = false;
                let result = tokio::select! {
                    status = child.wait() => (status?, false),
                    result = &mut relay => {
                        relay_finished = true;
                        if result.is_err() { tracing::warn!("private notification channel stopped"); }
                        let status = match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                            Ok(status) => status?,
                            Err(_) => { child.kill().await?; child.wait().await? },
                        };
                        (status, false)
                    }
                    () = wait_for_signal() => {
                        parent_shutdown.cancel();
                        #[cfg(unix)]
                        if let Some(pid) = child.id().and_then(|id| rustix::process::Pid::from_raw(id as i32)) {
                            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                        }
                        let status = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                            Ok(status) => status?,
                            Err(_) => { child.kill().await?; child.wait().await? },
                        };
                        (status, true)
                    }
                };
                worker_shutdown.cancel();
                // The relay's cleanup withdraws observations before a worker
                // can restart and reuse its local connection generations.
                if !relay_finished { let _ = (&mut relay).await; }
                result
            };
            // Nothing a worker started outlives it.
            #[cfg(unix)]
            runs.stop_all_and_wait(Duration::from_secs(5));
            if stopping || status.code() != Some(crate::sandbox::RESTART_CODE) {
                parent_shutdown.cancel();
                let _ = notifications.task.await;
                let _ = profiles.await;
                if stopping || status.success() { return Ok(()); }
                anyhow::bail!("the daemon stopped ({status})");
            }
            tracing::info!("restarting the daemon with a sandbox for the new folders");
        }
    })
}

/// Run the daemon until `shutdown` is cancelled (or SIGINT/SIGTERM when
/// `handle_signals` is set).
pub async fn run(paths: Paths, shutdown: CancellationToken, handle_signals: bool) -> Result<()> {
    // Embedders and tests opt in explicitly to a native helper. Merely
    // constructing a daemon must not open OS notification connections.
    let notifications = crate::notification_delivery::spawn(None, shutdown.child_token());
    let runner = crate::commands::Runner::local(&paths);
    run_with_notifications(paths, shutdown, handle_signals, notifications, None, runner).await
}

/// Run an independent chat control socket without any file RPC capability.
/// Like `run`, embedders opt into native notifications separately.
pub async fn run_organization(
    primary: Paths,
    profile: SavedOrganization,
    shutdown: CancellationToken,
) -> Result<()> {
    let paths = profile.runtime_paths(&primary)?;
    let proxy = Config::load(&primary)?.proxy;
    let notifications = crate::notification_delivery::spawn(None, shutdown.child_token());
    run_with_notifications(
        paths,
        shutdown,
        false,
        notifications,
        Some(OrganizationContext {
            primary,
            profile,
            proxy,
        }),
        crate::commands::Runner::Off("an organization's chats run no commands".into()),
    )
    .await
}

async fn run_with_notifications(
    paths: Paths,
    shutdown: CancellationToken,
    handle_signals: bool,
    notifications: crate::notification_delivery::Endpoint,
    organization: Option<OrganizationContext>,
    runner: crate::commands::Runner,
) -> Result<()> {
    #[cfg(unix)]
    {
        if organization.is_none() {
            let previous = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
            crate::writer::remember_umask(previous.bits() as u32);
        }
    }
    paths.ensure()?;
    let config = match &organization {
        Some(context) => {
            let mut config = Config {
                server: Some(context.profile.server.clone()),
                ..Config::default()
            };
            config.proxy = context.proxy.clone();
            config.index.enabled = false;
            config
        }
        None => Config::load(&paths)?,
    };
    let (events, _) = broadcast::channel(256);
    let audit = Arc::new(AuditLog::open(&paths.audit_file())?.with_events(events.clone()));
    // Bind first: a second daemon must fail before it touches the index.
    let listener = control::bind(&paths.socket_path())?;

    let changes = Changes::default();
    let reader = {
        let (config, paths, changes) = (config.clone(), paths.clone(), changes.clone());
        tokio::task::spawn_blocking(move || Reader::new(&config, &paths, changes))
            .await
            .context("starting the reader")??
    };
    let reader = Arc::new(reader);
    let limiter = Arc::new(Limiter::new(config.limits.clone()));
    let writer = Arc::new(Writer::new(
        Arc::clone(&reader),
        Arc::clone(&limiter),
        crate::trash::Trash::new(paths.home_trash.clone()),
    ));
    let daemon = Arc::new(Daemon {
        organization,
        runner,
        limiter,
        writer,
        paused: Arc::new(AtomicBool::new(config.paused)),
        config: Mutex::new(config),
        reader,
        audit,
        status: SharedStatus::new(changes.clone()),
        tunnel_token: Mutex::new(shutdown.child_token()),
        reloaded: Notify::new(),
        chats: Arc::new(ChatHub::new(events.clone())),
        chat_client: Mutex::new(None),
        notifications: crate::notification_delivery::Client::new(
            notifications,
            shutdown.child_token(),
        ),
        config_edits: Mutex::new(()),
        events,
        changes,
        shutdown: shutdown.clone(),
        paths,
    });
    tokio::spawn(Arc::clone(&daemon).publish_status(shutdown.clone()));

    if handle_signals {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            shutdown.cancel();
        });
    }
    tokio::spawn(control::serve(
        listener,
        Arc::clone(&daemon),
        shutdown.clone(),
    ));

    let mut entry = AuditEntry::event("started");
    entry.detail = Some(format!("cww {}", env!("CARGO_PKG_VERSION")));
    daemon.audit.append(&entry);

    daemon.supervise(&shutdown).await;

    let runner = daemon.runner.clone();
    let _ = tokio::task::spawn_blocking(move || runner.stop_all()).await;
    daemon.audit.append(&AuditEntry::event("stopped"));
    Ok(())
}

impl Daemon {
    /// Push a fresh status to subscribers whenever something changes. Sleeps
    /// until then; bursts of changes are coalesced.
    async fn publish_status(self: Arc<Self>, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = self.changes.wait() => {}
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(Duration::from_millis(150)) => {}
            }
            if self.events.receiver_count() > 0 {
                let status = self.status_json().await;
                let _ = self.events.send(ControlEvent::Status { status });
            }
        }
    }

    /// Keep a tunnel running whenever the computer is paired.
    async fn supervise(self: &Arc<Self>, shutdown: &CancellationToken) {
        while !shutdown.is_cancelled() {
            let config = self.config.lock().await.clone();
            let paths = self.paths.clone();
            let organization = self.organization.clone();
            let credentials = tokio::task::spawn_blocking(move || match organization {
                Some(context) => context.profile.credentials(&context.primary).map(Some),
                None => load_credentials(&config, &paths),
            })
            .await;
            let credentials = match credentials {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    tracing::error!("can't load credentials: {e:#}");
                    self.status
                        .set(Connection::NotPaired, Some(format!("{e:#}")));
                    None
                }
                Err(e) => {
                    tracing::error!("credential task failed: {e}");
                    None
                }
            };
            let Some(credentials) = credentials else {
                *self.chat_client.lock().await = None;
                if self.status.get().connection != Connection::NotPaired {
                    self.status.set(Connection::NotPaired, None);
                }
                self.wait_for_reload(shutdown).await;
                continue;
            };

            let token = shutdown.child_token();
            *self.tunnel_token.lock().await = token.clone();
            let max_message_bytes = self.config.lock().await.limits.max_message_bytes;
            let commands = command_tools(&*self.config.lock().await, &self.runner);
            let files = LocalFiles::new(
                Arc::clone(&self.reader),
                Arc::clone(&self.writer),
                Arc::clone(&self.limiter),
                Arc::clone(&self.audit),
                Arc::clone(&self.paused),
                max_message_bytes,
            )
            .with_commands(commands);
            let proxy_setting = self.config.lock().await.proxy.clone();
            let tunnel = crate::proxy::for_server(proxy_setting.as_deref(), &credentials.server)
                .and_then(|proxy| {
                    if let Some(proxy) = &proxy {
                        tracing::info!(
                            "reaching the server through the proxy {}",
                            proxy.redacted()
                        );
                    }
                    Tunnel::new(
                        TunnelSettings {
                            server: credentials.server,
                            device_id: credentials.device_id,
                            key: Arc::new(credentials.key),
                            store: credentials.store,
                            max_message_bytes,
                            proxy,
                            chats: Some(Arc::clone(&self.chats)),
                            notifications: Some(Arc::clone(&self.notifications)),
                        },
                        self.organization.is_none().then_some(files),
                        Arc::clone(&self.audit),
                        self.status.clone(),
                    )
                });
            let tunnel = match tunnel {
                Ok(tunnel) => {
                    *self.chat_client.lock().await =
                        Some(Arc::new(ChatClient::new(tunnel.tokens())));
                    tunnel
                }
                Err(e) => {
                    *self.chat_client.lock().await = None;
                    // A proxy setting that can't work: wait for a fixed config.
                    tracing::error!("can't reach the server: {e:#}");
                    self.status.set(Connection::Offline, Some(format!("{e:#}")));
                    self.wait_for_reload(shutdown).await;
                    continue;
                }
            };
            if tunnel.run(token).await == TunnelExit::Revoked {
                // Nothing runs for a device that's gone.
                self.runner.stop_everything().await;
                self.wait_for_reload(shutdown).await;
            }
        }
    }

    async fn wait_for_reload(&self, shutdown: &CancellationToken) {
        tokio::select! {
            () = shutdown.cancelled() => {}
            () = self.reloaded.notified() => {}
        }
    }

    async fn reload(&self) -> Result<()> {
        if self.organization.is_some() {
            self.tunnel_token.lock().await.cancel();
            self.reloaded.notify_one();
            return Ok(());
        }
        let paths = self.paths.clone();
        let new = Config::load(&paths)?;
        let roots: Vec<std::path::PathBuf> = new.roots.iter().map(|r| r.path.clone()).collect();
        // A folder that no longer allows changes is read-only from now on,
        // not only once the daemon has restarted under a new sandbox.
        let still_writable: Vec<String> = new
            .roots
            .iter()
            .filter(|r| r.writable)
            .map(|r| r.id.clone())
            .collect();
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || writer.stop_changes(&still_writable)).await?;
        if crate::sandbox::plan().is_some_and(|plan| !plan.covers(&roots, &writable_roots(&new))) {
            // The kernel won't let this process read the new folder, or
            // its write rights don't match the folders that allow changes.
            // Exit and let the supervisor start a worker whose sandbox fits.
            let mut entry = AuditEntry::event("restarting");
            entry.detail = Some("to confine the daemon to the new set of folders".into());
            self.audit.append(&entry);
            RESTART_FOR_SANDBOX.store(true, Ordering::SeqCst);
            self.shutdown.cancel();
            return Ok(());
        }
        let reader = Arc::clone(&self.reader);
        let (cfg, p) = (new.clone(), paths.clone());
        tokio::task::spawn_blocking(move || reader.reload(&cfg, &p)).await??;
        self.limiter.set_limits(new.limits.clone());
        self.paused.store(new.paused, Ordering::SeqCst);
        let mut config = self.config.lock().await;
        // A new set of folders that allow changes changes the tools offered:
        // reconnect, so the server lists them again.
        let pairing_changed = config.server != new.server
            || config.secret_store != new.secret_store
            || config.proxy != new.proxy
            || writable_roots(&config) != writable_roots(&new)
            || command_roots(&config) != command_roots(&new);
        *config = new;
        drop(config);
        // Runs in folders that no longer allow commands stop now.
        self.runner.prune().await;
        if pairing_changed {
            self.tunnel_token.lock().await.cancel();
        }
        self.reloaded.notify_one();
        self.audit.append(&AuditEntry::event("reloaded"));
        self.changes.bump();
        Ok(())
    }

    async fn add_root(&self, new: ControlRequest) -> Result<Value> {
        let ControlRequest::RootsAdd {
            path,
            label,
            follow_symlinks,
            i_know,
            writable,
        } = new
        else {
            unreachable!("only called for roots_add");
        };
        let root = self
            .edit_config(move |config, paths| {
                add_root(
                    config,
                    paths,
                    NewRoot {
                        path: &path,
                        label,
                        i_know,
                        follow_symlinks,
                        writable,
                    },
                )
            })
            .await?;
        let mut entry = AuditEntry::event("root_added");
        entry.detail = Some(format!(
            "{} ({}){}",
            root.id,
            root.label,
            if root.writable {
                ", changes allowed"
            } else {
                ""
            }
        ));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    /// Allow or stop changes in a folder. Only this computer's user can:
    /// the request comes over the control channel, never the tunnel.
    async fn set_writable(&self, which: String, writable: bool, i_know: bool) -> Result<Value> {
        let root = self
            .edit_config(move |config, _| set_writable(config, &which, writable, i_know))
            .await?;
        let mut entry = AuditEntry::event(if writable {
            "root_changes_allowed"
        } else {
            "root_changes_stopped"
        });
        entry.detail = Some(format!("{} ({})", root.id, root.label));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    async fn set_commands(&self, which: String, commands: bool, network: bool) -> Result<Value> {
        let root = self
            .edit_config(move |config, _| {
                crate::roots::set_commands(config, &which, commands, network)
            })
            .await?;
        let mut entry = AuditEntry::event(if commands {
            "root_commands_allowed"
        } else {
            "root_commands_stopped"
        });
        entry.detail = Some(format!(
            "{} ({}){}",
            root.id,
            root.label,
            if root.network { ", with network" } else { "" }
        ));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    async fn set_bot(
        &self,
        which: String,
        bot: String,
        name: Option<String>,
        lent: bool,
        unattended: bool,
    ) -> Result<Value> {
        let id = bot.clone();
        let root = self
            .edit_config(move |config, _| {
                crate::roots::set_bot(config, &which, &bot, name.as_deref(), lent, unattended)
            })
            .await?;
        let mut entry = AuditEntry::event(if lent {
            "root_bot_lent"
        } else {
            "root_bot_withdrawn"
        });
        let unattended = root.loan(&id).is_some_and(|loan| loan.unattended);
        entry.detail = Some(format!(
            "{} ({}) to bot {id}{}",
            root.id,
            root.label,
            if unattended {
                ", its commands unattended"
            } else {
                ""
            }
        ));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    async fn remove_root(&self, which: String) -> Result<Value> {
        let root = self
            .edit_config(move |config, _| remove_root(config, &which))
            .await?;
        let mut entry = AuditEntry::event("root_removed");
        entry.detail = Some(format!("{} ({})", root.id, root.label));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    async fn suggested_roots(&self) -> Value {
        let config = self.config.lock().await.clone();
        let suggestions: Vec<Value> = crate::paths::documents_dir()
            .into_iter()
            .map(|path| {
                let real = path.canonicalize().ok();
                let shared = config
                    .roots
                    .iter()
                    .any(|r| Some(&r.path) == real.as_ref() || r.path == path);
                json!({
                    "path": path,
                    "label": "Documents",
                    "exists": path.is_dir(),
                    "shared": shared,
                })
            })
            .collect();
        json!({ "suggestions": suggestions })
    }

    /// Run one chat API call on a blocking thread, as this device.
    async fn chat<F>(&self, call: F) -> Result<Value>
    where
        F: FnOnce(&ChatClient) -> Result<Value> + Send + 'static,
    {
        let client = self.chat_client.lock().await.clone().ok_or_else(|| {
            Refusal::new(
                "not_paired",
                "This computer isn't paired with Chat with Work. Pair it with cww login.",
            )
        })?;
        tokio::task::spawn_blocking(move || call(&client)).await?
    }

    async fn send_question(&self, question: Question) -> Result<Value> {
        let mut question = question;
        question.text = question.text.trim().to_string();
        // Files can go without a word, as in the composer.
        if question.text.is_empty() && question.attachments.is_empty() {
            return Err(Refusal::new("invalid", "Write a question first").into());
        }
        if question.text.chars().count() > MAX_QUESTION_CHARS {
            return Err(Refusal::new(
                "invalid",
                format!("Questions can be at most {MAX_QUESTION_CHARS} characters"),
            )
            .into());
        }
        if let Some(chat) = &question.chat {
            chat_number(chat)?;
        }
        if let Some(model) = &question.model {
            plain_token(model, 200, "model")?;
        }
        if question.attachments.len() > MAX_ATTACHMENTS {
            return Err(Refusal::new(
                "invalid",
                format!("A question can carry at most {MAX_ATTACHMENTS} files"),
            )
            .into());
        }
        for attachment in &question.attachments {
            plain_token(attachment, 2048, "attachment")?;
        }
        self.chat(move |client| client.send(&question)).await
    }

    /// Sends a scan the desktop app made: into a new chat (an upload, then
    /// the chat), or to a bot's trigger.
    #[allow(clippy::too_many_arguments)]
    async fn send_scan(
        &self,
        destination: crate::scans::Destination,
        filename: String,
        data: String,
        pages: u32,
        scan_id: String,
        scanner: Option<String>,
        message: Option<String>,
    ) -> Result<Value> {
        use crate::scans::{Destination, MAX_MESSAGE_CHARS, Scan};
        let upload = decode_upload(filename, Some("application/pdf".into()), data)?;
        let scan = Scan {
            filename: upload.filename,
            pdf: upload.data,
            pages,
            scan_id,
            scanner,
        }
        .check()?;
        match destination {
            Destination::Chat { project } => {
                let text = message.unwrap_or_default().trim().to_string();
                if text.chars().count() > MAX_MESSAGE_CHARS {
                    return Err(Refusal::new(
                        "invalid",
                        format!("Say it in at most {MAX_MESSAGE_CHARS} characters"),
                    )
                    .into());
                }
                self.chat(move |client| {
                    let uploaded = client.upload(&Upload {
                        filename: scan.filename,
                        content_type: "application/pdf".into(),
                        data: scan.pdf,
                    })?;
                    let signed_id = uploaded["signed_id"]
                        .as_str()
                        .ok_or_else(|| {
                            Refusal::new("unsupported", "The server didn't take the scan")
                        })?
                        .to_string();
                    client.send(&Question {
                        text,
                        project,
                        attachments: vec![signed_id],
                        ..Question::default()
                    })
                })
                .await
            }
            Destination::Trigger { id } => {
                self.chat(move |client| client.send_scan(id, &scan)).await
            }
        }
    }

    /// Upload a file a client read for a question.
    async fn upload(
        &self,
        filename: String,
        content_type: Option<String>,
        data: String,
    ) -> Result<Value> {
        let upload = decode_upload(filename, content_type, data)?;
        self.chat(move |client| client.upload(&upload)).await
    }

    /// Load config.toml, change it with `edit`, and save it, one request at
    /// a time: two requests editing at once would each save over the
    /// other's change.
    async fn edit_config<T: Send + 'static>(
        &self,
        edit: impl FnOnce(&mut Config, &Paths) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let _editing = self.config_edits.lock().await;
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || {
            let mut config = Config::load(&paths)?;
            let edited = edit(&mut config, &paths)?;
            config.save(&paths)?;
            Ok(edited)
        })
        .await?
    }

    async fn set_paused(&self, paused: bool) -> Result<()> {
        self.paused.store(paused, Ordering::SeqCst);
        // Edited on disk, not saved from memory: a folder just added may
        // not be in memory until its reload.
        self.edit_config(move |config, _| {
            config.paused = paused;
            Ok(())
        })
        .await?;
        self.config.lock().await.paused = paused;
        // Commands stop with the computer.
        if paused {
            self.runner.prune().await;
        }
        self.audit.append(&AuditEntry::event(if paused {
            "paused"
        } else {
            "resumed"
        }));
        self.changes.bump();
        Ok(())
    }

    /// The commands this computer ran, each with its folder's label.
    async fn commands_json(&self) -> Result<Value> {
        let list = self.runner.list().await.map_err(command_refusal)?;
        let config = self.config.lock().await.clone();
        let commands: Vec<Value> = list
            .into_iter()
            .map(|summary| {
                let folder = config
                    .roots
                    .iter()
                    .find(|r| r.id == summary.root)
                    .map_or_else(|| summary.root.clone(), |r| r.label.clone());
                let mut value = serde_json::to_value(&summary).unwrap_or_default();
                value["folder"] = Value::String(folder);
                value
            })
            .collect();
        Ok(json!({ "commands": commands }))
    }

    async fn status_json(&self) -> Value {
        let config = self.config.lock().await.clone();
        let reader = Arc::clone(&self.reader);
        let roots = tokio::task::spawn_blocking(move || reader.roots())
            .await
            .unwrap_or_default();
        let roots: Vec<Value> = roots
            .into_iter()
            .map(|r| {
                let configured = config.root(&r.id);
                let path = configured.map(|c| c.path.display().to_string());
                let mut v = serde_json::to_value(&r).unwrap_or_default();
                v["local_path"] = json!(path);
                v["bots"] = json!(configured.map(|c| c.bots.clone()).unwrap_or_default());
                v
            })
            .collect();
        json!({
            "protocol": PROTOCOL_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "pid": std::process::id(),
            "connection": self.status.get(),
            "paired": config.server.is_some(),
            "chat_only": self.organization.is_some(),
            "account": self.organization.as_ref().map(|context| &context.profile.account),
            "sandbox": crate::sandbox::status(),
            "config_file": self.paths.config_file(),
            "audit_file": self.paths.audit_file(),
            "server": config.server.as_ref().map(|s| &s.url),
            "device_id": config.server.as_ref().map(|s| &s.device_id),
            "proxy": proxy_info(&config),
            "paused": self.paused.load(Ordering::SeqCst),
            "roots": roots,
        })
    }

    async fn label_root(&self, which: String, label: String) -> Result<Value> {
        let label = label.trim().to_string();
        if label.is_empty() {
            bail!("the label can't be empty");
        }
        if label.chars().count() > 80 {
            bail!("the label is longer than 80 characters");
        }
        let root = self
            .edit_config(move |config, _| {
                let root = config
                    .roots
                    .iter_mut()
                    .find(|r| r.id == which)
                    .with_context(|| format!("no shared folder has the ID {which:?}"))?;
                root.label = label;
                Ok(root.clone())
            })
            .await?;
        let mut entry = AuditEntry::event("root_labeled");
        entry.detail = Some(format!("{} ({})", root.id, root.label));
        self.audit.append(&entry);
        self.reload().await?;
        Ok(json!({ "root": root }))
    }

    async fn deny(&self) -> Value {
        let config = self.config.lock().await.clone();
        let removed = &config.deny.remove;
        let builtin: Vec<&str> = DEFAULT_DENY
            .iter()
            .copied()
            .filter(|p| {
                !removed
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(p.trim_end_matches('/')))
            })
            .collect();
        let own: Vec<PathBuf> = self
            .paths
            .all_dirs()
            .iter()
            .map(|p| p.to_path_buf())
            .collect();
        let never_changed = crate::policy::never_changed(&config.deny.remove);
        json!({
            "builtin": builtin,
            "never_changed": never_changed,
            "extra": config.deny.extra,
            "removed": removed,
            "own_dirs": own,
            "allow_hardlinks": config.deny.allow_hardlinks,
            "config_file": self.paths.config_file(),
        })
    }
}

/// The proxy the tunnel uses for the paired server, without its password.
/// `null` for a direct connection, or when the setting is invalid (the
/// connection's `last_error` says why).
fn proxy_info(config: &Config) -> Option<crate::proxy::ProxyInfo> {
    let server = crate::auth::client::ServerUrl::parse(&config.server.as_ref()?.url).ok()?;
    crate::proxy::for_server(config.proxy.as_deref(), &server)
        .ok()
        .flatten()
        .map(|p| p.info())
}

impl ControlHandler for Daemon {
    fn events(&self) -> Option<broadcast::Receiver<ControlEvent>> {
        Some(self.events.subscribe())
    }

    async fn initial_status(&self) -> Option<Value> {
        Some(self.status_json().await)
    }

    async fn watch_chat(&self, chat: &str) -> Result<()> {
        chat_number(chat)?;
        if self.chat_client.lock().await.is_none() {
            return Err(Refusal::new(
                "not_paired",
                "This computer isn't paired with Chat with Work. Pair it with cww login.",
            )
            .into());
        }
        self.chats.watch(chat);
        Ok(())
    }

    fn unwatch_chat(&self, chat: &str) {
        self.chats.unwatch(chat);
    }

    async fn watch_chats(&self) -> Result<()> {
        if self.chat_client.lock().await.is_none() {
            return Err(Refusal::new(
                "not_paired",
                "This computer isn't paired with Chat with Work. Pair it with cww login.",
            )
            .into());
        }
        self.chats.watch_list();
        Ok(())
    }

    fn unwatch_chats(&self) {
        self.chats.unwatch_list();
    }

    async fn handle(&self, request: ControlRequest) -> Result<Value> {
        if self.organization.is_some()
            && matches!(
                request,
                ControlRequest::RootsAdd { .. }
                    | ControlRequest::RootsRemove { .. }
                    | ControlRequest::RootsWritable { .. }
                    | ControlRequest::RootsCommands { .. }
                    | ControlRequest::RootsBot { .. }
                    | ControlRequest::RootsLabel { .. }
                    | ControlRequest::Pause
                    | ControlRequest::Resume
                    | ControlRequest::SuggestedRoots
                    | ControlRequest::Deny
            )
        {
            return Err(Refusal::new(
                "chat_only",
                "Manage shared folders from this computer's folder settings.",
            )
            .into());
        }
        match request {
            ControlRequest::Hello => Ok(json!({
                "protocol": PROTOCOL_VERSION,
                "version": env!("CARGO_PKG_VERSION"),
                "platform": std::env::consts::OS,
                "pid": std::process::id(),
            })),
            ControlRequest::Status => Ok(self.status_json().await),
            ControlRequest::NotificationStatus => {
                Ok(serde_json::to_value(self.notifications.status())?)
            }
            ControlRequest::NotificationPermission { request } => {
                self.notifications.permission(request)?;
                Ok(serde_json::to_value(self.notifications.status())?)
            }
            ControlRequest::NotificationView {
                session,
                account,
                chat,
                visible,
            } => {
                self.notifications.view(session, account, chat, visible)?;
                Ok(json!({}))
            }
            ControlRequest::Pause => {
                self.set_paused(true).await?;
                Ok(json!({ "paused": true }))
            }
            ControlRequest::Resume => {
                self.set_paused(false).await?;
                Ok(json!({ "paused": false }))
            }
            ControlRequest::Reload => {
                self.reload().await?;
                Ok(json!({}))
            }
            ControlRequest::Shutdown => {
                let mut entry = AuditEntry::event("shutdown_requested");
                entry.detail = Some("over the control channel".into());
                self.audit.append(&entry);
                // Answer first, then stop.
                let shutdown = self.shutdown.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    shutdown.cancel();
                });
                Ok(json!({ "stopping": true }))
            }
            request @ ControlRequest::RootsAdd { .. } => self.add_root(request).await,
            ControlRequest::RootsRemove { root } => self.remove_root(root).await,
            ControlRequest::RootsWritable {
                root,
                writable,
                i_know,
            } => self.set_writable(root, writable, i_know).await,
            ControlRequest::RootsCommands {
                root,
                commands,
                network,
            } => self.set_commands(root, commands, network).await,
            ControlRequest::RootsBot {
                root,
                bot,
                name,
                lent,
                unattended,
            } => self.set_bot(root, bot, name, lent, unattended).await,
            ControlRequest::Commands => self.commands_json().await,
            ControlRequest::CommandOutput { run, offset, max } => {
                let max = max.unwrap_or(crate::commands::MAX_OUTPUT_CHUNK);
                let output = self
                    .runner
                    .output(&run, offset, max)
                    .await
                    .map_err(command_refusal)?;
                Ok(serde_json::to_value(output)?)
            }
            ControlRequest::CommandStop { run } => {
                let status = self.runner.stop(&run).await.map_err(command_refusal)?;
                let mut entry = AuditEntry::event("command_stopped");
                entry.detail = Some(format!("run {}", status.id));
                self.audit.append(&entry);
                Ok(serde_json::to_value(status)?)
            }
            ControlRequest::AuditTail { lines } => {
                let path = self.audit.path().to_path_buf();
                let n = lines.unwrap_or(50).min(1000);
                let entries =
                    tokio::task::spawn_blocking(move || crate::audit::tail(&path, n)).await??;
                Ok(json!({ "entries": entries }))
            }
            ControlRequest::SuggestedRoots => Ok(self.suggested_roots().await),
            ControlRequest::Chats => self.chat(|client| client.list()).await,
            ControlRequest::NotificationPreferences => {
                self.chat(|client| client.notification_preferences()).await
            }
            ControlRequest::NotificationPreferencesUpdate { preferences } => {
                self.chat(move |client| client.update_notification_preferences(preferences))
                    .await
            }
            ControlRequest::NotificationBrowserLink => {
                self.chat(|client| client.notification_browser_link()).await
            }
            ControlRequest::Workspace { project } => {
                self.chat(move |client| client.workspace(project)).await
            }
            ControlRequest::ServicePause { kind, key, paused } => {
                self.chat(move |client| client.service_pause(kind, &key, paused))
                    .await
            }
            ControlRequest::Instructions { project } => {
                self.chat(move |client| client.instructions(project)).await
            }
            ControlRequest::MemoryPreference { enabled } => {
                self.chat(move |client| client.memory_preference(enabled))
                    .await
            }
            ControlRequest::MemoryForget { id } => {
                self.chat(move |client| client.memory_forget(id)).await
            }
            ControlRequest::InstructionCount { text } => {
                self.chat(move |client| client.instruction_count(&text))
                    .await
            }
            ControlRequest::ComputerRemove { id } => {
                self.chat(move |client| client.computer_remove(id)).await
            }
            ControlRequest::Connectors { query, category } => {
                self.chat(move |client| client.connectors(&query, category.as_deref()))
                    .await
            }
            ControlRequest::ConnectorHandoff {
                key,
                access,
                connection,
            } => {
                self.chat(move |client| {
                    client.connector_handoff(&key, &access, connection.as_deref())
                })
                .await
            }
            ControlRequest::InstructionUpdate {
                project,
                layer,
                change,
            } => {
                self.chat(move |client| client.instruction_update(project, layer, change))
                    .await
            }
            ControlRequest::Chat { chat } => self.chat(move |client| client.show(&chat)).await,
            ControlRequest::ChatSend {
                chat,
                text,
                project,
                model,
                web_search,
                attachments,
                steer,
            } => {
                self.send_question(Question {
                    chat,
                    text,
                    project,
                    model,
                    web_search,
                    attachments,
                    steer,
                })
                .await
            }
            ControlRequest::ChatUnqueue { chat, queued } => {
                self.chat(move |client| client.unqueue(&chat, &queued))
                    .await
            }
            ControlRequest::Models => self.chat(|client| client.models()).await,
            ControlRequest::Templates { request } => {
                self.chat(move |client| client.templates(&request)).await
            }
            ControlRequest::TemplateUpload {
                target,
                filename,
                content_type,
                data,
            } => {
                let upload = decode_upload(filename, content_type, data)?;
                self.chat(move |client| client.upload_template(&target, &upload))
                    .await
            }
            ControlRequest::ModelPreferences => {
                self.chat(|client| client.model_preferences()).await
            }
            ControlRequest::Schedules { request } => {
                self.chat(move |client| client.schedules(&request)).await
            }
            ControlRequest::People { request } => {
                self.chat(move |client| client.people(&request)).await
            }
            ControlRequest::AccountSettings { request } => {
                self.chat(move |client| client.account_settings(&request))
                    .await
            }
            ControlRequest::ModelPreferencesUpdate { account, model_id } => {
                self.chat(move |client| {
                    client.update_model_preferences(&account, model_id.as_deref())
                })
                .await
            }
            ControlRequest::ChatApprove {
                chat,
                tool_call,
                for_rest_of_chat,
            } => {
                self.chat(move |client| client.approve(&chat, &tool_call, for_rest_of_chat))
                    .await
            }
            ControlRequest::ChatDeny {
                chat,
                tool_call,
                reason,
            } => {
                if reason
                    .as_ref()
                    .is_some_and(|r| r.chars().count() > MAX_REASON_CHARS)
                {
                    return Err(Refusal::new(
                        "invalid",
                        format!("Say it in at most {MAX_REASON_CHARS} characters"),
                    )
                    .into());
                }
                self.chat(move |client| client.deny(&chat, &tool_call, reason.as_deref()))
                    .await
            }
            ControlRequest::ChatUpload {
                filename,
                content_type,
                data,
            } => self.upload(filename, content_type, data).await,
            ControlRequest::ScanDestinations => {
                self.chat(|client| client.scan_destinations()).await
            }
            ControlRequest::ScanSend {
                destination,
                filename,
                data,
                pages,
                scan_id,
                scanner,
                message,
            } => {
                self.send_scan(
                    destination,
                    filename,
                    data,
                    pages,
                    scan_id,
                    scanner,
                    message,
                )
                .await
            }
            ControlRequest::ChatDownload { path } => {
                self.chat(move |client| client.download(&path)).await
            }
            ControlRequest::Dictation => self.chat(|client| client.dictation()).await,
            ControlRequest::Dictate { content_type, data } => {
                use base64::Engine;
                if data.len() > crate::chats::MAX_AUDIO_BYTES.div_ceil(3) * 4 {
                    return Err(Refusal::new("too_large", "Record at most five minutes").into());
                }
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| Refusal::new("invalid", "The recording isn't valid base64"))?;
                let content_type = plain_token(&content_type, 200, "content type")?.to_string();
                self.chat(move |client| {
                    client.dictate(&Upload {
                        filename: "recording".into(),
                        content_type,
                        data,
                    })
                })
                .await
            }
            ControlRequest::ChatRetry { chat, message } => {
                self.chat(move |client| client.retry(&chat, message.as_deref()))
                    .await
            }
            ControlRequest::ChatBranch { chat, message } => {
                self.chat(move |client| client.branch(&chat, message.as_deref()))
                    .await
            }
            ControlRequest::ChatRename { chat, title } => {
                let title = title.trim().to_string();
                if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
                    return Err(Refusal::new(
                        "invalid",
                        format!("A title needs 1 to {MAX_TITLE_CHARS} characters"),
                    )
                    .into());
                }
                self.chat(move |client| client.rename(&chat, &title)).await
            }
            ControlRequest::ChatDelete { chat } => {
                self.chat(move |client| client.delete(&chat)).await
            }
            ControlRequest::ChatPin { chat, pinned } => {
                self.chat(move |client| client.pin(&chat, pinned)).await
            }
            ControlRequest::ChatHistory { query, before } => {
                self.chat(move |client| client.history(&query, before.as_deref()))
                    .await
            }
            ControlRequest::ChatMove { chat, project } => {
                self.chat(move |client| client.move_to(&chat, project))
                    .await
            }
            ControlRequest::Projects { request } => {
                self.chat(move |client| client.projects(&request)).await
            }
            ControlRequest::ChatShare { chat } => {
                self.chat(move |client| client.share(&chat)).await
            }
            ControlRequest::ChatUnshare { chat } => {
                self.chat(move |client| client.unshare(&chat)).await
            }
            ControlRequest::ChatAnswer {
                chat,
                tool_call,
                input,
            } => {
                if input.as_ref().is_some_and(|i| !i.is_object()) {
                    return Err(Refusal::new(
                        "bad_request",
                        "bad request: input is the form's fields, by name",
                    )
                    .into());
                }
                self.chat(move |client| client.answer(&chat, &tool_call, input.as_ref()))
                    .await
            }
            ControlRequest::ChatDecline { chat, tool_call } => {
                self.chat(move |client| client.decline(&chat, &tool_call))
                    .await
            }
            ControlRequest::ChatCancel { chat } => {
                self.chat(move |client| client.cancel(&chat)).await
            }
            ControlRequest::ChatAccess => self.chat(|client| client.request_access()).await,
            ControlRequest::Asset { path } => self.chat(move |client| client.asset(&path)).await,
            ControlRequest::RootsLabel { root, label } => self.label_root(root, label).await,
            ControlRequest::Deny => Ok(self.deny().await),
            ControlRequest::Subscribe { .. } => {
                anyhow::bail!("subscribe is handled by the control channel")
            }
        }
    }
}

fn decode_upload(filename: String, content_type: Option<String>, data: String) -> Result<Upload> {
    if data.len() > MAX_UPLOAD_BYTES.div_ceil(3) * 4 {
        return Err(Refusal::new("attachment_refused", "Files can be at most 25 MB").into());
    }
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| Refusal::new("bad_request", "bad request: data is not base64"))?;
    if data.is_empty() {
        return Err(Refusal::new("attachment_refused", "That file is empty").into());
    }
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(Refusal::new(
            "attachment_refused",
            format!(
                "Files can be at most {} MB",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            ),
        )
        .into());
    }
    Ok(Upload {
        filename: upload_filename(&filename),
        content_type: match content_type {
            Some(t) => plain_token(&t, 200, "content type")?.to_string(),
            None => "application/octet-stream".into(),
        },
        data,
    })
}

#[cfg(windows)]
async fn wait_for_signal() {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
    let (Ok(mut c), Ok(mut brk), Ok(mut close), Ok(mut logoff), Ok(mut shutdown)) = (
        ctrl_c(),
        ctrl_break(),
        ctrl_close(),
        ctrl_logoff(),
        ctrl_shutdown(),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = c.recv() => {}
        _ = brk.recv() => {}
        _ = close.recv() => {}
        _ = logoff.recv() => {}
        _ = shutdown.recv() => {}
    }
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploaded_names_lose_their_directories_and_control_characters() {
        assert_eq!(upload_filename("/home/me/Q3 plan.pdf"), "Q3 plan.pdf");
        assert_eq!(upload_filename(r"C:\Users\me\notes.txt"), "notes.txt");
        assert_eq!(upload_filename("a\r\nb\".txt"), "ab.txt");
        for empty in ["", "..", "/", "\n"] {
            assert_eq!(upload_filename(empty), "file", "{empty:?}");
        }
    }

    #[test]
    fn ids_passed_to_the_server_are_plain() {
        assert!(plain_token("claude-sonnet-4.5", 200, "model").is_ok());
        assert!(plain_token("text/plain", 200, "content type").is_ok());
        for bad in ["", "a b", "a\nb", "x\r\nHost: evil"] {
            assert!(plain_token(bad, 200, "model").is_err(), "{bad:?}");
        }
    }
}
