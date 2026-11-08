//! A command's memory limit on Linux: a cgroup of its own with `memory.max`,
//! which counts the memory the command's processes really use.
//!
//! An rlimit (`RLIMIT_AS`, `RLIMIT_DATA`) counts address space a process
//! reserves, not memory it uses: rustc and LLVM, a JVM, Go and V8 reserve far
//! more than they touch, and failed to allocate under a 4 GB rlimit using a
//! fraction of it. So Linux runs get no memory rlimit, and the limit is the
//! kernel's own count of each run's memory, swap included.
//!
//! A cgroup needs a part of the tree this process may change, which systemd
//! gives a service with `Delegate=yes` (the packaged `cww.service` and the
//! one `cww daemon install` writes). There the first run moves this service's
//! own processes into a leaf, `cww-daemon` (a cgroup that holds processes
//! can't hand controllers to its children), and each run gets `run-<id>`
//! beside it, joined by the sandbox before the command starts
//! (`resources.cgroup`). Elsewhere (`cww daemon run` from a terminal, a
//! container) nothing is changed, and runs have no memory limit.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const ROOT: &str = "/sys/fs/cgroup";
/// The leaf this service's own processes move to.
const LEAF: &str = "cww-daemon";

/// One run's cgroup, removed when it's dropped, along with anything still
/// in it.
#[derive(Debug)]
pub struct RunCgroup {
    path: PathBuf,
}

impl RunCgroup {
    /// A cgroup for run `id` that holds it to `memory_mb` megabytes of
    /// memory, swap included, and ends the whole run when it's reached.
    /// `None` where this process has no delegated cgroup, or the kernel has
    /// no memory controller for it.
    pub fn make(id: &str, memory_mb: u64) -> Option<Self> {
        let parent = parent()?;
        let path = parent.join(format!("run-{id}"));
        let made = (|| -> std::io::Result<()> {
            std::fs::create_dir(&path)?;
            let bytes = memory_mb.saturating_mul(1024 * 1024);
            std::fs::write(path.join("memory.max"), bytes.to_string())?;
            // Swap would let it go past the limit, only slower.
            if path.join("memory.swap.max").exists() {
                std::fs::write(path.join("memory.swap.max"), "0")?;
            }
            // One process past the limit ends them all, as the run ends.
            std::fs::write(path.join("memory.oom.group"), "1")?;
            Ok(())
        })();
        match made {
            Ok(()) => Some(Self { path }),
            Err(e) => {
                tracing::warn!(error = %e.kind(), "a command's cgroup wasn't made: it has no memory limit");
                let _ = std::fs::remove_dir(&path);
                None
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the kernel ended something in it for reaching its limit.
    pub fn ran_out_of_memory(&self) -> bool {
        std::fs::read_to_string(self.path.join("memory.events"))
            .is_ok_and(|events| out_of_memory(&events))
    }
}

impl Drop for RunCgroup {
    fn drop(&mut self) {
        // Anything the command left behind ends with it.
        let _ = std::fs::write(self.path.join("cgroup.kill"), "1");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir(&self.path) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    tracing::warn!(error = %e.kind(), "a command's cgroup wasn't removed");
                    return;
                }
            }
        }
    }
}

/// Whether `memory.events` says the run reached its own limit (`oom`) and
/// the kernel ended a process for it. A kill from the computer running out,
/// or from a cgroup above, counts no `oom` here.
fn out_of_memory(events: &str) -> bool {
    let count = |name: &str| {
        events
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find(|(key, _)| *key == name)
            .and_then(|(_, n)| n.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    count("oom") > 0 && count("oom_kill") + count("oom_group_kill") > 0
}

/// Where runs' cgroups go, set up once.
fn parent() -> Option<&'static Path> {
    static PARENT: OnceLock<Option<PathBuf>> = OnceLock::new();
    PARENT
        .get_or_init(|| {
            let own = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
            let parent = delegated(&own).and_then(|cgroup| set_up(&cgroup));
            if parent.is_none() {
                tracing::info!("no cgroup to hold commands' memory: they have no memory limit");
            }
            parent
        })
        .as_deref()
}

/// The delegated cgroup this process belongs to, from `/proc/self/cgroup`:
/// its own, or the one above it once this process moved to [`LEAF`].
fn delegated(own: &str) -> Option<PathBuf> {
    cgroup_of(own).filter(|cgroup| is_delegated(cgroup) || unit_is_delegated(cgroup))
}

/// Whether systemd's unit for `cgroup` (`cww.service`, a scope) says
/// `Delegate=yes`. A user manager marks no attribute on what it delegates,
/// so it's asked.
fn unit_is_delegated(cgroup: &Path) -> bool {
    let Some(unit) = cgroup.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !(unit.ends_with(".service") || unit.ends_with(".scope")) {
        return false;
    }
    let mut systemctl = std::process::Command::new("systemctl");
    if cgroup
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with("user@"))
    {
        systemctl.arg("--user");
    }
    let Ok(mut child) = systemctl
        .args(["show", "--property=Delegate", "--value", unit])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    // A user manager that doesn't answer mustn't hold the run up.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
    let mut answer = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut answer);
    }
    answer.trim() == "yes"
}

