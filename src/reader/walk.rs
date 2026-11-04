//! Walk a root with ripgrep's `ignore` crate.
//!
//! The walk never follows symlinks, respects `.gitignore` and `.ignore`,
//! skips hidden files, and prunes denied directories so their contents are
//! never visited. Files are yielded by relative path; callers open them
//! through [`RootHandle`](super::safe_fs::RootHandle), never by the walked
//! path, so a swap between walk and open can't escape the root.
//!
//! It also skips what is never worth indexing or watching, Git-ignored or
//! not: dependency and build output (see [`skipped_dir`]) and linked Git
//! worktrees, which repeat a checkout kept elsewhere.
//!
//! A walk can be limited to a [`Scope`]: a few directories that changed.
//! It still starts at the root and only descends along the way to them, so
//! ignore files in the directories above apply exactly as in a full walk.

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

use ignore::WalkBuilder;
use tokio_util::sync::CancellationToken;

use super::safe_fs::{RelPath, RootHandle};
use crate::policy::DenyList;

pub struct WalkedFile {
    pub rel: RelPath,
    pub size: u64,
    pub modified: i64,
}

/// Part of a root to walk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// Everything below this directory (or this file).
    Recursive(RelPath),
    /// Only the files directly inside this directory.
    Shallow(RelPath),
}

impl Target {
    /// Whether an entry at `rel` is visited: files inside the target, and
    /// the directories on the way there.
    fn admits(&self, rel: &[String], is_dir: bool) -> bool {
        let (target, recursive) = match self {
            Self::Recursive(t) => (t.components(), true),
            Self::Shallow(t) => (t.components(), false),
        };
        if is_dir && target.starts_with(rel) {
            return true;
        }
        if recursive {
            rel.starts_with(target)
        } else {
            !is_dir && rel.len() == target.len() + 1 && rel.starts_with(target)
        }
    }
}

/// A set of targets. An empty scope means nothing; use `None` for the whole
/// root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope(pub Vec<Target>);

impl Scope {
    /// Whether an indexed file at `rel` (a `/`-separated relative path) is
    /// covered by this scope.
    pub fn covers_file(&self, rel: &str) -> bool {
        let parts: Vec<String> = rel.split('/').map(str::to_string).collect();
        self.0.iter().any(|t| t.admits(&parts, false))
    }

    fn admits(&self, rel: &[String], is_dir: bool) -> bool {
        self.0.iter().any(|t| t.admits(rel, is_dir))
    }
}

pub fn walk_files(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    visit: impl FnMut(WalkedFile) -> ControlFlow<()>,
) {
    walk_scoped(root, deny, None, visit);
}

pub fn walk_scoped(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    scope: Option<Arc<Scope>>,
    visit: impl FnMut(WalkedFile) -> ControlFlow<()>,
) {
    walk_scoped_until(root, deny, scope, &CancellationToken::new(), visit);
}

pub fn walk_scoped_until(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    scope: Option<Arc<Scope>>,
    cancelled: &CancellationToken,
    mut visit: impl FnMut(WalkedFile) -> ControlFlow<()>,
) {
    if cancelled.is_cancelled() {
        return;
    }
    for entry in walker(root, deny, scope, cancelled).flatten() {
        if cancelled.is_cancelled() {
            break;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&root.path) else {
            continue;
        };
        let Some(rel) = RelPath::from_relative_path(rel) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let file = WalkedFile {
            rel,
            size: meta.len(),
            modified,
        };
        if visit(file).is_break() {
            break;
        }
    }
}

