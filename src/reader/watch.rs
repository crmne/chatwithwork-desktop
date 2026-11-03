//! Which directories the index watches, and how many OS watches that takes.
//!
//! The watcher only keeps the search index fresh, so it watches exactly the
//! directories the index walks: never what Git or `.ignore` ignores, hidden
//! folders (`.git` among them), dependencies and build output, linked
//! worktrees, anything the deny list covers, or a symlink (so never
//! anything outside the shared folder). See [`super::walk`].
//!
//! On Linux (and the BSDs) every watched directory costs a kernel watch
//! from a per-user pool that every program shares (inotify's
//! `fs.inotify.max_user_watches`). Running it dry breaks other programs,
//! systemd and the desktop session among them. So each directory is
//! watched on its own, non-recursively, the shallowest first, and the
//! daemon never holds more than [`default_limit`] of them. Folders past
//! the limit are kept fresh by rescans instead: when a search asks, and on
//! the periodic rescan.
//!
//! macOS (FSEvents) and Windows (`ReadDirectoryChangesW`) watch a whole
//! tree with one handle, so each shared folder is watched recursively there
//! and has no limit; events from directories the index skips are dropped
//! before they cost a rescan.

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tokio_util::sync::CancellationToken;

use super::safe_fs::{RelPath, RootHandle};
use super::walk::{ALWAYS_SKIPPED, Scope, Target, walk_dirs_until};
use crate::policy::DenyList;

/// The most OS watches the daemon holds, whatever the system allows.
pub const MAX_WATCHES: usize = 8192;

/// The longest the warning about the limit stays quiet once given.
const WARN_EVERY: Duration = Duration::from_secs(60 * 60);

/// Whether each directory takes a watch of its own (inotify, kqueue), or one
/// recursive watch covers a whole tree (FSEvents, `ReadDirectoryChangesW`).
pub const PER_DIRECTORY: bool = !cfg!(any(target_os = "macos", target_os = "windows"));

/// The most watches to hold: the lesser of [`MAX_WATCHES`] and a tenth of
/// the user's inotify limit, so other programs keep the rest.
pub fn default_limit() -> usize {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let system = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok());
        limit_for(system)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        // kqueue holds a file descriptor per watch.
        1024
    }
}

/// The limit for a system-wide inotify limit, when it could be read.
pub fn limit_for(max_user_watches: Option<usize>) -> usize {
    match max_user_watches {
        Some(system) => (system / 10).clamp(1, MAX_WATCHES),
        None => MAX_WATCHES,
    }
}

/// Logs at most one warning per [`WARN_EVERY`] for each kind of trouble.
fn warn_rarely(slot: &'static Mutex<Option<Instant>>, message: impl FnOnce() -> String) {
    let mut last = slot.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|at| at.elapsed() < WARN_EVERY) {
        tracing::debug!("{}", message());
        return;
    }
    *last = Some(Instant::now());
    tracing::warn!("{}", message());
}

static LIMIT_WARNED: Mutex<Option<Instant>> = Mutex::new(None);
static ERROR_WARNED: Mutex<Option<Instant>> = Mutex::new(None);

/// A watcher error: warned about at most once an hour, then rescanned.
pub fn note_error(error: &notify::Error) {
    warn_rarely(&ERROR_WARNED, || {
        format!("file watcher error, rescanning: {error}")
    });
}

pub struct Watches {
    watcher: notify::RecommendedWatcher,
    per_directory: bool,
    limit: usize,
    /// The directories whose events count: on Linux, each one watched.
    dirs: HashSet<PathBuf>,
    /// OS watches held. The daemon has one set at a time (the old one goes
    /// before a new one is made), so this is the daemon's count.
    held: usize,
    /// Shared folders with directories left unwatched, by ID.
    incomplete: HashSet<String>,
    /// Directories left unwatched for the limit.
    skipped: usize,
}

impl Watches {
    pub fn new(limit: usize, handler: impl notify::EventHandler) -> notify::Result<Self> {
        Self::with_mode(limit, PER_DIRECTORY, handler)
    }

    pub fn with_mode(
        limit: usize,
        per_directory: bool,
        handler: impl notify::EventHandler,
    ) -> notify::Result<Self> {
        Ok(Self {
            watcher: notify::recommended_watcher(handler)?,
            per_directory,
            limit,
            dirs: HashSet::new(),
            held: 0,
            incomplete: HashSet::new(),
            skipped: 0,
        })
    }

