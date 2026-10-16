//! On-demand dictation shared by the desktop and terminal composers.
//! The installed app records through pipes, keeping audio libraries out of
//! the static daemon. Only the daemon sends audio to the paired server.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::chat::{Chats, Failure};

pub mod protocol;

pub const HELP: &str = "Dictate using credits. Audio is transcribed in the EU through your Chat with Work server. Review the words before sending.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Recording,
    Transcribing,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Dictate",
            Self::Starting => "Checking dictation (uses credits)…",
            Self::Recording => {
                "Listening. Escape stops. EU transcription uses credits (up to 5 minutes)."
            }
            Self::Transcribing => "Transcribing… Escape to discard.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Recording,
    Level { elapsed_ms: u32, level: u16 },
    Transcribing,
    Finished(Result<String, Failure>),
}

/// UI state uses a generation so a late transcription cannot enter a different
/// chat or a draft that has already been sent. Cancellation always invalidates it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub phase: Phase,
    pub elapsed_ms: u32,
    pub levels: [u16; 24],
    generation: u64,
}

impl State {
    pub fn elapsed(&self) -> String {
        let seconds = self.elapsed_ms / 1000;
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }

    pub fn start(&mut self) -> u64 {
        self.generation += 1;
        self.phase = Phase::Starting;
        self.elapsed_ms = 0;
        self.levels = [0; 24];
        self.generation
    }

    pub fn cancel(&mut self) {
        self.generation += 1;
        self.phase = Phase::Idle;
    }

    pub fn update(&mut self, generation: u64, update: Update) -> Option<Result<String, Failure>> {
        if generation != self.generation || self.phase == Phase::Idle {
            return None;
        }
        match update {
            Update::Recording if self.phase == Phase::Starting => self.phase = Phase::Recording,
            Update::Recording => {}
            Update::Level { elapsed_ms, level }
                if self.phase == Phase::Recording
                    && elapsed_ms >= self.elapsed_ms
                    && elapsed_ms <= protocol::MAX_MS
                    && level <= 1000 =>
            {
                self.elapsed_ms = elapsed_ms;
                self.levels.rotate_left(1);
                self.levels[23] = level;
            }
            Update::Level { .. } => {}
            Update::Transcribing => self.phase = Phase::Transcribing,
            Update::Finished(result) => {
                self.phase = Phase::Idle;
                return Some(result);
            }
        }
        None
    }
}

pub struct Recording {
    stop: mpsc::Sender<()>,
    cancelled: Arc<AtomicBool>,
}

impl Recording {
    /// `helper` is the app's own executable, or its installed sibling in the TUI.
    pub fn start(
        chats: Chats,
        helper: PathBuf,
        emit: impl Fn(Update) + Send + 'static,
    ) -> Result<Self, Failure> {
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        std::thread::Builder::new()
            .name("cww-dictation".into())
            .spawn(move || {
                let result = (|| {
                    let access = chats.dictation()?;
                    if !access.cloud {
                        return Err(Failure::new(
                            "unavailable",
                            access.reason.unwrap_or_else(|| {
                                "Dictation isn't available on this server.".into()
                            }),
                        ));
                    }
                    if cancel.load(Ordering::SeqCst) {
                        return Ok(String::new());
                    }
                    let audio = capture(helper, rx, &cancel, &emit)?;
                    if audio.is_empty() || cancel.load(Ordering::SeqCst) {
                        return Ok(String::new());
                    }
                    emit(Update::Transcribing);
                    chats.dictate(&audio, "audio/wav")
                })();
                if !cancel.load(Ordering::SeqCst) {
                    emit(Update::Finished(result));
                }
            })
            .map_err(|_| Failure::new("unavailable", "Could not start dictation. Try again."))?;
        Ok(Self {
            stop: tx,
            cancelled,
        })
    }

