//! The Whisper model's download for its first local dictation. It runs
//! while the person speaks, and once per model in this process: a dictation
//! cancelled while it runs leaves it going, and the next one picks it up
//! where it is. The file is verified before it's used (`Model::download`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Failure, Update};
use crate::dictation::Model;
use crate::paths::Paths;

struct Download {
    model: Model,
    bytes: AtomicU64,
    outcome: Mutex<Option<Result<(), String>>>,
}

static RUNNING: Mutex<Vec<Arc<Download>>> = Mutex::new(Vec::new());

/// One dictation's view of the download: it tells only progress it hasn't
/// told yet.
pub(super) struct Fetch {
    download: Arc<Download>,
    told: AtomicU64,
}

/// Starts getting `model`, or joins the download already getting it. None
/// when it's already here.
pub(super) fn fetch(model: Model, paths: &Paths) -> Option<Fetch> {
    fetch_with(model, paths, move |paths, cancelled, progress| {
        model.download(paths, cancelled, progress)
    })
}

fn fetch_with(
    model: Model,
    paths: &Paths,
    download: impl FnOnce(&Paths, &AtomicBool, &dyn Fn(u64)) -> anyhow::Result<()> + Send + 'static,
) -> Option<Fetch> {
    if model.downloaded(paths) {
        return None;
    }
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = running.iter().find(|d| d.model == model) {
        return Some(Fetch::new(existing.clone()));
    }
    let shared = Arc::new(Download {
        model,
        bytes: AtomicU64::new(0),
        outcome: Mutex::new(None),
    });
    let worker = shared.clone();
    let paths = paths.clone();
    let spawned = std::thread::Builder::new()
        .name("cww-model-download".into())
        .spawn(move || {
            // Never cancelled: a dictation that stops only stops waiting.
            let cancelled = AtomicBool::new(false);
            let result = download(&paths, &cancelled, &|bytes| {
                worker.bytes.store(bytes, Ordering::Relaxed);
            })
            .map_err(|e| format!("{e:#}"));
            let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
            running.retain(|d| !Arc::ptr_eq(d, &worker));
            *worker.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
    match spawned {
        Ok(_) => running.push(shared.clone()),
        Err(_) => {
            *shared.outcome.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Err("Could not start the download.".into()));
        }
    }
    Some(Fetch::new(shared))
}

impl Fetch {
    fn new(download: Arc<Download>) -> Self {
        Self {
            download,
            told: AtomicU64::new(u64::MAX),
        }
    }

    /// Tells the progress, at most once a percent.
    pub(super) fn report(&self, emit: &impl Fn(Update)) {
        let total = self.download.model.bytes();
        let bytes = self.download.bytes.load(Ordering::Relaxed);
        let percent = bytes * 100 / total;
        if self.told.swap(percent, Ordering::Relaxed) != percent {
            emit(Update::Downloading { bytes, total });
        }
    }

    /// Waits for the download after the recording has stopped, telling its
    /// progress, until it ends or the dictation is cancelled.
    pub(super) fn wait(&self, cancel: &AtomicBool, emit: &impl Fn(Update)) -> Result<(), Failure> {
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            self.report(emit);
            if let Some(outcome) = self
                .download
                .outcome
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                let total = self.download.model.bytes();
                emit(Update::Downloading {
                    bytes: total,
                    total,
                });
                return outcome.map_err(|error| {
                    Failure::new(
                        "local_model",
                        format!(
                            "Couldn't get the Whisper model: {error} Your recording wasn't uploaded."
                        ),
                    )
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_dictation_joins_the_download_already_running_and_a_failed_one_is_tried_again() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        let (release, gate) = mpsc::channel::<()>();
        let first = fetch_with(Model::Small, &paths, move |_, _, progress| {
            progress(Model::Small.bytes() / 2);
            gate.recv().unwrap();
            anyhow::bail!("Could not download the speech model.")
        })
        .unwrap();
        let second = fetch_with(Model::Small, &paths, |_, _, _| panic!("started twice")).unwrap();
        assert!(Arc::ptr_eq(&first.download, &second.download));
        let updates = std::cell::RefCell::new(Vec::new());
        let emit = |update| updates.borrow_mut().push(update);
        while first.download.bytes.load(Ordering::Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        first.report(&emit);
        first.report(&emit);
        assert_eq!(
            *updates.borrow(),
            [Update::Downloading {
                bytes: Model::Small.bytes() / 2,
                total: Model::Small.bytes()
            }]
        );
        // A cancelled dictation stops waiting; the download goes on.
        assert!(second.wait(&AtomicBool::new(true), &emit).is_ok());
        release.send(()).unwrap();
        let failure = first.wait(&AtomicBool::new(false), &emit).unwrap_err();
        assert_eq!(failure.code, "local_model");
        assert!(
            failure.message.contains("wasn't uploaded"),
            "{}",
            failure.message
        );
        let again = fetch_with(Model::Small, &paths, |_, _, _| Ok(())).unwrap();
        assert!(!Arc::ptr_eq(&first.download, &again.download));
        assert!(again.wait(&AtomicBool::new(false), &|_| {}).is_ok());
    }
}