    /// Whether every directory the index walks in this folder is watched.
    pub fn covers(&self, root_id: &str) -> bool {
        !self.incomplete.contains(root_id)
    }

    /// The directories watched (or, on macOS and Windows, followed).
    pub fn dirs(&self) -> &HashSet<PathBuf> {
        &self.dirs
    }

    /// OS watches held: one per directory on Linux, one per folder elsewhere.
    pub fn held(&self) -> usize {
        self.held
    }

    /// Watch a shared folder: every directory the index walks, the shallowest
    /// first, up to the limit.
    pub fn add_root(
        &mut self,
        root: &RootHandle,
        deny: &Arc<DenyList>,
        cancelled: &CancellationToken,
    ) {
        if !self.per_directory {
            match self.watcher.watch(&root.path, RecursiveMode::Recursive) {
                Ok(()) => self.held += 1,
                Err(e) => {
                    tracing::warn!(root = %root.id, "can't watch folder, rescanning periodically: {e}");
                    self.incomplete.insert(root.id.clone());
                }
            }
        }
        let dirs = collect_dirs(root, deny, None, cancelled);
        if cancelled.is_cancelled() {
            self.incomplete.insert(root.id.clone());
            return;
        }
        self.add_dirs(root, dirs);
    }

    /// Events for paths the index skips are dropped; new directories are
    /// watched, removed ones let go. Returns the paths worth a rescan.
    pub fn filter(
        &mut self,
        paths: Vec<PathBuf>,
        roots: &[Arc<RootHandle>],
        deny: &Arc<DenyList>,
        cancelled: &CancellationToken,
    ) -> Vec<PathBuf> {
        let mut kept = Vec::new();
        for path in paths {
            let Some(root) = roots.iter().find(|r| path.starts_with(&r.path)) else {
                continue;
            };
            if never_indexed(&root.path, &path) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            if self.dirs.contains(&path) {
                if !is_dir {
                    self.forget(&path);
                }
                kept.push(path);
                continue;
            }
            let parent_known = path.parent().is_some_and(|p| self.dirs.contains(p));
            if !parent_known {
                // From inside a directory the index skips (only recursive
                // watches report those), or left over from one let go.
                continue;
            }
            if is_dir && self.per_directory && self.held >= self.limit {
                // No watch left for it: the rescan it sets off, and later ones,
                // keep it up to date.
                self.skipped += 1;
                self.incomplete.insert(root.id.clone());
            } else if is_dir {
                let Some(rel) = path
                    .strip_prefix(&root.path)
                    .ok()
                    .and_then(RelPath::from_relative_path)
                else {
                    continue;
                };
                let scope = Arc::new(Scope(vec![Target::Recursive(rel)]));
                let found = collect_dirs(root, deny, Some(scope), cancelled);
                if !found.iter().any(|d| d == &path) {
                    // Ignored, denied or skipped.
                    continue;
                }
                self.add_dirs(root, found);
            }
            kept.push(path);
        }
        kept
    }

    fn add_dirs(&mut self, root: &RootHandle, mut dirs: Vec<PathBuf>) {
        dirs.retain(|d| !self.dirs.contains(d));
        if !self.per_directory {
            self.dirs.extend(dirs);
            return;
        }
        // The shallowest first: past the limit, the deep corners of a tree go
        // unwatched rather than whole top-level folders.
        dirs.sort_by_key(|d| d.components().count());
        let mut left = 0usize;
        for dir in dirs {
            if self.held >= self.limit {
                left += 1;
                continue;
            }
            match self.watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.held += 1;
                    self.dirs.insert(dir);
                }
                // Gone since the walk.
                Err(e) if matches!(e.kind, notify::ErrorKind::PathNotFound) => {}
                // Unreadable, say: rescans cover it.
                Err(e) => {
                    left += 1;
                    tracing::debug!(root = %root.id, "can't watch a folder: {e}");
                    if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                        // The system ran out before our limit: other programs
                        // hold the rest. Stop asking for more.
                        self.limit = self.held;
                    }
                }
            }
        }
        if left > 0 {
            self.skipped += left;
            self.incomplete.insert(root.id.clone());
            let (held, limit, skipped) = (self.held, self.limit, self.skipped);
            warn_rarely(&LIMIT_WARNED, || {
                format!(
                    "watching {held} folders (at most {limit}, to leave the system's \
                     file watches to other programs); {skipped} more are kept up to date \
                     by rescans instead"
                )
            });
        }
    }

    /// A directory went away (or became a file or a link): let go of it and
    /// everything watched below it.
    fn forget(&mut self, gone: &Path) {
        let below: Vec<PathBuf> = self
            .dirs
            .iter()
            .filter(|d| d.starts_with(gone))
            .cloned()
            .collect();
        for dir in below {
            self.dirs.remove(&dir);
            if self.per_directory {
                // The kernel drops a deleted directory's watch by itself.
                let _ = self.watcher.unwatch(&dir);
                self.held -= 1;
            }
        }
    }
}

