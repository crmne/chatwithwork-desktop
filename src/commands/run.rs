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
    let state = loop {
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
    }
    state
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
