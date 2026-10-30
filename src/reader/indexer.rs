//! Background indexing: an initial scan per root, then file watching with
//! `notify` and debounced, incremental updates.
//!
//! A scan compares each file's mtime and size with the index and only
//! extracts files that changed, so rescans are cheap. While the watcher
//! works, the indexer sleeps until an event arrives: there is no timer and
//! no polling. Events are collected until the folder has been quiet for a
//! moment (or a burst has gone on for a while), then only the directories
//! they touched are rescanned. A full rescan of a root happens at start,
//! after a config change, and when the OS says events were lost. Only when
//! watching is off or unavailable does a periodic rescan take over.

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use serde::Serialize;

use super::extract::{self, format_for, read_capped};
use super::index::{MAX_BODY_CHARS, SearchIndex};
use super::safe_fs::{OpenPolicy, RelPath, RootHandle};
use super::walk::{Scope, Target, walk_scoped_until};
use crate::policy::DenyList;
use crate::status::Changes;

/// Index once the folder has been quiet this long...
const QUIET: Duration = Duration::from_millis(750);
/// ...or once a burst of changes has lasted this long.
const MAX_WAIT: Duration = Duration::from_secs(10);
/// More changed paths than this in one burst rescan the whole root.
const MAX_PENDING_PATHS: usize = 4096;
/// Report progress every so many files during a first pass...
const PROGRESS_EVERY: u64 = 500;
/// ...but commit at most this often. Each commit syncs new segment files,
/// which is slow on Windows, where Defender scans every one of them.
const COMMIT_INTERVAL: Duration = Duration::from_secs(15);
/// A parser may still be completing one bounded file. Do not hold a sandbox
/// restart hostage to that synchronous third-party call.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    Pending,
    Indexing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootIndexStatus {
    pub state: IndexState,
    pub files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub type StatusMap = Arc<Mutex<HashMap<String, RootIndexStatus>>>;

pub struct IndexerSettings {
    pub roots: Vec<Arc<RootHandle>>,
    pub deny: Arc<DenyList>,
    pub allow_hardlinks: bool,
    pub max_file_bytes: u64,
    pub watch: bool,
    /// Full rescan interval, used only when watching is off or failed.
    pub rescan: Duration,
}

enum Command {
    Configure(IndexerSettings, CancellationToken),
    Changed(Vec<PathBuf>),
    /// The watcher lost events; rescan everything.
    Rescan,
    Shutdown,
}

pub struct Indexer {
    tx: mpsc::Sender<Command>,
    status: StatusMap,
    thread: Option<JoinHandle<()>>,
    stopped: CancellationToken,
    scan: Mutex<CancellationToken>,
    finished: Mutex<mpsc::Receiver<()>>,
    index: Arc<OnceLock<Arc<SearchIndex>>>,
}

impl Indexer {
    pub fn start(path: PathBuf, settings: IndexerSettings, changes: Changes) -> Self {
        Self::start_with(settings, changes, move || SearchIndex::open(&path))
    }

    fn start_with(
        settings: IndexerSettings,
        changes: Changes,
        open: impl FnOnce() -> anyhow::Result<SearchIndex> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let status: StatusMap = Arc::new(Mutex::new(
            settings
                .roots
                .iter()
                .map(|root| {
                    (
                        root.id.clone(),
                        RootIndexStatus {
                            state: IndexState::Pending,
                            files: 0,
                            last_error: None,
                        },
                    )
                })
                .collect(),
        ));
        let stopped = CancellationToken::new();
        let scan = stopped.child_token();
        let index = Arc::new(OnceLock::new());
        let (worker_status, worker_stop, worker_scan, worker_index, worker_tx) = (
            Arc::clone(&status),
            stopped.clone(),
            scan.clone(),
            Arc::clone(&index),
            tx.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("cww-indexer".into())
            .spawn(move || {
                if worker_stop.is_cancelled() {
                    let _ = done.send(());
                    return;
                }
                // Opening a large persisted index must not delay control/status.
                match open() {
                    Ok(index) if !worker_stop.is_cancelled() => {
                        let index = Arc::new(index);
                        let _ = worker_index.set(Arc::clone(&index));
                        Worker {
                            index,
                            status: worker_status,
                            changes,
                            tx: worker_tx,
                            settings,
                            watcher: None,
                            watching: false,
                            pending: HashMap::new(),
                            burst: None,
                            last_full: Instant::now(),
                            stopped: worker_stop,
                            scan: worker_scan,
                        }
                        .run(rx);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        for value in worker_status.lock().expect("status lock").values_mut() {
                            value.state = IndexState::Error;
                            value.last_error = Some(error.to_string());
                        }
                        changes.bump();
                        tracing::warn!("local search index unavailable: {error}");
                    }
                }
                let _ = done.send(());
            })
            .expect("spawning indexer thread");
        Self {
            tx,
            status,
            thread: Some(thread),
            stopped,
            scan: Mutex::new(scan),
            finished: Mutex::new(finished),
            index,
        }
    }

    pub fn index(&self) -> Option<Arc<SearchIndex>> {
        self.index.get().cloned()
    }

    pub fn configure(&self, settings: IndexerSettings) {
        let mut scan = self.scan.lock().expect("scan cancellation");
        scan.cancel();
        *scan = self.stopped.child_token();
        let _ = self.tx.send(Command::Configure(settings, scan.clone()));
    }

    pub fn stop(&self) {
        self.stopped.cancel();
        let _ = self.tx.send(Command::Shutdown);
    }

    pub fn status(&self) -> HashMap<String, RootIndexStatus> {
        self.status.lock().expect("status lock").clone()
    }

    /// True once every configured root has been scanned at least once.
    pub fn is_ready(&self, root_id: &str) -> bool {
        self.status
            .lock()
            .expect("status lock")
            .get(root_id)
            .is_some_and(|s| s.state == IndexState::Ready)
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            if self
                .finished
                .lock()
                .expect("index completion")
                .recv_timeout(SHUTDOWN_WAIT)
                .is_ok()
                || thread.is_finished()
            {
                let _ = thread.join();
            } else {
                // Only the already-running parser/open can remain. Cooperative
                // checks prevent another read, prune or commit afterward; a
                // confined worker process restart ends this thread as well.
                tracing::debug!("indexer is finishing its cancelled operation");
            }
        }
    }
}

/// Changes waiting for the debounce to end, per root.
#[derive(Default)]
struct Pending {
    full: bool,
    paths: HashSet<PathBuf>,
}

/// When the current burst of changes started and last grew.
#[derive(Clone, Copy)]
struct Burst {
    first: Instant,
    last: Instant,
}

struct Worker {
    index: Arc<SearchIndex>,
    status: StatusMap,
    changes: Changes,
    tx: mpsc::Sender<Command>,
    settings: IndexerSettings,
    watcher: Option<notify::RecommendedWatcher>,
    /// Every root is watched, so no periodic rescan is needed.
    watching: bool,
    pending: HashMap<String, Pending>,
    burst: Option<Burst>,
    last_full: Instant,
    stopped: CancellationToken,
    scan: CancellationToken,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Command>) {
        self.apply_settings();
        while !self.stopped.is_cancelled() {
            let command = match self.timeout() {
                // Nothing pending and the watcher covers everything: sleep
                // until something happens.
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(timeout) => rx.recv_timeout(timeout),
            };
            match command {
                Ok(Command::Configure(settings, scan)) => {
                    self.settings = settings;
                    self.scan = scan;
                    self.apply_settings();
                }
                Ok(Command::Changed(paths)) => self.note_changes(paths),
                Ok(Command::Rescan) => {
                    for root in &self.settings.roots {
                        self.pending.entry(root.id.clone()).or_default().full = true;
                    }
                    self.note_burst();
                }
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if self.burst.take().is_some() {
                        self.flush_pending();
                    } else {
                        self.scan_all();
                    }
                }
            }
        }
        // Cancelled scans are replayed at next startup; do not start another
        // expensive fsync/merge during shutdown.
    }

    /// How long to wait for the next command, or `None` to wait forever.
    fn timeout(&self) -> Option<Duration> {
        if let Some(burst) = self.burst {
            let quiet = QUIET.saturating_sub(burst.last.elapsed());
            let cap = MAX_WAIT.saturating_sub(burst.first.elapsed());
            return Some(quiet.min(cap));
        }
        if self.watching || self.settings.roots.is_empty() {
            return None;
        }
        Some(
            self.settings
                .rescan
                .saturating_sub(self.last_full.elapsed()),
        )
    }

    fn note_burst(&mut self) {
        let now = Instant::now();
        self.burst = Some(match self.burst {
            Some(b) => Burst { last: now, ..b },
            None => Burst {
                first: now,
                last: now,
            },
        });
    }

    fn note_changes(&mut self, paths: Vec<PathBuf>) {
        let mut any = false;
        for path in paths {
            // Our own index and logs are denied; ignoring them also stops
            // commits from retriggering scans.
            if self.settings.deny.is_denied(&path) {
                continue;
            }
            let Some(root) = self
                .settings
                .roots
                .iter()
                .find(|r| path.starts_with(&r.path))
            else {
                continue;
            };
            let pending = self.pending.entry(root.id.clone()).or_default();
            if !pending.full {
                pending.paths.insert(path);
                if pending.paths.len() > MAX_PENDING_PATHS {
                    pending.full = true;
                    pending.paths.clear();
                }
            }
            any = true;
        }
        if any {
            self.note_burst();
        }
    }

    fn flush_pending(&mut self) {
        for (id, pending) in std::mem::take(&mut self.pending) {
            if self.scan.is_cancelled() {
                break;
            }
            let Some(root) = self.root(&id) else {
                continue;
            };
            if pending.full {
                self.scan(&root, None);
            } else if let Some(scope) = scope_for(&root, pending.paths) {
                self.scan(&root, Some(scope));
            } else {
                self.scan(&root, None);
            }
        }
    }

    fn root(&self, id: &str) -> Option<Arc<RootHandle>> {
        self.settings.roots.iter().find(|r| r.id == id).cloned()
    }

    fn apply_settings(&mut self) {
        if self.scan.is_cancelled() {
            return;
        }
        {
            let mut status = self.status.lock().expect("status lock");
            status.retain(|id, _| self.settings.roots.iter().any(|r| &r.id == id));
            for root in &self.settings.roots {
                status.entry(root.id.clone()).or_insert(RootIndexStatus {
                    state: IndexState::Pending,
                    files: 0,
                    last_error: None,
                });
            }
        }
        self.changes.bump();
        // Drop documents of roots that are no longer shared.
        if let Ok(indexed) = self.index.indexed_roots() {
            for id in indexed {
                if self.scan.is_cancelled() {
                    return;
                }
                if !self.settings.roots.iter().any(|r| r.id == id) {
                    self.index.delete_root(&id);
                }
            }
            if self.scan.is_cancelled() {
                return;
            }
            let _ = self.index.commit();
        }
        if self.scan.is_cancelled() {
            return;
        }
        self.watcher = None;
        self.watching = false;
        if self.settings.watch {
            self.start_watcher();
        }
        self.scan_all();
    }

    fn start_watcher(&mut self) {
        use notify::{RecursiveMode, Watcher};

        let tx = self.tx.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let command = match event {
                Ok(event) if event.need_rescan() => Command::Rescan,
                Ok(event) if event.kind.is_access() => return,
                Ok(event) => Command::Changed(event.paths),
                Err(e) => {
                    tracing::warn!("file watcher error, rescanning: {e}");
                    Command::Rescan
                }
            };
            let _ = tx.send(command);
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("file watching unavailable, rescanning periodically: {e}");
                return;
            }
        };
        let mut all = true;
        for root in &self.settings.roots {
            if self.scan.is_cancelled() {
                return;
            }
            if let Err(e) = watcher.watch(&root.path, RecursiveMode::Recursive) {
                tracing::warn!(root = %root.id, "can't watch root, rescanning periodically: {e}");
                all = false;
            }
        }
        self.watching = all;
        self.watcher = Some(watcher);
    }

    fn scan_all(&mut self) {
        self.last_full = Instant::now();
        self.pending.clear();
        self.burst = None;
        for root in self.settings.roots.clone() {
            if self.scan.is_cancelled() {
                break;
            }
            self.scan(&root, None);
        }
    }

    fn set_status(&self, id: &str, state: IndexState, files: Option<u64>, error: Option<String>) {
        let mut status = self.status.lock().expect("status lock");
        if let Some(s) = status.get_mut(id) {
            let changed = s.state != state || files.is_some_and(|f| f != s.files);
            s.state = state;
            if let Some(files) = files {
                s.files = files;
            }
            s.last_error = error;
            if changed {
                self.changes.bump();
            }
        }
    }

    /// Bring the index for one root (or part of it) in line with the disk.
    fn scan(&self, root: &RootHandle, scope: Option<Scope>) {
        if self.scan.is_cancelled() {
            return;
        }
        let first = !matches!(
            self.status
                .lock()
                .expect("status lock")
                .get(&root.id)
                .map(|s| s.state),
            Some(IndexState::Ready)
        );
        if first {
            self.set_status(&root.id, IndexState::Indexing, None, None);
        }
        let mut known = match self
            .index
            .indexed_files_until(&root.id, || self.scan.is_cancelled())
        {
            Ok(Some(known)) => known,
            Ok(None) => return,
            Err(e) => {
                self.set_status(&root.id, IndexState::Error, None, Some(e.to_string()));
                return;
            }
        };
        if let Some(scope) = &scope {
            known.retain(|rel, _| scope.covers_file(rel));
        }
        let policy = OpenPolicy {
            deny: &self.settings.deny,
            allow_hardlinks: self.settings.allow_hardlinks,
        };
        let mut seen = 0u64;
        let mut last_commit = Instant::now();
        walk_scoped_until(
            root,
            &self.settings.deny,
            scope.map(Arc::new),
            &self.scan,
            |file| {
                if self.scan.is_cancelled() {
                    return ControlFlow::Break(());
                }
                let rel = file.rel.as_string();
                seen += 1;
                if first && seen.is_multiple_of(PROGRESS_EVERY) {
                    // Progress for the status line during the first pass.
                    self.set_status(&root.id, IndexState::Indexing, Some(seen), None);
                }
                let unchanged = known
                    .remove(&rel)
                    .is_some_and(|(mtime, size)| mtime == file.modified && size == file.size);
                if unchanged {
                    return ControlFlow::Continue(());
                }
                // Files the policy refuses to open (hard links, special files)
                // are left out entirely; files that can't be turned into text
                // are still findable by name.
                let body = if file.size <= self.settings.max_file_bytes
                    && format_for(&rel).is_some()
                {
                    match root.open_file(&file.rel, policy) {
                        Err(e) if e.code.is_denial() => {
                            self.index.delete(&root.id, &rel);
                            return ControlFlow::Continue(());
                        }
                        Err(_) => String::new(),
                        Ok(mut opened) => {
                            read_capped(&mut opened.file, self.settings.max_file_bytes)
                                .and_then(|bytes| extract::extract(&rel, &bytes, MAX_BODY_CHARS))
                                .unwrap_or_default()
                        }
                    }
                } else {
                    String::new()
                };
                if self.scan.is_cancelled() {
                    return ControlFlow::Break(());
                }
                if let Err(e) = self
                    .index
                    .upsert(&root.id, &rel, &body, file.modified, file.size)
                {
                    tracing::warn!(root = %root.id, "indexing failed: {e}");
                }
                // Commit now and then, so searches see a long first pass as it
                // goes.
                if last_commit.elapsed() >= COMMIT_INTERVAL {
                    let _ = self.index.commit();
                    last_commit = Instant::now();
                }
                ControlFlow::Continue(())
            },
        );
        // Unseen files are not deleted by an interrupted pass.
        if self.scan.is_cancelled() {
            return;
        }
        for rel in known.keys() {
            if self.scan.is_cancelled() {
                return;
            }
            self.index.delete(&root.id, rel);
        }
        if self.scan.is_cancelled() {
            return;
        }
        match self.index.commit() {
            Ok(()) => {
                let files = self.index.root_count(&root.id);
                self.set_status(&root.id, IndexState::Ready, Some(files), None);
            }
            Err(e) => self.set_status(&root.id, IndexState::Error, None, Some(e.to_string())),
        }
        release_memory();
    }
}