/// The directories a walk enters, the root (or the scope's way down to its
/// targets) included: exactly the directories whose files are indexed, so
/// exactly the ones worth watching. Symlinks are never followed, so a link
/// out of the root is never visited.
pub fn walk_dirs_until(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    scope: Option<Arc<Scope>>,
    cancelled: &CancellationToken,
    mut visit: impl FnMut(&Path) -> ControlFlow<()>,
) {
    if cancelled.is_cancelled() {
        return;
    }
    for entry in walker(root, deny, scope, cancelled).flatten() {
        if cancelled.is_cancelled() {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        if visit(entry.path()).is_break() {
            break;
        }
    }
}

fn walker(
    root: &RootHandle,
    deny: &Arc<DenyList>,
    scope: Option<Arc<Scope>>,
    cancelled: &CancellationToken,
) -> ignore::Walk {
    let filter_cancelled = cancelled.clone();
    let filter_deny = Arc::clone(deny);
    let root_path = root.path.clone();
    WalkBuilder::new(&root.path)
        .follow_links(false)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .parents(false)
        .require_git(false)
        .same_file_system(false)
        .filter_entry(move |entry| {
            // Yield one entry to the outer loop so it can stop. Filtering
            // everything out would still visit every sibling in a huge folder.
            if filter_cancelled.is_cancelled() {
                return true;
            }
            if filter_deny.is_denied(entry.path()) {
                return false;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if is_dir && entry.depth() > 0 && skipped_dir(entry.path()) {
                return false;
            }
            let Some(scope) = &scope else {
                return true;
            };
            let Some(rel) = relative(&root_path, entry.path()) else {
                return false;
            };
            if rel.is_empty() {
                return true;
            }
            scope.admits(&rel, is_dir)
        })
        .build()
}

/// Folders skipped wherever they are: package managers' dependencies and
/// bytecode caches. (Hidden folders, `.git`, `.venv` and `.cache` among
/// them, are skipped anyway.)
pub(crate) const ALWAYS_SKIPPED: &[&str] = &["node_modules", "bower_components", "__pycache__"];

/// Folders skipped when the folder holding them is a software project:
/// build output, vendored dependencies and scratch space. Elsewhere, in
/// Documents say, a folder called `build` or `vendor` is somebody's work.
const PROJECT_OUTPUT: &[&str] = &[
    "target", "build", "dist", "out", "vendor", "tmp", "coverage", "_build",
];

/// Files that make a folder a software project.
pub(crate) const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "Gemfile",
    "composer.json",
    "pyproject.toml",
    "setup.py",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "CMakeLists.txt",
    "Makefile",
    "meson.build",
    "mix.exs",
    "Package.swift",
    "deno.json",
];

/// Whether a directory below a shared folder is left out of indexing and
/// watching even when no ignore file says so: dependencies, build output,
/// caches (a `CACHEDIR.TAG`, which Cargo's `target` has too), Python virtual
/// environments, and linked Git worktrees (a `.git` file, not a folder).
pub fn skipped_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if ALWAYS_SKIPPED.contains(&name) {
        return true;
    }
    if PROJECT_OUTPUT.contains(&name)
        && path
            .parent()
            .is_some_and(|parent| PROJECT_MARKERS.iter().any(|m| parent.join(m).exists()))
    {
        return true;
    }
    path.join("CACHEDIR.TAG").is_file()
        || path.join("pyvenv.cfg").is_file()
        || linked_worktree(path)
}

/// A linked worktree has a `.git` file naming a folder under the main
/// checkout's `.git/worktrees`. A submodule has a `.git` file too, naming
/// `.git/modules`, and is kept.
fn linked_worktree(path: &Path) -> bool {
    use std::io::Read;
    let git = path.join(".git");
    if !std::fs::symlink_metadata(&git).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut head = Vec::with_capacity(512);
    let Ok(file) = std::fs::File::open(&git) else {
        return false;
    };
    if file.take(4096).read_to_end(&mut head).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&head).replace('\\', "/");
    let Some(target) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(str::trim)
    else {
        return false;
    };
    worktree_gitdir(target)
}

/// Whether a `gitdir:` target is a linked worktree's metadata: a Git
/// directory (`.git`, or a bare `name.git`) followed by `worktrees`. A
/// submodule's lives under `.git/modules`, even where a folder on its path
/// is called `worktrees`.
fn worktree_gitdir(target: &str) -> bool {
    let parts: Vec<&str> = target.split('/').filter(|p| !p.is_empty()).collect();
    let git_dir = |p: &str| p == ".git" || p.ends_with(".git");
    let under = |next: &str| {
        parts
            .windows(2)
            .rposition(|w| git_dir(w[0]) && w[1] == next)
    };
    match (under("worktrees"), under("modules")) {
        (Some(worktree), Some(module)) => worktree > module,
        (Some(_), None) => true,
        _ => false,
    }
}

