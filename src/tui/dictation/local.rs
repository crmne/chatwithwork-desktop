//! A killable, bounded local inference request. No audio touches the daemon.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{Failure, Helper};
use crate::dictation::Model;

const MAX_RESPONSE: u64 = 512 * 1024;

fn failed() -> Failure {
    Failure::new(
        "local_transcription",
        "Local transcription failed. Check the downloaded model in General settings. Your audio was not uploaded.",
    )
}

pub(super) fn transcribe(
    helper: &Path,
    model: Model,
    audio: Vec<u8>,
    cancel: &AtomicBool,
) -> Result<String, Failure> {
    run(helper, model, audio, cancel, Duration::from_secs(600))
}

fn run(
    helper: &Path,
    model: Model,
    audio: Vec<u8>,
    cancel: &AtomicBool,
    deadline: Duration,
) -> Result<String, Failure> {
    let mut command = Command::new(helper);
    command
        .args(["--transcribe-local", model.id()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Helper(command.spawn().map_err(|_| failed())?);
    let mut stdin = child.0.stdin.take().ok_or_else(failed)?;
    let stdout = child.0.stdout.take().ok_or_else(failed)?;
    let (tx, rx) = mpsc::sync_channel(2);
    let sent = tx.clone();
    std::thread::Builder::new()
        .name("cww-speech-input".into())
        .spawn(move || {
            let result = stdin.write_all(&audio).map(|()| Vec::new());
            drop(stdin);
            let _ = sent.send(result);
        })
        .map_err(|_| failed())?;
    std::thread::Builder::new()
        .name("cww-speech-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        })
        .map_err(|_| failed())?;
    let started = Instant::now();
    let mut completed = 0;
    let mut output = None;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(String::new());
        }
        if started.elapsed() > deadline {
            return Err(failed());
        }
        if completed < 2 {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(bytes)) => {
                    completed += 1;
                    if bytes.len() as u64 > MAX_RESPONSE {
                        return Err(failed());
                    }
                    if !bytes.is_empty() {
                        output = Some(bytes);
                    }
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(failed()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        if completed == 2 {
            match child.0.try_wait().map_err(|_| failed())? {
                Some(status) if status.success() => {
                    #[derive(serde::Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Reply {
                        text: String,
                    }
                    let reply: Reply = serde_json::from_slice(&output.ok_or_else(failed)?)
                        .map_err(|_| failed())?;
                    if reply.text.len() > 64 * 1024 {
                        return Err(failed());
                    }
                    return Ok(reply.text);
                }
                Some(_) => return Err(failed()),
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn local_results_are_bounded_and_stalled_helpers_are_reaped() {
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join("speech");
        for (body, expected) in [
            (
                "[ \"$1\" = --transcribe-local ] && [ \"$2\" = tiny ] || exit 2\ncat >/dev/null\nprintf '%s' '{\"text\":\"hello 🌍\"}'",
                Some("hello 🌍"),
            ),
            (
                "cat >/dev/null\nprintf '%s' '{\"text\":\"wrong\",\"extra\":true}'",
                None,
            ),
            (
                "cat >/dev/null\nprintf '%s' '{\"text\":\"wrong\"}'\nexit 1",
                None,
            ),
            ("cat >/dev/null\nwhile :; do :; done", None),
        ] {
            crate::test_process::write_script(&helper, body);
            let started = Instant::now();
            let result = run(
                &helper,
                Model::Tiny,
                vec![1; 100_000],
                &AtomicBool::new(false),
                Duration::from_millis(250),
            );
            assert_eq!(result.ok().as_deref(), expected);
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        crate::test_process::write_script(&helper, "while :; do :; done");
        assert_eq!(
            run(
                &helper,
                Model::Tiny,
                vec![],
                &AtomicBool::new(true),
                Duration::from_secs(1)
            )
            .unwrap(),
            ""
        );
    }
}
