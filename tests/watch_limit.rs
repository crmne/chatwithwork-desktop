//! The index's file watching holds few inotify watches, however big the
//! shared folder. Counted from the kernel's own record of this process's
//! watches (`/proc/self/fdinfo`), so this test runs in a process of its own.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cww::config::Root;
use cww::policy::DenyList;
use cww::reader::indexer::{IndexState, Indexer, IndexerSettings};
use cww::reader::safe_fs::RootHandle;
use cww::status::Changes;

/// Inotify watches this process holds, across all its inotify instances.
fn inotify_watches() -> usize {
    let mut total = 0;
    for fd in std::fs::read_dir("/proc/self/fd").unwrap().flatten() {
        let is_inotify = std::fs::read_link(fd.path())
            .is_ok_and(|target| target.to_string_lossy() == "anon_inode:inotify");
        if !is_inotify {
            continue;
        }
        let info = Path::new("/proc/self/fdinfo").join(fd.file_name());
        if let Ok(text) = std::fs::read_to_string(info) {
            total += text
                .lines()
                .filter(|l| l.starts_with("inotify wd:"))
                .count();
        }
    }
    total
}

/// Watches go when the watcher's thread notices it was dropped.
fn settle_at(expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while inotify_watches() != expected {
        assert!(
            Instant::now() < deadline,
            "{} watches, expected {expected}",
            inotify_watches()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn mkdirs(base: &Path, dirs: impl IntoIterator<Item = String>) {
    for dir in dirs {
        std::fs::create_dir_all(base.join(dir)).unwrap();
    }
}

/// `projects` code checkouts, each with sources, dependencies, build output
/// and Git's own folder, some with a build output link to a cache elsewhere
/// (as mbx makes). Returns how many directories the index walks.
fn generate(base: &Path, outside: &Path, projects: usize) -> usize {
    mkdirs(outside, (0..200).map(|i| format!("cache/{i}")));
    for p in 0..projects {
        let project = base.join(format!("project-{p}"));
        mkdirs(&project, ["src/a".to_string()]);
        std::fs::write(project.join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(project.join("src/a/lib.rs"), "pub fn f() {}\n").unwrap();
        mkdirs(
            &project,
            (0..50).map(|i| format!("node_modules/dep-{i}/lib")),
        );
        mkdirs(&project, (0..30).map(|i| format!("build/out-{i}")));
        mkdirs(&project, (0..30).map(|i| format!(".git/objects/{i:02x}")));
        if p < 20 {
            std::os::unix::fs::symlink(outside.join("cache"), project.join("target")).unwrap();
        }
    }
    // The shared folder, and each project with its src and src/a.
    1 + projects * 3
}

fn settings(base: &Path, max_watches: usize) -> IndexerSettings {
    let root = RootHandle::open(&Root {
        id: "code".into(),
        label: "Code".into(),
        path: base.to_path_buf(),
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
        max_file_bytes: 1024 * 1024,
        watch: true,
        max_watches,
        rescan: Duration::from_secs(3600),
    }
}

fn wait_ready(indexer: &Indexer) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while indexer.status().get("code").map(|s| s.state) != Some(IndexState::Ready) {
        assert!(Instant::now() < deadline, "the index never got ready");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn watches_stay_bounded_on_a_big_code_tree() {
    use notify::Watcher;

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("code");
    let outside = tmp.path().join("elsewhere");
    let walked = generate(&base, &outside, 200);
    let base = base.canonicalize().unwrap();
    assert_eq!(inotify_watches(), 0);

    // Before: one recursive watch, as the daemon used to set, follows every
    // folder and every link out of the shared folder. It takes over 25,000
    // of the system's watches, more than some systems allow at all, and
    // they're shared with every other program: measured only when asked
    // (`CWW_WATCH_BASELINE=1`).
    let baseline = std::env::var("CWW_WATCH_BASELINE").as_deref() == Ok("1");
    let before = if !baseline {
        0
    } else {
        let mut watcher =
            notify::recommended_watcher(|_: notify::Result<notify::Event>| {}).unwrap();
        watcher
            .watch(&base, notify::RecursiveMode::Recursive)
            .unwrap();
        inotify_watches()
    };
    settle_at(0);
    assert!(
        !baseline || before > 25_000,
        "a recursive watch took {before}"
    );

    // After: only what the index walks.
    let index = tmp.path().join("index");
    let indexer = Indexer::start(index.clone(), settings(&base, 8192), Changes::default());
    wait_ready(&indexer);
    let after = inotify_watches();
    eprintln!("{walked} folders indexed: {before} watches before, {after} after");
    assert_eq!(after, walked);
    drop(indexer);
    settle_at(0);

    // And never more than the limit, with the rest left to rescans.
    let indexer = Indexer::start(index, settings(&base, 100), Changes::default());
    wait_ready(&indexer);
    assert_eq!(inotify_watches(), 100);
    drop(indexer);
}
