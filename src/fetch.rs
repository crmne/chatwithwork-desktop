//! Verified downloads of the files the app fetches for features on this
//! computer (Whisper models, OCR languages): over HTTPS, through the
//! configured proxy, in small validated ranges so cancelling is prompt, into
//! a private temporary file that only replaces the destination once its
//! length and SHA-256 match the ones pinned in the code.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use ring::digest::{Context as Digest, SHA256};

use crate::paths::{Paths, ensure_private_dir};

/// A file pinned in the code.
pub struct Pinned<'a> {
    pub url: &'a str,
    pub bytes: u64,
    pub sha256: &'a str,
    /// What it is, in words, for errors ("speech model").
    pub what: &'a str,
}

/// Downloads `file` to `destination`. Cancellation is checked between
/// bounded reads; network stalls time out. An incomplete or wrong file is
/// never kept, and an earlier file at `destination` stays until a good one
/// replaces it.
pub fn verified(
    paths: &Paths,
    file: &Pinned<'_>,
    destination: &Path,
    cancelled: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<()> {
    let parsed = url::Url::parse(file.url)?;
    ensure!(parsed.scheme() == "https", "Downloads are HTTPS only.");
    let origin = crate::auth::client::ServerUrl::parse(&parsed.origin().ascii_serialization())?;
    let config = crate::config::Config::load(paths)?;
    let proxy = crate::proxy::for_server(config.proxy.as_deref(), &origin)?
        .map(|p| p.to_ureq())
        .transpose()?;
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .proxy(proxy)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_body(Some(Duration::from_secs(10)))
        .tls_config(crate::tls::ureq_config())
        .build();
    // Small validated ranges give cancellation a finite bound without a
    // second HTTP stack or a worker stuck reading a large stalled body.
    let source = Ranges {
        agent: ureq::Agent::new_with_config(config),
        url: file.url.to_owned(),
        offset: 0,
        total: file.bytes,
        buffer: std::io::Cursor::new(Vec::new()),
        cancelled,
        what: file.what,
    };
    install(
        destination,
        source,
        file.bytes,
        file.sha256,
        file.what,
        cancelled,
        progress,
    )
}

struct Ranges<'a> {
    agent: ureq::Agent,
    url: String,
    offset: u64,
    total: u64,
    buffer: std::io::Cursor<Vec<u8>>,
    cancelled: &'a AtomicBool,
    what: &'a str,
}

impl Read for Ranges<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Error, ErrorKind};
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(Error::new(ErrorKind::Interrupted, "Download cancelled."));
        }
        if self.buffer.position() == self.buffer.get_ref().len() as u64 && self.offset < self.total
        {
            let end = (self.offset + 1024 * 1024).min(self.total) - 1;
            // Distinct URLs avoid intermediaries reusing a different range.
            let mut response = self
                .agent
                .get(format!(
                    "{}?download=true&cww-range={}",
                    self.url, self.offset
                ))
                .header("Range", format!("bytes={}-{}", self.offset, end))
                .call()
                .map_err(|_| {
                    Error::other(format!(
                        "Could not download the {}. Check your connection and try again.",
                        self.what
                    ))
                })?;
            let expected = format!("bytes {}-{end}/{}", self.offset, self.total);
            if response.status().as_u16() != 206
                || response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    != Some(expected.as_str())
            {
                return Err(Error::other(format!(
                    "The server returned an incomplete download of the {}. Try again.",
                    self.what
                )));
            }
            let size = end - self.offset + 1;
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(size + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 != size {
                return Err(Error::other(format!(
                    "The {} download was interrupted. Try again.",
                    self.what
                )));
            }
            self.offset = end + 1;
            self.buffer = std::io::Cursor::new(bytes);
        }
        self.buffer.read(out)
    }
}

/// Checks a file's length and SHA-256.
pub fn check(mut reader: impl Read, bytes: u64, sha256: &str, what: &str) -> Result<()> {
    let mut hash = Digest::new(&SHA256);
    let mut buffer = [0; 64 * 1024];
    let mut total = 0;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= bytes,
            "The {what} has the wrong size. Download it again."
        );
        hash.update(&buffer[..count]);
    }
    let actual: String = hash
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ensure!(
        total == bytes && actual == sha256,
        "The {what} failed verification. Download it again."
    );
    Ok(())
}

fn install(
    path: &Path,
    mut source: impl Read,
    bytes: u64,
    sha256: &str,
    what: &str,
    cancelled: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<()> {
    let directory = path.parent().context("download folder missing")?;
    ensure_private_dir(directory)?;
    // Unique per operation, including multiple desktop/TUI processes.
    let random = ring::rand::SystemRandom::new();
    let suffix = ring::rand::generate::<[u8; 16]>(&random)
        .map_err(|_| anyhow::anyhow!("Could not prepare the download."))?
        .expose();
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let partial = directory.join(format!(".download-{suffix}.part"));
    struct Partial(PathBuf);
    impl Drop for Partial {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Partial(partial.clone());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&partial)?;
    let mut total = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            bail!("Download cancelled.");
        }
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= bytes, "The {what} has the wrong size. Try again.");
        file.write_all(&buffer[..count])?;
        progress(total);
    }
    file.sync_all()?;
    drop(file);
    check(File::open(&partial)?, bytes, sha256, what)?;
    ensure!(!cancelled.load(Ordering::SeqCst), "Download cancelled.");
    fs::rename(&partial, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_corrupt_and_cancelled_downloads_never_replace_a_model() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("model.bin");
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for (data, cancel) in [
            (b"ab".as_slice(), false),
            (b"abcd", false),
            (b"bad", false),
            (b"abc", true),
        ] {
            fs::write(&path, b"previous").unwrap();
            assert!(
                install(
                    &path,
                    data,
                    3,
                    digest,
                    "file",
                    &AtomicBool::new(cancel),
                    |_| {}
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), b"previous");
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
        install(
            &path,
            b"abc".as_slice(),
            3,
            digest,
            "file",
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }
}