/// The directories to rescan for a set of changed paths: a changed file
/// rescans its directory (non-recursively), a changed or vanished directory
/// is rescanned in full. `None` means the root itself changed.
fn scope_for(root: &RootHandle, paths: HashSet<PathBuf>) -> Option<Scope> {
    let mut targets: HashSet<Target> = HashSet::new();
    for path in paths {
        let Ok(rel) = path.strip_prefix(&root.path) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            return None;
        }
        let Some(rel_path) = RelPath::from_relative_path(rel) else {
            continue;
        };
        let target = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => Target::Recursive(rel_path),
            Ok(_) => Target::Shallow(parent(&rel_path)),
            // Gone: whatever was indexed at or below it goes too.
            Err(_) => Target::Recursive(rel_path),
        };
        targets.insert(target);
    }
    Some(Scope(targets.into_iter().collect()))
}

fn parent(rel: &RelPath) -> RelPath {
    let parts = rel.components();
    RelPath::parse(&parts[..parts.len().saturating_sub(1)].join("/")).unwrap_or_default()
}

/// Extracting large documents peaks at a few hundred megabytes. Once a pass
/// is done, have mimalloc (the binaries' allocator) return what it freed, so
/// an idle daemon doesn't sit on it.
fn release_memory() {
    // SAFETY: mi_collect only returns free memory to the system.
    unsafe {
        libmimalloc_sys::mi_collect(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(path: &std::path::Path) -> IndexerSettings {
        let root = RootHandle::open(&crate::config::Root {
            id: "docs".into(),
            label: "Docs".into(),
            path: path.to_path_buf(),
            bots: Vec::new(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        })
        .unwrap();
        IndexerSettings {
            roots: vec![Arc::new(root)],
            deny: Arc::new(DenyList::new(Vec::<String>::new()).unwrap()),
            allow_hardlinks: false,
            max_file_bytes: 1024,
            watch: false,
            rescan: Duration::from_secs(3600),
        }
    }

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "index worker did not reach the expected state"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn slow_open_leaves_status_available_and_uses_the_latest_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("docs");
        std::fs::create_dir(&root).unwrap();
        let path = tmp.path().join("index");
        let (entered, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let indexer = Indexer::start_with(settings(&root), Changes::default(), move || {
            entered.send(()).unwrap();
            gate.recv().unwrap();
            SearchIndex::open(&path)
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(indexer.status()["docs"].state, IndexState::Pending);
        assert!(
            indexer.index().is_none(),
            "status does not wait for Tantivy"
        );
        let mut new = settings(&root);
        new.roots.clear();
        indexer.configure(new);
        release.send(()).unwrap();
        until(|| indexer.index().is_some() && indexer.status().is_empty());
        assert_eq!(indexer.index().unwrap().doc_count(), 0);
    }

    #[test]
    fn shutdown_does_not_wait_indefinitely_for_a_synchronous_open() {
        let tmp = tempfile::tempdir().unwrap();
        let (entered, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (ended, end) = mpsc::channel();
        let indexer = Indexer::start_with(settings(tmp.path()), Changes::default(), move || {
            entered.send(()).unwrap();
            gate.recv().unwrap();
            ended.send(()).unwrap();
            anyhow::bail!("synthetic unavailable index")
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        drop(indexer);
        let elapsed = start.elapsed();
        release.send(()).unwrap();
        end.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            elapsed < Duration::from_secs(2),
            "shutdown took {elapsed:?}"
        );
    }

    #[test]
    fn cancellation_of_a_large_scan_does_not_prune_unseen_indexed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("docs");
        std::fs::create_dir(&root).unwrap();
        for i in 0..5000 {
            std::fs::write(root.join(format!("file-{i}.bin")), "x").unwrap();
        }
        let index = SearchIndex::open(&tmp.path().join("index")).unwrap();
        for i in 0..20000 {
            index
                .upsert("docs", &format!("retained-{i}.bin"), "", 1, 1)
                .unwrap();
        }
        index.commit().unwrap();
        let indexer = Indexer::start_with(settings(&root), Changes::default(), move || Ok(index));
        until(|| {
            indexer
                .status()
                .get("docs")
                .is_some_and(|s| s.state == IndexState::Indexing && s.files >= PROGRESS_EVERY)
        });
        let index = indexer.index().unwrap();
        let start = Instant::now();
        indexer.stop();
        drop(indexer);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            index.doc_count(),
            20000,
            "an interrupted traversal must not delete entries it has not visited"
        );
    }
}
