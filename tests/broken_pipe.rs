//! A reader that closes early (`cww log | head`) ends the CLI quietly: no
//! panic, and the exit status a shell expects of a process SIGPIPE ended
//! (141 on Unix). Crash reports are off here, so a test never sends one.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

#[test]
fn a_reader_that_closes_early_ends_the_cli_quietly() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    // Far more than a pipe's buffer holds.
    let line = "{\"ts\":\"2026-11-04T00:00:00Z\",\"event\":\"connected\"}\n";
    std::fs::write(state.join("audit.jsonl"), line.repeat(50_000)).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cww"))
        .args(["log", "-n", "50000", "--json"])
        .env("CWW_HOME", home.path())
        // Never a real report from a test; the reporter's own refusal is
        // covered by `crash_reports::tests::a_closed_pipe_is_never_a_crash`.
        .env("CWW_CRASH_REPORTS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = [0u8; 64];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut first)
        .unwrap();
    // The reader goes away, as `head` does.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.code(), Some(141), "{stderr}");
}