fn relative(root: &Path, path: &Path) -> Option<Vec<String>> {
    let rel = path.strip_prefix(root).ok()?;
    rel.components()
        .map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_worktrees_are_told_from_submodules_by_their_gitdir() {
        assert!(worktree_gitdir("/src/app/.git/worktrees/feature"));
        assert!(worktree_gitdir("../app/.git/worktrees/feature"));
        assert!(worktree_gitdir("/srv/app.git/worktrees/feature"));
        assert!(worktree_gitdir("C:/src/app/.git/worktrees/feature"));
        assert!(!worktree_gitdir("../../.git/modules/sub"));
        // A submodule in a folder called `worktrees`.
        assert!(!worktree_gitdir("../../.git/modules/worktrees/plugin"));
        assert!(!worktree_gitdir("/src/worktrees/app/.git"));
    }

    fn rel(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    fn parts(s: &str) -> Vec<String> {
        s.split('/').map(str::to_string).collect()
    }

    #[test]
    fn targets_admit_their_files_and_the_way_there() {
        let deep = Target::Recursive(rel("a/b"));
        assert!(deep.admits(&parts("a"), true));
        assert!(deep.admits(&parts("a/b"), true));
        assert!(deep.admits(&parts("a/b/c/d.txt"), false));
        assert!(!deep.admits(&parts("a/x.txt"), false));
        assert!(!deep.admits(&parts("a/c"), true));

        let flat = Target::Shallow(rel("a"));
        assert!(flat.admits(&parts("a"), true));
        assert!(flat.admits(&parts("a/x.txt"), false));
        assert!(!flat.admits(&parts("a/sub"), true));
        assert!(!flat.admits(&parts("a/sub/y.txt"), false));
        assert!(!flat.admits(&parts("b.txt"), false));

        let top = Target::Shallow(RelPath::default());
        assert!(top.admits(&parts("x.txt"), false));
        assert!(!top.admits(&parts("a/x.txt"), false));

        let scope = Scope(vec![deep, flat]);
        assert!(scope.covers_file("a/b/c.txt"));
        assert!(scope.covers_file("a/x.txt"));
        assert!(!scope.covers_file("z.txt"));
    }

    #[test]
    fn scoped_walks_visit_only_the_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        for dir in ["a/b", "a/c", "d"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        for file in [
            "top.txt",
            "a/one.txt",
            "a/b/two.txt",
            "a/c/three.txt",
            "d/four.txt",
        ] {
            std::fs::write(base.join(file), "x").unwrap();
        }
        std::fs::write(base.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(base.join("a/b/ignored")).unwrap();
        std::fs::write(base.join("a/b/ignored/five.txt"), "x").unwrap();
        let root = RootHandle::open(&crate::config::Root {
            bots: Vec::new(),
            id: "docs".into(),
            label: "Docs".into(),
            path: base,
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        })
        .unwrap();
        let deny = Arc::new(DenyList::new(Vec::<String>::new()).unwrap());
        let collect = |scope: Option<Scope>| {
            let mut seen = Vec::new();
            walk_scoped(&root, &deny, scope.map(Arc::new), |f| {
                seen.push(f.rel.as_string());
                ControlFlow::Continue(())
            });
            seen.sort();
            seen
        };
        assert_eq!(
            collect(None),
            [
                "a/b/two.txt",
                "a/c/three.txt",
                "a/one.txt",
                "d/four.txt",
                "top.txt"
            ]
        );
        assert_eq!(
            collect(Some(Scope(vec![Target::Recursive(rel("a/b"))]))),
            ["a/b/two.txt"],
            "the root's .gitignore still applies"
        );
        assert_eq!(
            collect(Some(Scope(vec![
                Target::Shallow(rel("a")),
                Target::Shallow(RelPath::default())
            ]))),
            ["a/one.txt", "top.txt"]
        );
    }

    #[test]
    fn dependencies_build_output_and_worktrees_are_not_walked() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        for (file, text) in [
            ("app/package.json", "{}"),
            ("app/index.js", "x"),
            ("app/node_modules/dep/index.js", "x"),
            ("app/dist/bundle.js", "x"),
            ("app/build/out.js", "x"),
            ("app/.venv/lib/site.py", "x"),
            ("app/venv/pyvenv.cfg", "x"),
            ("app/venv/lib/site.py", "x"),
            (
                "app/cache/CACHEDIR.TAG",
                "Signature: 8a477f597d28d172789f06886806bc55",
            ),
            ("app-wt/.git", "gitdir: /src/app/.git/worktrees/app-wt\n"),
            ("app-wt/index.js", "x"),
            ("docs/build/plan.md", "x"),
            ("docs/dist/list.md", "x"),
        ] {
            let path = base.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let root = RootHandle::open(&crate::config::Root {
            bots: Vec::new(),
            id: "code".into(),
            label: "Code".into(),
            path: base,
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            read_budget: None,
        })
        .unwrap();
        let deny = Arc::new(DenyList::new(Vec::<String>::new()).unwrap());
        let mut seen = Vec::new();
        walk_files(&root, &deny, |f| {
            seen.push(f.rel.as_string());
            ControlFlow::Continue(())
        });
        seen.sort();
        assert_eq!(
            seen,
            [
                "app/index.js",
                "app/package.json",
                "docs/build/plan.md",
                "docs/dist/list.md"
            ]
        );
    }
}
