//! The bundled, fixed-method browser engine. Its private pipe is never a tool.
//!
//! Installation is verified before confinement; the child inherits that
//! confinement. Missing/incompatible installations choose the legacy path
//! before a browser operation starts, never after a possible input dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU8, Ordering},
};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use ring::digest::{Context, SHA256};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::error::{ErrorCode, ToolError};
use crate::paths::Paths;

use super::bridge::{Bridge, EngineSession};

const NODE_VERSION: &str = "22.23.4";
const ENGINE_VERSION: &str = "1.59.13";
const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_TOTAL: usize = 16 * 1024 * 1024;
// First launch on Windows can spend over 20 seconds in OS inspection.
// This is before any extension lease or browser input exists.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(14);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(18);
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(500);

static INSTALLATION: OnceLock<Option<Arc<Installation>>> = OnceLock::new();

#[derive(Debug)]
pub struct Installation {
    pub root: PathBuf,
    pub executable: PathBuf,
    entry: PathBuf,
    // 0 unstarted, 1 warming, 2 ready, 3 unavailable until daemon restart.
    readiness: AtomicU8,
}

pub(crate) struct Route {
    installation: Arc<Installation>,
    // Shared by an automatic dry run and its action, under Rails' 20 seconds.
    deadline: tokio::time::Instant,
}

struct ReadyChild {
    _child: ChildGuard,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

struct ChildGuard {
    child: Child,
    stderr: tokio::task::JoinHandle<()>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stderr.abort();
        // Child itself has kill_on_drop; only this operation's PID is ended.
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol: u64,
    app_version: String,
    node_version: String,
    engine_version: String,
    target: String,
    #[serde(rename = "node_support_ends")]
    _node_support_ends: String,
    files: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    protocol: u64,
    directory: String,
}

/// This runs once, before the daemon's sandbox starts, not from a tool.
pub fn initialize(config: &Config, paths: &Paths) {
    INSTALLATION.get_or_init(|| {
        let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
        let parent = executable.parent()?;
        let candidates = vec![
            parent.join("browser-engine"),
            parent.join("../lib/chatwithwork/browser-engine"),
            parent.join("../libexec/chatwithwork/browser-engine"),
            parent.join("../Resources/browser-engine"),
        ];
        #[cfg(windows)]
        let candidates = {
            let mut candidates = candidates;
            if msi_installation(parent) && let Some(local_data) = dirs::data_local_dir() {
                // dirs uses SHGetKnownFolderPath on Windows, not an
                // environment-selected path. MSI supports custom app paths.
                candidates.push(local_data.join("Programs/CwwBE"));
            }
            candidates
        };
        for candidate in candidates {
            if !candidate.exists() {
                continue;
            }
            match Installation::load(&candidate, config, paths) {
                Ok(installation) => return Some(Arc::new(installation)),
                Err(_) => tracing::warn!(
                    "browser engine installation is incomplete, incompatible or inside writable shared data; using legacy browser actions"
                ),
            }
        }
        None
    });
}

#[cfg(any(windows, test))]
fn msi_installation(parent: &Path) -> bool {
    let marker = parent.join("cww-installer.txt");
    marker
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 64)
        && std::fs::read(marker).is_ok_and(|bytes| {
            matches!(
                bytes.as_slice(),
                b"cww-installer-v1\n" | b"cww-installer-v1\r\n" | b"cww-installer-v1"
            )
        })
}

pub fn installed() -> Option<Arc<Installation>> {
    INSTALLATION.get().and_then(Clone::clone)
}

/// A portable code directory newly shared with write access must be rejected
/// by fresh admission, even when the daemon sandbox was explicitly disabled.
pub fn requires_restart(config: &Config, paths: &Paths) -> bool {
    installed()
        .is_some_and(|installation| !separate_from_writable(&installation.root, config, paths))
}

fn separate_from_writable(root: &Path, config: &Config, paths: &Paths) -> bool {
    config
        .roots
        .iter()
        .filter(|r| r.writable)
        .map(|r| r.path.as_path())
        .chain(paths.all_dirs())
        .all(|writable| {
            let writable = writable
                .canonicalize()
                .unwrap_or_else(|_| writable.to_path_buf());
            !root.starts_with(&writable) && !writable.starts_with(root)
        })
}