    pub fn stop(&self) {
        let _ = self.stop.send(());
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// No PATH lookup: use the app installed beside this command.
pub fn installed_helper() -> Result<PathBuf, Failure> {
    let path = std::env::current_exe().map_err(|_| microphone_failure())?;
    let helper = path.with_file_name(if cfg!(windows) {
        "cww-app.exe"
    } else {
        "cww-app"
    });
    if cfg!(windows) && !helper.is_file() {
        Ok(path.with_file_name("Chat with Work.exe"))
    } else {
        Ok(helper)
    }
}

struct Helper(Child);

enum CaptureEvent {
    Progress(Update),
    Finished(Result<Vec<u8>, Failure>),
}

impl Drop for Helper {
    fn drop(&mut self) {
        // Exact child only; also closes the microphone if the UI exits or cancels.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn capture(
    helper: PathBuf,
    stop: mpsc::Receiver<()>,
    cancel: &AtomicBool,
    emit: &impl Fn(Update),
) -> Result<Vec<u8>, Failure> {
    let mut command = Command::new(helper);
    command
        .arg("--record-audio-progress")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut helper = Helper(command.spawn().map_err(|_| {
        Failure::new(
            "unavailable",
            "Install the Chat with Work desktop app beside cww to use dictation.",
        )
    })?);
    let stdout = helper.0.stdout.take().ok_or_else(microphone_failure)?;
    let (tx, rx) = mpsc::sync_channel(4);
    std::thread::Builder::new()
        .name("cww-microphone".into())
        .spawn(move || {
            let result = protocol::read(stdout, |update| {
                let _ = tx.send(CaptureEvent::Progress(update));
            });
            let _ = tx.send(CaptureEvent::Finished(result));
        })
        .map_err(|_| microphone_failure())?;
    let started = Instant::now();
    let mut ready = false;
    let mut stopped = false;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        if !stopped && stop.try_recv().is_ok() {
            if let Some(mut input) = helper.0.stdin.take() {
                input
                    .write_all(b"stop\n")
                    .map_err(|_| microphone_failure())?;
            }
            stopped = true;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(CaptureEvent::Progress(update)) => {
                ready |= update == Update::Recording;
                emit(update);
            }
            Ok(CaptureEvent::Finished(result)) => {
                let audio = result?;
                // EOF means the helper closed its stream. Reap it with a bound.
                let exit = Instant::now();
                loop {
                    if let Some(status) = helper.0.try_wait().map_err(|_| microphone_failure())? {
                        return if status.success() {
                            Ok(audio)
                        } else {
                            Err(microphone_failure())
                        };
                    }
                    if exit.elapsed() > Duration::from_secs(2) {
                        return Err(microphone_failure());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(microphone_failure()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if started.elapsed() > Duration::from_secs(if ready { 365 } else { 60 }) {
            return Err(Failure::new(
                "timeout",
                "The microphone didn't respond. Check its permission and try again.",
            ));
        }
    }
}

fn validate_audio(bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    if !bytes.is_empty()
        && (bytes.len() > crate::chats::MAX_AUDIO_BYTES
            || bytes.len() < 44
            || &bytes[..4] != b"RIFF"
            || &bytes[8..12] != b"WAVE")
    {
        return Err(microphone_failure());
    }
    Ok(bytes)
}

fn microphone_failure() -> Failure {
    Failure::new(
        "microphone",
        "Couldn't record. Check your microphone and allow Chat with Work to use it in system settings.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_words_never_enter_a_new_draft() {
        let mut state = State::default();
        let first = state.start();
        state.update(first, Update::Recording);
        state.cancel();
        let next = state.start();
        assert!(
            state
                .update(first, Update::Finished(Ok("old draft".into())))
                .is_none()
        );
        assert_eq!(state.phase, Phase::Starting);
        state.update(next, Update::Transcribing);
        assert_eq!(
            state.update(next, Update::Finished(Ok("new draft".into()))),
            Some(Ok("new draft".into()))
        );
        assert_eq!(state.phase, Phase::Idle);
        assert!(state.update(next, Update::Recording).is_none());
    }

    #[test]
    fn waveform_keeps_recent_levels_and_ignores_old_or_late_updates() {
        let mut state = State::default();
        let generation = state.start();
        state.update(generation, Update::Recording);
        for n in 1..=30 {
            state.update(
                generation,
                Update::Level {
                    elapsed_ms: n * 70,
                    level: n as u16,
                },
            );
        }
        assert_eq!(state.levels, std::array::from_fn(|index| index as u16 + 7));
        assert_eq!(state.elapsed(), "0:02");
        let current = state.clone();
        state.update(
            generation,
            Update::Level {
                elapsed_ms: 70,
                level: 100,
            },
        );
        assert_eq!(state, current);
        state.update(generation, Update::Transcribing);
        state.update(
            generation,
            Update::Level {
                elapsed_ms: 3000,
                level: 100,
            },
        );
        assert_eq!(state.levels, current.levels);
        state.cancel();
        state.start();
        state.update(
            generation,
            Update::Level {
                elapsed_ms: 4000,
                level: 200,
            },
        );
        assert_eq!(state.levels, [0; 24]);
        assert_eq!(state.elapsed_ms, 0);
    }
}
