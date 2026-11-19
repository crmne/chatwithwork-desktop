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
        base,
        reader,
    }
}

#[test]
fn sends_the_file_by_where_it_really_is_and_counts_it() {
    let f = fixture();
    let limiter = Limiter::new(Limits::default());
    let sent = std::cell::RefCell::new(None);
    let send = |path: &Path, size: u64, sha: &str| -> Result<Value, ToolError> {
        *sent.borrow_mut() = Some((path.to_path_buf(), size, sha.to_string()));
        Ok(json!({ "path": "builds/app.AppImage", "size": size }))
    };
    let answer = send_to_chat(&f.reader, &limiter, &send, "builds:app.AppImage").unwrap();
    let (path, size, sha) = sent.into_inner().unwrap();
    assert_eq!(path, f.base.join("builds/app.AppImage"));
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
            send_to_chat(&f.reader, &limiter, &send, path).is_err(),
            "{path}"
        );
    }
    #[cfg(unix)]
    assert_eq!(
        send_to_chat(&f.reader, &limiter, &send, "builds:link.txt")
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
    assert!(send_to_chat(&f.reader, &limiter, &send, "builds:app.AppImage").is_ok());
    let error = send_to_chat(&f.reader, &limiter, &send, "builds:app.AppImage").unwrap_err();
    assert_eq!(error.code, ErrorCode::RateLimited);
}

#[test]
fn a_file_that_changed_while_it_went_is_reported_and_a_refusal_passes_through() {
    let f = fixture();
    let limiter = Limiter::new(Limits::default());
    let changing = |_: &Path, _: u64, _: &str| -> Result<Value, ToolError> {
        fs::write(f.base.join("builds/app.AppImage"), vec![1u8; 30_000]).unwrap();
        Ok(Value::Null)
    };
    assert_eq!(
        send_to_chat(&f.reader, &limiter, &changing, "builds:app.AppImage")
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let refusing = |_: &Path, _: u64, _: &str| -> Result<Value, ToolError> {
        Err(ToolError::new(ErrorCode::Exists, "already there"))
    };
    assert_eq!(
        send_to_chat(&f.reader, &limiter, &refusing, "builds:app.AppImage")
            .unwrap_err()
            .code,
        ErrorCode::Exists
    );
}