/// Start only trusted imports under the real confinement, without browser
/// access or a lease. Slow first-run OS inspection is outside tool requests.
pub fn prewarm(shutdown: CancellationToken) {
    let Some(installation) = installed() else {
        return;
    };
    if installation
        .readiness
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tokio::spawn(installation.warm(shutdown));
}

impl Installation {
    async fn warm(self: Arc<Self>, shutdown: CancellationToken) {
        let ready = tokio::select! {
            biased;
            _ = shutdown.cancelled() => false,
            result = tokio::time::timeout(STARTUP_TIMEOUT, self.start()) => matches!(result, Ok(Ok(_))),
        };
        self.readiness
            .store(if ready { 2 } else { 3 }, Ordering::Release);
        if ready {
            tracing::info!("bundled browser engine is ready");
        } else {
            tracing::warn!(
                "bundled browser runtime did not become ready under confinement; using legacy browser actions until daemon restart"
            );
        }
    }
}

pub(crate) fn route(bridge: &Bridge, method: &str) -> Option<Route> {
    if !matches!(method, "click" | "type" | "select") || bridge.engine_session().is_none() {
        return None;
    }
    let installation = installed()?;
    (installation.readiness.load(Ordering::Acquire) == 2).then(|| Route {
        installation,
        deadline: tokio::time::Instant::now() + REQUEST_TIMEOUT,
    })
}

pub(crate) async fn call(
    bridge: &Arc<Bridge>,
    route: Option<&Route>,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match route {
        Some(route) => {
            tracing::debug!("using bundled browser engine");
            let session = bridge.engine_session().ok_or_else(failed)?;
            execute(
                bridge,
                &route.installation,
                session,
                method,
                params,
                route.deadline,
            )
            .await
        }
        None => bridge.call(method, params).await,
    }
}