fn collect_dirs(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    scope: Option<Arc<Scope>>,
    cancelled: &CancellationToken,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    walk_dirs_until(root, deny, scope, cancelled, |dir| {
        dirs.push(dir.to_path_buf());
        ControlFlow::Continue(())
    });
    dirs
}

/// A cheap first look at an event's path, by name only: a hidden file, or
/// anything inside a hidden folder or one the index always skips, can't
/// change the index. (The deny list is checked by the caller, and a new
/// directory's own rules when it is walked.)
fn never_indexed(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    let parts: Vec<Component> = rel.components().collect();
    for (i, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return true;
        };
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') || (i + 1 < parts.len() && ALWAYS_SKIPPED.contains(&name)) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &Path) -> RootHandle {
        RootHandle::open(&crate::config::Root {
            id: "code".into(),
            label: "Code".into(),
            path: path.to_path_buf(),
            bots: Vec::new(),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        })
        .unwrap()
    }

    fn no_deny() -> Arc<DenyList> {
        Arc::new(DenyList::new(Vec::<String>::new()).unwrap())
    }

    fn write(base: &Path, rel: &str, text: &str) {
        let path = base.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A shared folder of code, with everything that shouldn't be watched.
    fn project_tree(base: &Path) {
        write(base, "app/Cargo.toml", "[package]\n");
        write(base, "app/.gitignore", "/generated/\n");
        write(base, "app/src/main.rs", "fn main() {}\n");
        write(base, "app/src/deep/mod.rs", "\n");
        write(base, "app/target/debug/deps/x.rlib", "x");
        write(
            base,
            "app/target/CACHEDIR.TAG",
            "Signature: 8a477f597d28d172789f06886806bc55",
        );
        write(base, "app/build/out.o", "x");
        write(base, "app/generated/schema.rs", "x");
        write(base, "app/node_modules/left-pad/index.js", "x");
        write(base, "app/node_modules/left-pad/lib/a.js", "x");
        write(base, "app/.git/objects/aa/bb", "x");
        write(base, "app/.git/HEAD", "ref: refs/heads/main\n");
        write(base, "app/tmp/cache/x", "x");
        write(base, "app/vendor/bundle/gem.rb", "x");
        write(base, "app/__pycache__/m.pyc", "x");
        write(base, "app/env/pyvenv.cfg", "home = /usr\n");
        write(base, "app/env/lib/site.py", "x");
        write(
            base,
            "app/cache/CACHEDIR.TAG",
            "Signature: 8a477f597d28d172789f06886806bc55",
        );
        write(base, "app/cache/blob", "x");
        // A linked worktree repeats a checkout; a submodule is its own code.
        write(
            base,
            "app-feature/.git",
            "gitdir: /src/app/.git/worktrees/app-feature\n",
        );
        write(base, "app-feature/src/main.rs", "x");
        write(base, "app/lib/sub/.git", "gitdir: ../../.git/modules/sub\n");
        write(base, "app/lib/sub/sub.rs", "x");
        // Outside a project, `build` and `vendor` are somebody's documents.
        write(base, "notes/build/plan.md", "x");
        write(base, "notes/vendor/invoice.md", "x");
        write(base, "notes/.obsidian/workspace.json", "x");
        write(base, "denied/secret.txt", "x");
    }

    fn rels(base: &Path, dirs: &HashSet<PathBuf>) -> Vec<String> {
        let mut out: Vec<String> = dirs
            .iter()
            .map(|d| {
                d.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        out.sort();
        out
    }

    fn watches(limit: usize, per_directory: bool) -> Watches {
        Watches::with_mode(limit, per_directory, |_: notify::Result<notify::Event>| {}).unwrap()
    }

    #[test]
    fn dependencies_build_output_git_and_ignored_folders_get_no_watches() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        project_tree(&base);
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "big/a/b.txt", "x");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), base.join("app/linked")).unwrap();
        let root = root(&base);
        let deny = Arc::new(DenyList::new(["denied".to_string()]).unwrap());
        for per_directory in [true, false] {
            let mut w = watches(1000, per_directory);
            w.add_root(&root, &deny, &CancellationToken::new());
            assert_eq!(
                rels(&base, w.dirs()),
                [
                    "",
                    "app",
                    "app/lib",
                    "app/lib/sub",
                    "app/src",
                    "app/src/deep",
                    "notes",
                    "notes/build",
                    "notes/vendor",
                ],
                "per directory: {per_directory}"
            );
            assert!(w.covers("code"));
            assert_eq!(w.held(), if per_directory { 9 } else { 1 });
        }
    }

    #[test]
    fn the_limit_holds_and_the_shallowest_folders_are_watched_first() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        for i in 0..20 {
            write(&base, &format!("top-{i}/a/b/c/file.txt"), "x");
        }
        let root = root(&base);
        let mut w = watches(25, true);
        w.add_root(&root, &no_deny(), &CancellationToken::new());
        assert_eq!(w.held(), 25);
        assert_eq!(w.dirs().len(), 25);
        assert!(!w.covers("code"), "the rest is rescanned instead");
        let depth = |d: &PathBuf| d.strip_prefix(&base).unwrap().components().count();
        assert_eq!(w.dirs().iter().filter(|d| depth(d) == 1).count(), 20);
        assert!(w.dirs().iter().all(|d| depth(d) <= 2));

        // New folders past the limit are rescanned, not watched.
        std::fs::create_dir(base.join("top-0/new")).unwrap();
        let kept = w.filter(
            vec![base.join("top-0/new")],
            &[Arc::new(root)],
            &no_deny(),
            &CancellationToken::new(),
        );
        assert_eq!(kept, [base.join("top-0/new")]);
        assert_eq!(w.held(), 25);
    }

    #[test]
    fn new_folders_are_watched_by_the_same_rules_and_removed_ones_let_go() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        project_tree(&base);
        let root = Arc::new(root(&base));
        let roots = [Arc::clone(&root)];
        let mut w = watches(1000, true);
        let (deny, never) = (no_deny(), CancellationToken::new());
        w.add_root(&root, &deny, &never);
        let before = w.held();

        write(&base, "app/src/new/deeper/x.rs", "x");
        write(&base, "app/node_modules/more/index.js", "x");
        write(&base, "app/target/release/y", "x");
        write(&base, "app/generated/again/z.rs", "x");
        let kept = w.filter(
            vec![
                base.join("app/src/new"),
                base.join("app/node_modules/more"),
                base.join("app/node_modules/more/index.js"),
                base.join("app/target/release"),
                base.join("app/generated/again"),
                base.join("app/.git/index"),
                base.join("app/src/.main.rs.swp"),
                base.join("app/src/main.rs"),
            ],
            &roots,
            &deny,
            &never,
        );
        assert_eq!(
            kept,
            [base.join("app/src/new"), base.join("app/src/main.rs")]
        );
        assert_eq!(w.held(), before + 2, "app/src/new and app/src/new/deeper");

        std::fs::remove_dir_all(base.join("app/src")).unwrap();
        let kept = w.filter(vec![base.join("app/src")], &roots, &deny, &never);
        assert_eq!(kept, [base.join("app/src")]);
        assert_eq!(w.held(), before - 2, "app/src and app/src/deep");
        assert!(
            w.dirs()
                .iter()
                .all(|d| !d.starts_with(base.join("app/src")))
        );
    }

    #[test]
    fn the_limit_is_a_tenth_of_the_system_s_at_most_8192() {
        assert_eq!(limit_for(Some(524_288)), 8192);
        assert_eq!(limit_for(Some(65_536)), 6553);
        assert_eq!(limit_for(Some(8192)), 819);
        assert_eq!(limit_for(Some(0)), 1);
        assert_eq!(limit_for(None), 8192);
        assert!(default_limit() <= MAX_WATCHES);
    }
}
