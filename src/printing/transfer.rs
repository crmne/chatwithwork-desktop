//! A file on its way from Chat with Work to the printer, a chunk at a time,
//! since a message carries at most 256 KiB. It's kept in a private folder of
//! cww's own, under an id the server chose, until it's printed or an hour
//! has passed. It can't name anything outside that folder.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::Serialize;

use crate::error::{ErrorCode, ToolError};
use crate::paths::ensure_private_dir;

/// As large as an upload to Chat with Work may be.
pub const MAX_BYTES: u64 = 25 * 1024 * 1024;
/// A chunk, in bytes, before base64.
pub const MAX_CHUNK: usize = 64 * 1024;
/// Files on their way at once.
const MAX_OPEN: usize = 4;
/// Files nobody finished or printed, and receipts of jobs a day old.
const FILE_LIFE: Duration = Duration::from_secs(60 * 60);
const RECEIPT_LIFE: Duration = Duration::from_secs(24 * 60 * 60);

/// What a chunk adds to a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Received {
    pub transfer: String,
    pub received: u64,
    pub complete: bool,
}

/// One chunk of a file.
pub struct Chunk<'a> {
    pub transfer: &'a str,
    pub offset: u64,
    pub size: u64,
    pub sha256: &'a str,
    pub data: &'a str,
}

/// Ids are the server's: 16 to 64 lowercase hex digits, so one can only name
/// a file in this folder.
pub fn valid_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::invalid_argument(message)
}

fn part(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.part"))
}

fn whole(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.pdf"))
}

/// The file `id` once all of it arrived and checked out.
pub fn finished(dir: &Path, id: &str) -> Option<PathBuf> {
    let path = whole(dir, id);
    (path.is_file() && !older_than(&path, FILE_LIFE)).then_some(path)
}

/// Add a chunk. A chunk sent again (the answer was lost) starts over from
/// its offset. The last chunk's file must have the SHA-256 it was announced
/// with, and be a PDF.
pub fn receive(dir: &Path, chunk: &Chunk<'_>) -> Result<Received, ToolError> {
    // One at a time: a slot among `MAX_OPEN`, and a file being written, are
    // each checked and then used.
    static RECEIVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = RECEIVING.lock().unwrap_or_else(|held| held.into_inner());
    if !valid_id(chunk.transfer) {
        return Err(invalid("`transfer` is 16 to 64 lowercase hex digits"));
    }
    if chunk.size == 0 || chunk.size > MAX_BYTES {
        return Err(ToolError::new(
            ErrorCode::TooLarge,
            format!("a file to print is at most {} MiB", MAX_BYTES >> 20),
        ));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(chunk.data)
        .map_err(|_| invalid("`data` isn't base64"))?;
    if data.len() > MAX_CHUNK || data.is_empty() {
        return Err(invalid(format!("a chunk is 1 to {MAX_CHUNK} bytes")));
    }
    let end = chunk.offset.saturating_add(data.len() as u64);
    if end > chunk.size {
        return Err(invalid("that chunk runs past `size`"));
    }
    ensure_private_dir(dir).map_err(|_| unavailable())?;
    purge(dir);
    let path = part(dir, chunk.transfer);
    if finished(dir, chunk.transfer).is_some() {
        return Ok(Received {
            transfer: chunk.transfer.to_string(),
            received: chunk.size,
            complete: true,
        });
    }
    if !path.exists() && open_transfers(dir) >= MAX_OPEN {
        return Err(ToolError::new(
            ErrorCode::RateLimited,
            "too many files on their way to the printer",
        ));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|_| unavailable())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    let have = file.metadata().map_err(|_| unavailable())?.len();
    if chunk.offset > have {
        return Err(invalid(format!(
            "this chunk starts at {} but {have} bytes have arrived",
            chunk.offset
        )));
    }
    file.set_len(chunk.offset).map_err(|_| unavailable())?;
    file.seek(SeekFrom::Start(chunk.offset))
        .map_err(|_| unavailable())?;
    file.write_all(&data).map_err(|_| unavailable())?;
    if end < chunk.size {
        return Ok(Received {
            transfer: chunk.transfer.to_string(),
            received: end,
            complete: false,
        });
    }
    file.flush().map_err(|_| unavailable())?;
    file.seek(SeekFrom::Start(0)).map_err(|_| unavailable())?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut head = [0u8; 5];
    let mut buffer = vec![0u8; 64 * 1024];
    let mut seen = 0usize;
    loop {
        let read = file.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        if seen < head.len() {
            let take = read.min(head.len() - seen);
            head[seen..seen + take].copy_from_slice(&buffer[..take]);
            seen += take;
        }
        hash.update(&buffer[..read]);
    }
    drop(file);
    let digest: String = hash
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != chunk.sha256 {
        let _ = std::fs::remove_file(&path);
        return Err(invalid("the file that arrived isn't the one announced"));
    }
    if &head != b"%PDF-" {
        let _ = std::fs::remove_file(&path);
        return Err(ToolError::new(ErrorCode::Unsupported, "only a PDF prints"));
    }
    std::fs::rename(&path, whole(dir, chunk.transfer)).map_err(|_| unavailable())?;
    Ok(Received {
        transfer: chunk.transfer.to_string(),
        received: chunk.size,
        complete: true,
    })
}

fn unavailable() -> ToolError {
    ToolError::new(
        ErrorCode::Internal,
        "couldn't keep the file to print on this computer",
    )
}

fn open_transfers(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.ends_with(".part") || name.ends_with(".pdf")
                })
                .count()
        })
        .unwrap_or(0)
}

/// Drop what nobody finished or printed.
pub fn purge(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let life = if name.ends_with(".part") || name.ends_with(".pdf") {
            FILE_LIFE
        } else if name.ends_with(".receipt") {
            RECEIPT_LIFE
        } else {
            continue;
        };
        if older_than(&entry.path(), life) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn older_than(path: &Path, life: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > life)
}