impl Installation {
    fn load(container: &Path, config: &Config, paths: &Paths) -> anyhow::Result<Self> {
        let metadata = container.symlink_metadata()?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "runtime container symlink"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                metadata.permissions().mode() & 0o022 == 0,
                "runtime container writable by others"
            );
        }
        let locator_path = container.join("current.json");
        let metadata = locator_path.symlink_metadata()?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= 1024,
            "invalid runtime locator"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                metadata.permissions().mode() & 0o022 == 0,
                "runtime locator writable by others"
            );
        }
        let locator: Locator = serde_json::from_slice(&std::fs::read(locator_path)?)?;
        anyhow::ensure!(
            locator.protocol == 1
                && locator.directory.len() == 64
                && locator
                    .directory
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid runtime directory"
        );
        let payload = container.join(&locator.directory);
        let metadata = payload.symlink_metadata()?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "runtime payload symlink"
        );
        let manifest_path = payload.join("manifest.json");
        let metadata = manifest_path.symlink_metadata()?;
        anyhow::ensure!(metadata.is_file() && metadata.len() <= MAX_FRAME as u64);
        let actual: String = ring::digest::digest(&SHA256, &std::fs::read(&manifest_path)?)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        anyhow::ensure!(
            actual == locator.directory,
            "runtime directory differs from manifest"
        );
        Self::verify(&payload, config, paths)
    }

    fn verify(root: &Path, config: &Config, paths: &Paths) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !root.symlink_metadata()?.file_type().is_symlink(),
            "runtime root symlink"
        );
        let root = root.canonicalize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                root.metadata()?.permissions().mode() & 0o022 == 0,
                "runtime root writable by others"
            );
        }
        anyhow::ensure!(
            separate_from_writable(&root, config, paths),
            "runtime overlaps writable data"
        );
        let manifest_path = root.join("manifest.json");
        let metadata = manifest_path.symlink_metadata()?;
        anyhow::ensure!(metadata.is_file() && metadata.len() <= MAX_FRAME as u64);
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
        anyhow::ensure!(
            manifest.protocol == 1
                && manifest.app_version == env!("CARGO_PKG_VERSION")
                && manifest.node_version == NODE_VERSION
                && manifest.engine_version == ENGINE_VERSION
                && target_matches(&manifest.target)
                && manifest.files.len() <= 10_000,
            "incompatible runtime manifest"
        );
        let executable = root.join(if cfg!(windows) { "node.exe" } else { "node" });
        let entry = root.join("entry.mjs");
        for required in [
            executable.file_name().unwrap().to_str().unwrap(),
            "entry.mjs",
            "node_modules/@xmorse/playwright-core/package.json",
        ] {
            anyhow::ensure!(
                manifest.files.contains_key(required),
                "missing runtime file"
            );
        }
        let mut actual = BTreeSet::new();
        collect_files(&root, &root, &mut actual)?;
        actual.remove("manifest.json");
        anyhow::ensure!(
            actual == manifest.files.keys().cloned().collect(),
            "unlisted runtime file"
        );
        for (relative, expected) in manifest.files {
            let path = Path::new(&relative);
            anyhow::ensure!(
                !path.as_os_str().is_empty()
                    && path.components().all(|c| matches!(c, Component::Normal(_)))
                    && !relative.contains('\\'),
                "invalid runtime path"
            );
            anyhow::ensure!(
                expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit())
            );
            let mut file = std::fs::File::open(root.join(path))?;
            let mut hash = Context::new(&SHA256);
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let size = file.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                hash.update(&buffer[..size]);
            }
            let actual: String = hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            anyhow::ensure!(
                actual.eq_ignore_ascii_case(&expected),
                "runtime checksum mismatch"
            );
        }
        Ok(Self {
            root,
            executable,
            entry,
            readiness: AtomicU8::new(0),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .env_clear()
            .current_dir(&self.root)
            .kill_on_drop(true);
        // Windows needs its system directory for ordinary OS services. None
        // of the Node/module/loader environment is inherited.
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Landlock permits reading /dev/null, not writing it. Drain
            // diagnostics privately rather than widening that kernel rule.
            .stderr(Stdio::piped());
        command
    }

    fn unavailable(&self) -> ToolError {
        self.readiness.store(3, Ordering::Release);
        startup_failed()
    }

    /// The actual entry proves its loader and imports can run, not merely
    /// that a compatible executable happens to be present.
    async fn start(&self) -> Result<ReadyChild, ToolError> {
        let mut command = self.command();
        command
            .args(["--no-addons", "--no-global-search-paths"])
            .arg(&self.entry)
            .stdin(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            tracing::debug!(kind = ?error.kind(), code = error.raw_os_error(), "browser engine process could not start");
            self.unavailable()
        })?;
        let input = child.stdin.take().ok_or_else(|| self.unavailable())?;
        let mut output = BufReader::new(child.stdout.take().ok_or_else(|| self.unavailable())?);
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| self.unavailable())?
            .take(64 * 1024);
        let stderr = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        let mut child = ChildGuard { child, stderr };
        let mut ready = Vec::new();
        read_frame(&mut output, &mut ready).await.map_err(|_| {
            tracing::debug!(status = ?child.child.try_wait().ok().flatten(), "browser engine ended before ready");
            self.unavailable()
        })?;
        let handshake: Value = serde_json::from_slice(&ready).map_err(|_| self.unavailable())?;
        if handshake != json!({"type":"ready","protocol":1}) {
            return Err(self.unavailable());
        }
        Ok(ReadyChild {
            _child: child,
            input,
            output,
        })
    }
}

fn target_matches(target: &str) -> bool {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => target == "macos-universal" || target == format!("{arch}-apple-darwin"),
        "linux" => {
            target == format!("{arch}-unknown-linux-gnu")
                || target == format!("{arch}-unknown-linux-musl")
        }
        "windows" => target == format!("{arch}-pc-windows-msvc"),
        _ => false,
    }
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        directory.strip_prefix(root)?.components().count() <= 64,
        "runtime nesting limit"
    );
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.path().symlink_metadata()?;
        anyhow::ensure!(!metadata.file_type().is_symlink(), "runtime symlink");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                metadata.permissions().mode() & 0o022 == 0,
                "runtime writable by others"
            );
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else {
            anyhow::ensure!(metadata.is_file(), "runtime special file");
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative);
            anyhow::ensure!(files.len() <= 10_001, "runtime file limit");
        }
    }
    Ok(())
}

