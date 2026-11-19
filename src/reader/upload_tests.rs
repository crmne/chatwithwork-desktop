//! Files checked for an upload (`Reader::upload_file`): only a regular file
//! inside a shared folder, by where it really is, and bound to what it holds.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::config::Root;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    reader: Reader,
}

fn fixture(follow_symlinks: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let shared = base.join("shared");
    fs::create_dir_all(shared.join("shots")).unwrap();
    fs::create_dir_all(base.join("private")).unwrap();
    fs::write(
        shared.join("shots/listing.png"),
        b"\x89PNG\r\n\x1a\nlisting",
    )
    .unwrap();
    fs::write(shared.join("bundle.aab"), vec![7u8; 4096]).unwrap();
    fs::write(shared.join(".env"), "TOKEN=x").unwrap();
    fs::write(base.join("private/secret.txt"), "not shared").unwrap();
    let mut config = Config {
        roots: vec![Root {
            bots: Vec::new(),
            id: "work".into(),
            label: "Work".into(),
            path: shared,
            follow_symlinks,
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
    let paths = Paths::under(&base.join("cww"));
    let reader = Reader::new(&config, &paths, Changes::default()).unwrap();
    Fixture {
        _tmp: tmp,
        base,
        reader,
    }
}

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn vouches_for_a_shared_file_by_its_real_path() {
    let f = fixture(false);
    let file = f
        .reader
        .upload_file("work:shots/listing.png", 1 << 20)
        .unwrap();
    assert_eq!(file.path, "work:shots/listing.png");
    assert_eq!(file.folder, "Work");
    assert_eq!(file.name, "listing.png");
    assert_eq!(file.size, 15);
    assert_eq!(file.mime_type, "image/png");
    assert_eq!(file.sha256, sha256(b"\x89PNG\r\n\x1a\nlisting"));
    // As the browser takes it: on Windows, without `\\?\`.
    assert_eq!(
        file.real,
        safe_fs::without_verbatim_prefix(&f.base.join("shared/shots/listing.png"))
    );
    // Where it really is never leaves the computer.
    let sent = serde_json::to_string(&file).unwrap();
    assert!(!sent.contains(f.base.to_str().unwrap()));
}

#[test]
fn refuses_paths_that_climb_out_or_name_no_shared_folder() {
    let f = fixture(false);
    for (path, code) in [
        ("work:../private/secret.txt", ErrorCode::InvalidPath),
        (
            "work:shots/../../private/secret.txt",
            ErrorCode::InvalidPath,
        ),
        ("/etc/passwd", ErrorCode::InvalidPath),
        ("~/private/secret.txt", ErrorCode::InvalidPath),
        ("private:secret.txt", ErrorCode::UnknownRoot),
        ("work:", ErrorCode::NotAFile),
        ("work:shots", ErrorCode::NotAFile),
        ("work:missing.png", ErrorCode::NotFound),
    ] {
        let error = f.reader.upload_file(path, 1 << 20).unwrap_err();
        assert_eq!(error.code, code, "{path}");
        assert!(
            !error.message.contains("secret"),
            "{path}: {}",
            error.message
        );
    }
}

#[test]
fn refuses_what_the_deny_list_hides() {
    let f = fixture(false);
    assert_eq!(
        f.reader.upload_file("work:.env", 1 << 20).unwrap_err().code,
        ErrorCode::Denied
    );
}

#[cfg(unix)]
#[test]
fn never_follows_a_symlink_out_of_the_folder() {
    for follow in [false, true] {
        let f = fixture(follow);
        std::os::unix::fs::symlink(
            f.base.join("private/secret.txt"),
            f.base.join("shared/innocent.png"),
        )
        .unwrap();
        std::os::unix::fs::symlink(f.base.join("private"), f.base.join("shared/linked")).unwrap();
        for path in ["work:innocent.png", "work:linked/secret.txt"] {
            let error = f.reader.upload_file(path, 1 << 20).unwrap_err();
            assert!(
                matches!(error.code, ErrorCode::Denied | ErrorCode::OutsideRoot),
                "follow={follow} {path}: {:?}",
                error.code
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_the_folder_follows_inside_it_hands_over_its_target() {
    let f = fixture(true);
    std::os::unix::fs::symlink("shots/listing.png", f.base.join("shared/latest.png")).unwrap();
    let file = f.reader.upload_file("work:latest.png", 1 << 20).unwrap();
    assert_eq!(
        file.real,
        safe_fs::without_verbatim_prefix(&f.base.join("shared/shots/listing.png"))
    );
    assert_eq!(file.name, "latest.png");
    // A folder that doesn't follow symlinks refuses even that one.
    let f = fixture(false);
    std::os::unix::fs::symlink("shots/listing.png", f.base.join("shared/latest.png")).unwrap();
    assert_eq!(
        f.reader
            .upload_file("work:latest.png", 1 << 20)
            .unwrap_err()
            .code,
        ErrorCode::Denied
    );
}

#[cfg(unix)]
#[test]
fn a_copy_keeps_the_name_the_call_gave_and_the_bytes_checked() {
    let f = fixture(true);
    std::os::unix::fs::symlink("shots/listing.png", f.base.join("shared/latest.png")).unwrap();
    let file = f
        .reader
        .upload_snapshot("work:latest.png", 1 << 20, &f.base.join("snapshots"))
        .unwrap();
    assert_eq!(file.real.file_name().unwrap(), "latest.png");
    assert!(file.real.starts_with(f.base.join("snapshots")));
    assert_eq!(fs::read(&file.real).unwrap(), b"\x89PNG\r\n\x1a\nlisting");
    assert_eq!(file.sha256, sha256(b"\x89PNG\r\n\x1a\nlisting"));
    let plain = f.reader.upload_file("work:latest.png", 1 << 20).unwrap();
    assert_eq!(
        UploadFile::fingerprint(&[file]),
        UploadFile::fingerprint(&[plain]),
        "the copy is the file approved"
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_hard_link_to_a_file_elsewhere() {
    let f = fixture(false);
    fs::hard_link(
        f.base.join("private/secret.txt"),
        f.base.join("shared/copy.txt"),
    )
    .unwrap();
    assert_eq!(
        f.reader
            .upload_file("work:copy.txt", 1 << 20)
            .unwrap_err()
            .code,
        ErrorCode::Denied
    );
}

#[test]
fn refuses_a_file_over_the_limit() {
    let f = fixture(false);
    let error = f.reader.upload_file("work:bundle.aab", 4095).unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    assert!(f.reader.upload_file("work:bundle.aab", 4096).is_ok());
}

#[test]
fn the_fingerprint_changes_when_a_file_does() {
    let f = fixture(false);
    let check = |f: &Fixture| {
        let files = vec![
            f.reader
                .upload_file("work:shots/listing.png", 1 << 20)
                .unwrap(),
            f.reader.upload_file("work:bundle.aab", 1 << 20).unwrap(),
        ];
        UploadFile::fingerprint(&files)
    };
    let approved = check(&f);
    assert_eq!(check(&f), approved);
    // Same size and, within a second, the same time: only the content differs.
    fs::write(f.base.join("shared/bundle.aab"), vec![8u8; 4096]).unwrap();
    assert_ne!(check(&f), approved);
    // Another file put in its place.
    let f = fixture(false);
    let approved = check(&f);
    let path = f.base.join("shared/shots/listing.png");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"\x89PNG\r\n\x1a\nlisting").unwrap();
    let again = f
        .reader
        .upload_file("work:shots/listing.png", 1 << 20)
        .unwrap();
    assert_eq!(again.sha256, sha256(b"\x89PNG\r\n\x1a\nlisting"));
    // The order is part of it: the page gets the files in that order.
    let files = vec![
        f.reader.upload_file("work:bundle.aab", 1 << 20).unwrap(),
        f.reader
            .upload_file("work:shots/listing.png", 1 << 20)
            .unwrap(),
    ];
    assert_ne!(UploadFile::fingerprint(&files), approved);
}

#[test]
fn guesses_types_the_way_pages_check_them() {
    assert_eq!(upload_mime_type("Shot.PNG"), "image/png");
    assert_eq!(
        upload_mime_type("app-release.aab"),
        "application/octet-stream"
    );
    assert_eq!(upload_mime_type("extension.zip"), "application/zip");
    assert_eq!(upload_mime_type("README"), "application/octet-stream");
    assert_eq!(upload_mime_type("scan.BMP"), "image/bmp");
    assert_eq!(upload_mime_type("voice.ogg"), "audio/ogg");
    assert_eq!(upload_mime_type("clip.mkv"), "video/x-matroska");
}

#[test]
fn reports_a_path_without_the_verbatim_prefix() {
    assert_eq!(
        safe_fs::without_verbatim_prefix(Path::new(r"\\?\C:\Users\me\shot.png")),
        PathBuf::from(r"C:\Users\me\shot.png")
    );
    assert_eq!(
        safe_fs::without_verbatim_prefix(Path::new(r"\\?\UNC\nas\builds\app.aab")),
        PathBuf::from(r"\\nas\builds\app.aab")
    );
    assert_eq!(
        safe_fs::without_verbatim_prefix(Path::new("/home/me/shot.png")),
        PathBuf::from("/home/me/shot.png")
    );
}
