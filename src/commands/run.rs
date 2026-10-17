//! One run's thread: start the sandbox, read its streams, keep its clock,
//! stop it when asked, and clean up after it.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Cell, Plan, State, Stream, lock};

/// How often the thread checks for the end, a stop and the clock.
const POLL: Duration = Duration::from_millis(50);
/// How long the streams may stay open after the command ends (something it
/// left running holds them) before they're closed.
const DRAIN: Duration = Duration::from_secs(1);
/// How often its processes are counted, on Linux.
const COUNT_EVERY: u32 = 5;
/// Processes of the sandbox's own (Bubblewrap and its init) that don't
/// count against the command's limit.
#[cfg(target_os = "linux")]
const SANDBOX_PROCESSES: usize = 2;

pub(super) fn spawn(
    plan: Plan,
    cell: Arc<Mutex<Cell>>,
    retire: impl FnOnce(&str) + Send + 'static,
) {
    let id = plan.id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("cww-run-{id}"))
        .spawn({
            let cell = cell.clone();
            move || {
                let state = run(&plan, &cell);
                lock(&cell).finish(state);
                let _ = std::fs::remove_dir_all(&plan.home);
                retire(&plan.id);
            }
        });
    if let Err(e) = spawned {
        lock(&cell).finish(State::Failed(format!(
            "couldn't start a thread for it: {e}"
        )));
    }
}

fn run(plan: &Plan, cell: &Arc<Mutex<Cell>>) -> State {
    if let Err(e) = make_home(plan) {
        return State::Failed(format!("couldn't make its home folder: {e}"));
    }
    let home = crate::paths::home_dir().ok();
    let request = plan.request_json(home.as_deref());
    let mut process = match mxc_sdk::__ffi::spawn_container_json(&request, false) {
        Ok(process) => process,
        Err(e) => {
            tracing::warn!(run = %plan.id, "the command sandbox didn't start");
            return State::Failed(format!("the sandbox couldn't start it: {e}"));
        }
    };
    let readers: Vec<_> = [
        (process.take_stdout(), Stream::Out),
        (process.take_stderr(), Stream::Err),
    ]
    .into_iter()
    .filter_map(|(reader, stream)| reader.map(|r| (r, stream)))
    .map(|(mut reader, stream)| {
        let cell = cell.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => lock(&cell).append(stream, &buffer[..n]),
                }
            }
        })
    })
    .collect();
    let closers = [process.stdout_closer(), process.stderr_closer()];
    let deadline = Instant::now() + Duration::from_secs(plan.limits.timeout_seconds.max(1));
    let mut ended = None;
    let mut polls: u32 = 0;
    let state = loop {
        polls = polls.wrapping_add(1);
        if polls.is_multiple_of(COUNT_EVERY)
            && too_many_processes(process.id(), plan.limits.processes)
        {
            tracing::info!(run = %plan.id, "a command reached its process limit");
            lock(cell).limit = Some("processes");
            let _ = process.kill();
            let _ = process.wait();
            break State::Stopped;
        }
        let stop = lock(cell).stop;
        if stop || Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            break if stop {
                State::Stopped
            } else {
                State::TimedOut
            };
        }
        match process.try_wait() {
            Ok(Some(code)) => {
                ended = Some(code);
                break State::Exited(code);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = process.kill();
                break State::Failed(format!("lost track of it: {e}"));
            }
        }
    };
    // Its output may still be in the pipes: give the readers a moment.
    let drained = Instant::now() + DRAIN;
    while readers.iter().any(|r| !r.is_finished()) && Instant::now() < drained {
        std::thread::sleep(Duration::from_millis(10));
    }
    for closer in closers.into_iter().flatten() {
        closer.close();
    }
    for reader in readers {
        let _ = reader.join();
    }
    if ended.is_some() {
        let _ = process.wait();
        let cpu = process
            .output_metadata()
            .and_then(|m| m.resource_usage)
            .and_then(|u| u.limit_exceeded);
        if cpu == Some(mxc_sdk::mxc_common::models::ResourceLimit::CpuTime) {
            lock(cell).limit = Some("cpu_time");
        }
    }
    state
}

/// Whether the sandbox started from `root` holds more than `limit`
/// processes. MXC limits processes only on Windows, so on Linux the daemon
/// counts them itself, from `/proc`, a few times a second. A limit of 0
/// means none.
#[cfg(target_os = "linux")]
fn too_many_processes(root: u32, limit: u32) -> bool {
    limit > 0 && descendants(root, &parents()) > limit as usize + SANDBOX_PROCESSES
}

#[cfg(not(target_os = "linux"))]
fn too_many_processes(_root: u32, _limit: u32) -> bool {
    false
}

/// Every process's parent, from `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
fn parents() -> Vec<(u32, u32)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| {
            let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            Some((pid, parent_in_stat(&stat)?))
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
/// The parent in a `stat` line: the field after the state, which follows
/// the name in parentheses (which can hold anything, parentheses too).
fn parent_in_stat(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(any(target_os = "linux", test))]
/// How many processes descend from `root`, from (pid, parent) pairs.
fn descendants(root: u32, parents: &[(u32, u32)]) -> usize {
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for &(pid, parent) in parents {
        children.entry(parent).or_default().push(pid);
    }
    let mut count = 0;
    let mut queue = vec![root];
    let mut seen = std::collections::HashSet::from([root]);
    while let Some(pid) = queue.pop() {
        for &child in children.get(&pid).map_or(&[][..], Vec::as_slice) {
            if seen.insert(child) {
                count += 1;
                queue.push(child);
            }
        }
    }
    count
}

/// The run's own home and temporary folder, private to this user.
fn make_home(plan: &Plan) -> std::io::Result<()> {
    for dir in [plan.home.clone(), plan.home.join("tmp")] {
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_is_read_past_a_name_with_parentheses() {
        assert_eq!(parent_in_stat("42 (sh) S 7 42 42 0"), Some(7));
        assert_eq!(parent_in_stat("43 (a) b) (c) R 42 1"), Some(42));
        assert_eq!(parent_in_stat("nonsense"), None);
    }

    #[test]
    fn descendants_count_the_whole_tree_and_nothing_else() {
        let tree = [(2, 1), (3, 2), (4, 2), (5, 4), (6, 1), (7, 99)];
        assert_eq!(descendants(2, &tree), 3);
        assert_eq!(descendants(1, &tree), 5);
        assert_eq!(descendants(7, &tree), 0);
    }
}