fn failed() -> ToolError {
    ToolError::new(
        ErrorCode::PageError,
        "The browser action did not finish. Its result is uncertain; read the page before deciding what to do next.",
    )
}

fn startup_failed() -> ToolError {
    ToolError::new(
        ErrorCode::BrowserUnavailable,
        "The browser engine could not start. No browser action was sent.",
    )
}

struct Lease {
    bridge: Arc<Bridge>,
    session: u64,
    lease: String,
    finished: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let bridge = Arc::clone(&self.bridge);
        let session = self.session;
        let lease = self.lease.clone();
        runtime.spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                bridge.call_in_session(session, "engine_cancel", json!({"lease":lease})),
            )
            .await;
        });
    }
}

/// There is deliberately no legacy retry here, even when input has not yet
/// been observed: only the engine/extension knows its exact dispatch point.
pub(crate) async fn execute(
    bridge: &Arc<Bridge>,
    installation: &Installation,
    mut session: EngineSession,
    action: &str,
    params: Value,
    request_deadline: tokio::time::Instant,
) -> Result<Value, ToolError> {
    // Load the fixed helper before starting an extension operation. Slow OS
    // inspection must not consume the short action deadline or hold a lease.
    if session.revision.has_changed().unwrap_or(true) {
        return Err(startup_failed());
    }
    let operation_limit = request_deadline - CLEANUP_TIMEOUT;
    if tokio::time::Instant::now() >= operation_limit {
        return Err(startup_failed());
    }
    let ReadyChild {
        _child,
        mut input,
        mut output,
    } = tokio::select! {
        biased;
        _ = session.revision.changed() => return Err(startup_failed()),
        result = tokio::time::timeout_at(operation_limit, installation.start()) => result.map_err(|_| startup_failed())??,
    };
    if session.revision.has_changed().unwrap_or(true) {
        return Err(startup_failed());
    }
    let now = tokio::time::Instant::now();
    let deadline = operation_limit.min(now + OPERATION_TIMEOUT);
    let timeout_ms = deadline.saturating_duration_since(now).as_millis();
    if timeout_ms == 0 {
        return Err(startup_failed());
    }
    let begin = tokio::time::timeout_at(
        deadline,
        bridge.call_in_session(
            session.id,
            "engine_begin",
            json!({"action":action,"params":params,"timeout_ms":timeout_ms}),
        ),
    )
    .await
    .map_err(|_| failed())??;
    let lease = begin
        .get("lease")
        .and_then(Value::as_str)
        .filter(|s| {
            (16..=128).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(failed)?
        .to_string();
    let mut cleanup = Lease {
        bridge: Arc::clone(bridge),
        session: session.id,
        lease: lease.clone(),
        finished: false,
    };
    let result = tokio::time::timeout_at(deadline, async {
        if begin.get("action") != Some(&json!(action)) || begin.get("params") != Some(&params)
            || !begin.get("tab").and_then(Value::as_u64).is_some_and(|tab| tab > 0)
            || params.get("tab").is_some_and(|tab| begin.get("tab") != Some(tab))
            || session.revision.has_changed().unwrap_or(true)
            || !begin.get("timeout_ms").and_then(Value::as_u64).is_some_and(|value| value > 0 && u128::from(value) <= timeout_ms)
        {
            return Err(failed());
        }
        let mut total = 0;
        let call = json!({"type":"call","operation":begin});
        charge_frame(&mut total, &call)?;
        write_frame(&mut input, &call).await?;
        let mut ids = BTreeSet::new();
        let mut replies: FuturesUnordered<futures_util::future::BoxFuture<'_, (u64, Result<Value, ToolError>)>> = FuturesUnordered::new();
        let mut frame = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = session.revision.changed() => {
                    tracing::debug!("browser engine operation revoked");
                    let _ = write_frame(&mut input, &json!({"type":"cancel"})).await;
                    return Err(failed());
                }
                event = session.events.recv() => {
                    let event = event.map_err(|_| failed())?;
                    if event.session == session.id && event.lease == lease {
                        let frame = json!({"type":"event","message":event.message});
                        charge_frame(&mut total, &frame)?;
                        write_frame(&mut input, &frame).await?;
                    }
                }
                Some((id, reply)) = replies.next(), if !replies.is_empty() => {
                    let mut response = json!({"type":"reply","id":id});
                    match reply {
                        Ok(envelope) => {
                            let object = envelope.as_object().ok_or_else(failed)?;
                            if object.len() != 1 || (!object.contains_key("result") && !object.contains_key("error")) { return Err(failed()); }
                            for (key,value) in object { response[key] = value.clone(); }
                        }
                        Err(error) => {
                            tracing::debug!(code = ?error.code, "browser engine transport refused a command");
                            response["error"] = json!({"code":"operation_refused","message":"Browser operation refused."});
                        }
                    }
                    charge_frame(&mut total, &response)?;
                    write_frame(&mut input, &response).await?;
                }
                line = read_frame(&mut output, &mut frame) => {
                    line?;
                    total = total.saturating_add(frame.len());
                    if total > MAX_TOTAL { return Err(failed()); }
                    let value: Value = serde_json::from_slice(&frame).map_err(|_| failed())?;
                    frame.clear();
                    match value.get("type").and_then(Value::as_str) {
                        Some("command") => {
                            let id = value.get("id").and_then(Value::as_u64).ok_or_else(failed)?;
                            if ids.len() >= 512 || replies.len() >= 128 || !ids.insert(id) { return Err(failed()); }
                            let command = value.get("command").filter(|v| v.is_object()).ok_or_else(failed)?;
                            if command.get("method").and_then(Value::as_str).is_none() { return Err(failed()); }
                            let params = json!({"lease":lease,"command":command});
                            let session_id = session.id;
                            replies.push(Box::pin(async move {
                                (id, bridge.call_in_session(session_id, "engine_command", params).await)
                            }));
                        }
                        Some("result") => {
                            if !replies.is_empty() {
                                tracing::debug!(pending = replies.len(), "browser engine completed with pending commands");
                                return Err(failed());
                            }
                            let surface = value.get("result").and_then(|v| v.get("surfaceBackendNodeId"));
                            if surface.is_some_and(|v| !v.is_u64() && !v.is_null()) { return Err(failed()); }
                            let mut finish = json!({"lease":lease});
                            if let Some(surface) = surface { finish["surfaceBackendNodeId"] = surface.clone(); }
                            // Finish is the extension's normal redacted result, never
                            // raw engine data. Dropping child ends only our own process.
                            return bridge.call_in_session(session.id,"engine_finish",finish).await;
                        }
                        Some("error") => {
                            tracing::debug!(commands = ids.len(), bytes = total, "browser engine reported an incomplete operation");
                            return Err(failed());
                        }
                        _ => return Err(failed()),
                    }
                }
            }
        }
    }).await.unwrap_or_else(|_| Err(failed()));
    if result.is_err() {
        let _ = tokio::time::timeout_at(
            request_deadline.min(tokio::time::Instant::now() + CLEANUP_TIMEOUT),
            bridge.call_in_session(session.id, "engine_cancel", json!({"lease":lease})),
        )
        .await;
    }
    cleanup.finished = true;
    result
}

fn charge_frame(total: &mut usize, value: &Value) -> Result<(), ToolError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| failed())?
        .len()
        .saturating_add(1);
    *total = total.saturating_add(bytes);
    if *total > MAX_TOTAL {
        return Err(failed());
    }
    Ok(())
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), ToolError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| failed())?;
    if bytes.len() > MAX_FRAME {
        return Err(failed());
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|_| failed())?;
    writer.flush().await.map_err(|_| failed())
}

async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    frame: &mut Vec<u8>,
) -> Result<(), ToolError> {
    loop {
        let bytes = reader.fill_buf().await.map_err(|_| failed())?;
        if bytes.is_empty() {
            return Err(failed());
        }
        let count = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len());
        if frame.len().saturating_add(count) > MAX_FRAME {
            return Err(failed());
        }
        let ended = bytes[count - 1] == b'\n';
        frame.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if ended {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
