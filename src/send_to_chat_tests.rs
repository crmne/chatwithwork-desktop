//! `send_to_chat`: a shared file sent to a chat over HTTPS, resolved like a
//! read, within the copy budget, and checked again once it went.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::config::{Config, Limits, Root};
use crate::paths::Paths;
use crate::status::Changes;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    snapshots: PathBuf,
    reader: Reader,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("builds")).unwrap();
    fs::create_dir_all(base.join("private")).unwrap();
    fs::write(base.join("builds/app.AppImage"), vec![9u8; 30_000]).unwrap();
    fs::write(base.join("private/secret.txt"), "no").unwrap();
    let mut config = Config {
        roots: vec![Root {
            bots: Vec::new(),
            id: "builds".into(),
            label: "Builds".into(),
            path: base.join("builds"),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.enabled = false;
    let reader = Reader::new(
        &config,
        &Paths::under(&base.join("cww")),
        Changes::default(),
    )
    .unwrap();
    Fixture {
        _tmp: tmp,
        snapshots: base.join("snapshots"),
        base,
        reader,
    }
}

#[test]
fn sends_a_private_copy_of_the_checked_bytes_and_counts_it() {
    let f = fixture();
    let limiter = Limiter::new(Limits::default());
    let sent = std::cell::RefCell::new(None);
    let send = |path: &Path, size: u64, sha: &str| -> Result<Value, ToolError> {
        assert_eq!(fs::read(path).unwrap(), vec![9u8; 30_000]);
        *sent.borrow_mut() = Some((path.to_path_buf(), size, sha.to_string()));
        Ok(json!({ "path": "builds/app.AppImage", "size": size }))
    };
    let answer = send_to_chat(
        &f.reader,
        &limiter,
        &send,
        "builds:app.AppImage",
        &f.snapshots,
    )
    .unwrap();
    let (path, size, sha) = sent.into_inner().unwrap();
    // A copy in cww's own folder, never the shared path, removed once sent.
    assert!(path.starts_with(&f.snapshots), "{path:?}");
    assert!(!path.exists());
    assert!(fs::read_dir(&f.snapshots).unwrap().next().is_none());
    assert_eq!(size, 30_000);
    assert_eq!(sha.len(), 64);
    assert_eq!(answer["kept"]["size"], 30_000);
    assert_eq!(answer["path"], "builds:app.AppImage");
    assert!(
        !answer.to_string().contains(f.base.to_str().unwrap()),
        "the real path never goes back"
    );
}

#[test]
fn refuses_what_isnt_shared_without_sending_anything() {
    let f = fixture();
    let limiter = Limiter::new(Limits::default());
    let calls = std::cell::Cell::new(0);
    let send = |_: &Path, _: u64, _: &str| -> Result<Value, ToolError> {
        calls.set(calls.get() + 1);
        Ok(Value::Null)
    };
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        f.base.join("private/secret.txt"),
        f.base.join("builds/link.txt"),
    )
    .unwrap();
    for path in [
        "builds:../private/secret.txt",
        "private:secret.txt",
        "builds:missing.bin",
        "builds:",
    ] {
        assert!(
            send_to_chat(&f.reader, &limiter, &send, path, &f.snapshots).is_err(),
            "{path}"
        );
    }
    #[cfg(unix)]
    assert_eq!(
        send_to_chat(&f.reader, &limiter, &send, "builds:link.txt", &f.snapshots)
            .unwrap_err()
            .code,
        ErrorCode::Denied
    );
    assert_eq!(calls.get(), 0);
}

#[test]
fn keeps_to_the_hourly_copy_allowance() {
    let f = fixture();
    let limiter = Limiter::new(Limits {
        copy_bytes_per_hour: 40_000,
        ..Limits::default()
    });
    let send =
        |_: &Path, size: u64, _: &str| -> Result<Value, ToolError> { Ok(json!({ "size": size })) };
    assert!(
        send_to_chat(
            &f.reader,
            &limiter,
            &send,
            "builds:app.AppImage",
            &f.snapshots
        )
        .is_ok()
    );
    let error = send_to_chat(
        &f.reader,
        &limiter,
        &send,
        "builds:app.AppImage",
        &f.snapshots,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::RateLimited);
}

#[test]
fn a_file_that_changes_while_it_goes_sends_what_was_checked_and_a_failure_still_counts() {
    let f = fixture();
    let limiter = Limiter::new(Limits {
        copy_bytes_per_hour: 70_000,
        ..Limits::default()
    });
    let changing = |path: &Path, _: u64, _: &str| -> Result<Value, ToolError> {
        fs::write(f.base.join("builds/app.AppImage"), vec![1u8; 30_000]).unwrap();
        assert_eq!(fs::read(path).unwrap(), vec![9u8; 30_000]);
        Ok(Value::Null)
    };
    send_to_chat(
        &f.reader,
        &limiter,
        &changing,
        "builds:app.AppImage",
        &f.snapshots,
    )
    .unwrap();
    let refusing = |_: &Path, _: u64, _: &str| -> Result<Value, ToolError> {
        Err(ToolError::new(ErrorCode::Exists, "already there"))
    };
    assert_eq!(
        send_to_chat(
            &f.reader,
            &limiter,
            &refusing,
            "builds:app.AppImage",
            &f.snapshots
        )
        .unwrap_err()
        .code,
        ErrorCode::Exists
    );
    // The refused send counted too: the allowance is spent.
    let send = |_: &Path, _: u64, _: &str| -> Result<Value, ToolError> { Ok(Value::Null) };
    assert_eq!(
        send_to_chat(
            &f.reader,
            &limiter,
            &send,
            "builds:app.AppImage",
            &f.snapshots
        )
        .unwrap_err()
        .code,
        ErrorCode::RateLimited
    );
}
