//! Local-only speech preferences and verified, explicitly downloaded models.
//! No audio, server token, or account information enters the downloader.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use ring::digest::{Context as Digest, SHA256};
use serde::{Deserialize, Serialize};

use crate::paths::{Paths, ensure_private_dir, write_private_file};

/// Pinned upstream model revision. LFS SHA-256 and lengths are checked before
/// publishing a download and again before inference, including after restarts.
const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Cloud,
    Local,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    Tiny,
    #[default]
    Base,
    Small,
}

impl Model {
    pub const ALL: [Self; 3] = [Self::Tiny, Self::Base, Self::Small];

    pub fn id(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiny => "Tiny · 78 MB · fastest",
            Self::Base => "Base · 148 MB · balanced",
            Self::Small => "Small · 488 MB · more accurate",
        }
    }

    pub fn bytes(self) -> u64 {
        match self {
            Self::Tiny => 77_691_713,
            Self::Base => 147_951_465,
            Self::Small => 487_601_967,
        }
    }

    fn sha256(self) -> &'static str {
        match self {
            Self::Tiny => "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
            Self::Base => "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            Self::Small => "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }
    }

    pub fn path(self, paths: &Paths) -> PathBuf {
        paths
            .data_dir
            .join("speech-models")
            .join(format!("ggml-{}.bin", self.id()))
    }

    /// A quick UI check only. The inference process always verifies the hash.
    pub fn downloaded(self, paths: &Paths) -> bool {
        fs::symlink_metadata(self.path(paths))
            .is_ok_and(|meta| meta.is_file() && meta.len() == self.bytes())
    }

    pub fn verify(self, paths: &Paths) -> Result<PathBuf> {
        let path = self.path(paths);
        ensure!(
            self.downloaded(paths),
            "Download this speech model in General settings first."
        );
        check_reader(File::open(&path)?, self.bytes(), self.sha256())?;
        Ok(path)
    }

    pub fn remove(self, paths: &Paths) -> Result<()> {
        match fs::remove_file(self.path(paths)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Cancellation is checked between bounded reads; network stalls time out.
    /// The incomplete file is never offered for inference and is always removed.
    pub fn download(
        self,
        paths: &Paths,
        cancelled: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<()> {
        let origin = crate::auth::client::ServerUrl::parse("https://huggingface.co")?;
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
        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/ggml-{}.bin",
            self.id()
        );
        // Small validated ranges give cancellation a finite bound without a
        // second HTTP stack or a worker stuck reading a large stalled body.
        let source = Ranges {
            agent: ureq::Agent::new_with_config(config),
            url,
            offset: 0,
            total: self.bytes(),
            buffer: std::io::Cursor::new(Vec::new()),
            cancelled,
        };
        install(
            self.path(paths).as_path(),
            source,
            self.bytes(),
            self.sha256(),
            cancelled,
            progress,
        )
    }
}

struct Ranges<'a> {
    agent: ureq::Agent,
    url: String,
    offset: u64,
    total: u64,
    buffer: std::io::Cursor<Vec<u8>>,
    cancelled: &'a AtomicBool,
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
                    Error::other(
                        "Could not download the speech model. Check your connection and try again.",
                    )
                })?;
            let expected = format!("bytes {}-{end}/{}", self.offset, self.total);
            if response.status().as_u16() != 206
                || response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    != Some(expected.as_str())
            {
                return Err(Error::other(
                    "The model server returned an incomplete download. Try again.",
                ));
            }
            let size = end - self.offset + 1;
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(size + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 != size {
                return Err(Error::other(
                    "The speech model download was interrupted. Try again.",
                ));
            }
            self.offset = end + 1;
            self.buffer = std::io::Cursor::new(bytes);
        }
        self.buffer.read(out)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub mode: Mode,
    pub model: Model,
}

impl Preferences {
    pub fn load(paths: &Paths) -> Result<Self> {
        match fs::read(paths.config_dir.join("dictation.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Could not read dictation settings. Choose a transcription mode in General settings."),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            // Never silently choose cloud after a local preference becomes unreadable.
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(self, paths: &Paths) -> Result<()> {
        write_private_file(
            &paths.config_dir.join("dictation.json"),
            &serde_json::to_vec(&self)?,
        )
    }

    pub fn help(self) -> &'static str {
        match self.mode {
            Mode::Cloud => {
                "Dictate using credits. Audio is transcribed in the EU through your Chat with Work server. Review the words before sending."
            }
            Mode::Local => {
                "Dictate on this computer with Whisper. Audio stays here and uses no credits. Review the words before sending."
            }
        }
    }
}

fn check_reader(mut reader: impl Read, bytes: u64, sha256: &str) -> Result<()> {
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
            "Speech model has the wrong size. Download it again."
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
        "Speech model failed verification. Download it again."
    );
    Ok(())
}

fn install(
    path: &Path,
    mut source: impl Read,
    bytes: u64,
    sha256: &str,
    cancelled: &AtomicBool,
    progress: impl Fn(u64),
) -> Result<()> {
    let directory = path.parent().context("model directory missing")?;
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
        ensure!(
            total <= bytes,
            "Speech model has the wrong size. Try again."
        );
        file.write_all(&buffer[..count])?;
        progress(total);
    }
    file.sync_all()?;
    drop(file);
    check_reader(File::open(&partial)?, bytes, sha256)?;
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
            assert!(install(&path, data, 3, digest, &AtomicBool::new(cancel), |_| {}).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"previous");
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
        install(
            &path,
            b"abc".as_slice(),
            3,
            digest,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }

    #[test]
    fn invalid_preferences_never_fall_back_to_cloud() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        let prefs = Preferences {
            mode: Mode::Local,
            model: Model::Tiny,
        };
        prefs.save(&paths).unwrap();
        assert_eq!(Preferences::load(&paths).unwrap(), prefs);
        fs::write(paths.config_dir.join("dictation.json"), b"broken").unwrap();
        assert!(Preferences::load(&paths).is_err());
        assert!(!Model::Tiny.downloaded(&paths));
    }
}
