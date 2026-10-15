//! On-demand dictation shared by the desktop and terminal composers.
//! The installed app records through pipes, keeping audio libraries out of
//! the static daemon. Only the daemon sends audio to the paired server.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::chat::{Chats, Failure};

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
    Transcribing,
    Finished(Result<String, Failure>),
}

/// UI state uses a generation so a late transcription cannot enter a different
/// chat or a draft that has already been sent. Cancellation always invalidates it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub phase: Phase,
    generation: u64,
}

impl State {
    pub fn start(&mut self) -> u64 {
        self.generation += 1;
        self.phase = Phase::Starting;
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
        .arg("--record-audio")
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
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("cww-microphone".into())
        .spawn(move || {
            let result = read_audio(stdout, || {
                let _ = tx.send(None);
            });
            let _ = tx.send(Some(result));
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
            Ok(None) => {
                ready = true;
                emit(Update::Recording);
            }
            Ok(Some(result)) => {
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

fn read_audio(reader: impl Read, ready: impl FnOnce()) -> Result<Vec<u8>, Failure> {
    let mut reader = BufReader::new(reader);
    let mut header = Vec::new();
    reader
        .by_ref()
        .take(64)
        .read_until(b'\n', &mut header)
        .map_err(|_| microphone_failure())?;
    if header != b"ready\n" {
        return Err(microphone_failure());
    }
    ready();
    let mut bytes = Vec::new();
    reader
        .take(crate::chats::MAX_AUDIO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| microphone_failure())?;
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
    fn pipe_protocol_accepts_silence_and_refuses_malformed_or_oversized_audio() {
        assert!(read_audio(&b"ready\n"[..], || {}).unwrap().is_empty());
        for bytes in [b"error\n".as_slice(), b"ready\nerror\n", b"not-ready\n"] {
            assert!(read_audio(bytes, || {}).is_err());
        }
        let mut audio = b"ready\nRIFF".to_vec();
        audio.extend_from_slice(&[0; 4]);
        audio.extend_from_slice(b"WAVE");
        audio.resize(6 + 44, 0);
        assert_eq!(read_audio(audio.as_slice(), || {}).unwrap().len(), 44);
        audio.resize(6 + crate::chats::MAX_AUDIO_BYTES + 1, 0);
        assert!(read_audio(audio.as_slice(), || {}).is_err());
    }
}