/// The cgroup v2 a `/proc/<pid>/cgroup` names, above [`LEAF`].
fn cgroup_of(own: &str) -> Option<PathBuf> {
    let path = own.lines().find_map(|line| line.strip_prefix("0::"))?;
    let mut cgroup = Path::new(ROOT).join(path.trim().trim_start_matches('/'));
    if cgroup.file_name().is_some_and(|name| name == LEAF) {
        cgroup.pop();
    }
    Some(cgroup)
}

/// systemd marks a cgroup it delegated (`Delegate=yes`) with an extended
/// attribute where it can: `trusted.delegate` from the system's manager
/// (readable only with privilege), `user.delegate` from some.
fn is_delegated(cgroup: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(cgroup.as_os_str().as_bytes()) else {
        return false;
    };
    ["user.delegate", "trusted.delegate"].iter().any(|name| {
        let name = std::ffi::CString::new(*name).expect("no NUL");
        let mut value = [0u8; 8];
        // SAFETY: both strings are NUL-terminated and `value` is writable
        // for its length.
        let read = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        read > 0 && value[0] == b'1'
    })
}

/// Move this service's processes into [`LEAF`] and hand the memory
/// controller to the cgroups beside it. Answers where runs go.
fn set_up(cgroup: &Path) -> Option<PathBuf> {
    let controllers = std::fs::read_to_string(cgroup.join("cgroup.controllers")).ok()?;
    if !controllers.split_whitespace().any(|c| c == "memory") {
        return None;
    }
    let leaf = cgroup.join(LEAF);
    if let Err(e) = std::fs::create_dir(&leaf)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        tracing::warn!(error = %e.kind(), "couldn't make the daemon's own cgroup");
        return None;
    }
    // A process can start while they move: try a few times.
    for _ in 0..10 {
        let procs = std::fs::read_to_string(cgroup.join("cgroup.procs")).ok()?;
        if procs.trim().is_empty() {
            break;
        }
        for pid in procs.split_whitespace() {
            // One that just ended can't move.
            let _ = std::fs::write(leaf.join("cgroup.procs"), pid);
        }
    }
    match std::fs::write(cgroup.join("cgroup.subtree_control"), "+memory") {
        Ok(()) => Some(cgroup.to_path_buf()),
        Err(e) => {
            tracing::warn!(error = %e.kind(), "couldn't hand the memory controller to commands");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kill_at_the_limit_is_running_out_of_memory() {
        let quiet = "low 0\nhigh 0\nmax 12\noom 1\noom_kill 0\noom_group_kill 0\n";
        assert!(!out_of_memory(quiet));
        assert!(out_of_memory("low 0\nmax 40\noom 1\noom_kill 1\n"));
        assert!(out_of_memory("oom 1\noom_kill 0\noom_group_kill 1\n"));
        // Killed without reaching its own limit: the computer, or a cgroup
        // above, ran out.
        assert!(!out_of_memory("oom 0\noom_kill 1\n"));
        assert!(!out_of_memory(""));
    }

    #[test]
    fn the_daemons_own_leaf_reads_as_the_cgroup_above_it() {
        let service = "/sys/fs/cgroup/user.slice/user@1000.service/app.slice/cww.service";
        for own in [
            "0::/user.slice/user@1000.service/app.slice/cww.service\n",
            "0::/user.slice/user@1000.service/app.slice/cww.service/cww-daemon\n",
        ] {
            assert_eq!(cgroup_of(own), Some(PathBuf::from(service)), "{own}");
        }
        // cgroup v1 alone: nothing to hold memory with.
        assert_eq!(cgroup_of("1:name=systemd:/user.slice\n"), None);
    }
}
