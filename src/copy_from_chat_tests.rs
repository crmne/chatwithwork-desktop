//! `copy_from_chat`: a chat's file saved as a new file in a folder that
//! allows changes, kept only when exactly the approved bytes arrived.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::config::{Config, Root};
use crate::paths::Paths;
use crate::status::Changes;
use crate::trash::Trash;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    writer: Writer,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("builds")).unwrap();
    fs::create_dir_all(base.join("docs")).unwrap();
    fs::write(base.join("builds/old.aab"), "old").unwrap();
    let root = |id: &str, writable: bool| Root {
        bots: Vec::new(),
        id: id.into(),
        label: id.to_uppercase(),
        path: base.join(id),
        follow_symlinks: false,
        writable,
        commands: false,
        network: false,
        local_services: true,
        reach: Vec::new(),
        read_budget: None,
    };
    let mut config = Config {
        roots: vec![root("builds", true), root("docs", false)],
        ..Config::default()
    };
    config.index.enabled = false;
    let paths = Paths::under(&base.join("cww"));
    let reader = Arc::new(Reader::new(&config, &paths, Changes::default()).unwrap());
    let limiter = Arc::new(Limiter::new(config.limits.clone()));
    let writer = Writer::new(reader, limiter, Trash::new(Some(base.join("trash"))));
    Fixture {
        _tmp: tmp,
        base,
        writer,
    }
}

fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn request(path: &str, bytes: &[u8], dry_run: bool) -> CopyFromChatRequest {
    CopyFromChatRequest {
        path: path.into(),
        size: bytes.len() as u64,
        sha256: sha(bytes),
        transfer: (!dry_run).then(|| "eyJfcmFpbHMiOnsiZGF0YSI6WzFdfX0--abcdef0123".into()),
        dry_run,
    }
}

type Sink<'a> = &'a mut dyn FnMut(&[u8]) -> Result<(), ToolError>;

/// A server that sends `bytes` in pieces, and counts its calls.
fn serving(
    bytes: Vec<u8>,
    calls: &std::cell::Cell<u32>,
) -> impl Fn(&str, Sink<'_>) -> Result<(), ToolError> + '_ {
    move |_transfer, sink| {
        calls.set(calls.get() + 1);
        for piece in bytes.chunks(7000) {
            sink(piece)?;
        }
        Ok(())
    }
}

#[test]
fn saves_the_approved_bytes_as_a_new_file() {
    let f = fixture();
    let bytes: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let calls = std::cell::Cell::new(0);
    let fetch = serving(bytes.clone(), &calls);

    let checked = copy_from_chat(
        &f.writer,
        &fetch,
        &request("builds:app-release.aab", &bytes, true),
    )
    .unwrap();
    assert!(checked.dry_run);
    assert_eq!(calls.get(), 0, "a dry run fetches nothing");
    assert!(!f.base.join("builds/app-release.aab").exists());

    let saved = copy_from_chat(
        &f.writer,
        &fetch,
        &request("builds:app-release.aab", &bytes, false),
    )
    .unwrap();
    assert_eq!(saved.written, bytes.len() as u64);
    assert_eq!(saved.sha256.as_deref(), Some(sha(&bytes).as_str()));
    assert_eq!(
        fs::read(f.base.join("builds/app-release.aab")).unwrap(),
        bytes
    );
    // Nothing temporary is left beside it.
    let names: Vec<_> = fs::read_dir(f.base.join("builds"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn never_replaces_anything_and_keeps_to_folders_that_allow_changes() {
    let f = fixture();
    let calls = std::cell::Cell::new(0);
    let fetch = serving(b"new".to_vec(), &calls);
    for (path, code) in [
        ("builds:old.aab", ErrorCode::Exists),
        ("docs:new.aab", ErrorCode::NotWritable),
        ("builds:../docs/x.aab", ErrorCode::InvalidPath),
        ("builds:setup.exe", ErrorCode::NotChangeable),
        ("builds:run.desktop", ErrorCode::NotChangeable),
    ] {
        let error = copy_from_chat(&f.writer, &fetch, &request(path, b"new", false)).unwrap_err();
        assert_eq!(error.code, code, "{path}");
    }
    assert_eq!(calls.get(), 0, "nothing was fetched for a refused file");
    assert_eq!(
        fs::read_to_string(f.base.join("builds/old.aab")).unwrap(),
        "old"
    );
}

#[test]
fn keeps_nothing_unless_exactly_the_approved_file_arrived() {
    let f = fixture();
    let approved = b"the approved bundle".to_vec();
    let calls = std::cell::Cell::new(0);
    for sent in [
        b"another bundle!!!!!".to_vec(),
        b"the approved".to_vec(),
        b"the approved bundle plus".to_vec(),
    ] {
        let fetch = serving(sent, &calls);
        let error = copy_from_chat(
            &f.writer,
            &fetch,
            &request("builds:app.aab", &approved, false),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(!f.base.join("builds/app.aab").exists());
    }
    let names: Vec<_> = fs::read_dir(f.base.join("builds"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "no partial file is left: {names:?}");
    // A transfer the server refuses leaves nothing either.
    let refusing =
        |_: &str, _: &mut dyn FnMut(&[u8]) -> Result<(), ToolError>| -> Result<(), ToolError> {
            Err(ToolError::new(ErrorCode::NotFound, "gone"))
        };
    assert_eq!(
        copy_from_chat(
            &f.writer,
            &refusing,
            &request("builds:app.aab", &approved, false)
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    assert!(!f.base.join("builds/app.aab").exists());
}

#[test]
fn refuses_what_is_too_big_malformed_or_ungranted() {
    let f = fixture();
    let calls = std::cell::Cell::new(0);
    let fetch = serving(Vec::new(), &calls);
    let mut big = request("builds:huge.bin", b"x", true);
    big.size = crate::chat_files::MAX_BYTES + 1;
    assert_eq!(
        copy_from_chat(&f.writer, &fetch, &big).unwrap_err().code,
        ErrorCode::TooLarge
    );
    let mut bad = request("builds:a.bin", b"x", true);
    bad.sha256 = "not-hex".into();
    assert_eq!(
        copy_from_chat(&f.writer, &fetch, &bad).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let mut ungranted = request("builds:a.bin", b"x", false);
    ungranted.transfer = None;
    assert_eq!(
        copy_from_chat(&f.writer, &fetch, &ungranted)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(calls.get(), 0);
}

#[test]
fn is_offered_with_the_other_changes() {
    assert!(change_tools().iter().any(|t| {
        t.name == "copy_from_chat"
            && t.annotations
                .as_ref()
                .is_some_and(|a| a.destructive_hint == Some(false))
    }));
}
