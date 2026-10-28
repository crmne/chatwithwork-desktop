//! Local-only speech preferences and verified Whisper models, downloaded the
//! first time they're needed. No audio, server token, or account
//! information enters the downloader.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::paths::{Paths, write_private_file};

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

    /// The model this computer runs comfortably: dictation is transcribed
    /// after the recording stops, so a slower model is a longer wait.
    pub fn for_this_computer() -> Self {
        Self::recommended(
            memory_bytes(),
            std::thread::available_parallelism().map_or(1, usize::from),
        )
    }

    /// Small wants about 1 GB of memory while it runs and a few cores to
    /// finish a minute of speech in seconds; Tiny suits small machines.
    pub fn recommended(memory: Option<u64>, cores: usize) -> Self {
        const GB: u64 = 1024 * 1024 * 1024;
        match memory {
            Some(memory) if memory >= 16 * GB && cores >= 8 => Self::Small,
            Some(memory) if memory < 8 * GB => Self::Tiny,
            _ if cores >= 4 => Self::Base,
            _ => Self::Tiny,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tiny => "Tiny",
            Self::Base => "Base",
            Self::Small => "Small",
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
            "This speech model isn't downloaded yet. Dictate once to get it."
        );
        crate::fetch::check(
            std::fs::File::open(&path)?,
            self.bytes(),
            self.sha256(),
            "speech model",
        )?;
        Ok(path)
    }

    pub fn remove(self, paths: &Paths) -> Result<()> {
        match fs::remove_file(self.path(paths)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// The model, downloaded first if it isn't here yet.
    pub fn ensure(
        self,
        paths: &Paths,
        cancelled: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<()> {
        if self.downloaded(paths) {
            return Ok(());
        }
        self.download(paths, cancelled, progress)
    }

    /// Cancellation is checked between bounded reads; network stalls time out.
    /// The incomplete file is never offered for inference and is always removed.
    pub fn download(
        self,
        paths: &Paths,
        cancelled: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<()> {
        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/ggml-{}.bin",
            self.id()
        );
        crate::fetch::verified(
            paths,
            &crate::fetch::Pinned {
                url: &url,
                bytes: self.bytes(),
                sha256: self.sha256(),
                what: "speech model",
            },
            &self.path(paths),
            cancelled,
            progress,
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub mode: Mode,
    /// The model someone picked; none means this computer's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
}

impl Preferences {
    /// The model local dictation uses.
    pub fn model(self) -> Model {
        self.model.unwrap_or_else(Model::for_this_computer)
    }

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

/// This computer's memory, where the system says.
fn memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
        meminfo_total(&meminfo)
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: a zeroed MEMORYSTATUSEX with its length set is what
        // GlobalMemoryStatusEx fills in; it keeps no pointer to it.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn meminfo_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_preferences_never_fall_back_to_cloud() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        let prefs = Preferences {
            mode: Mode::Local,
            model: Some(Model::Tiny),
        };
        prefs.save(&paths).unwrap();
        assert_eq!(Preferences::load(&paths).unwrap(), prefs);
        fs::write(paths.config_dir.join("dictation.json"), b"broken").unwrap();
        assert!(Preferences::load(&paths).is_err());
        assert!(!Model::Tiny.downloaded(&paths));
    }

    /// Downloads Tiny (78 MB) from Hugging Face: `cargo test -- --ignored`.
    #[test]
    #[ignore = "downloads 78 MB"]
    fn ensure_downloads_and_verifies_a_missing_model_once() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        paths.ensure().unwrap();
        let seen = std::sync::atomic::AtomicU64::new(0);
        Model::Tiny
            .ensure(&paths, &AtomicBool::new(false), |bytes| {
                seen.store(bytes, std::sync::atomic::Ordering::Relaxed)
            })
            .unwrap();
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::Relaxed),
            Model::Tiny.bytes()
        );
        Model::Tiny.verify(&paths).unwrap();
        Model::Tiny
            .ensure(&paths, &AtomicBool::new(false), |_| {
                panic!("downloaded twice")
            })
            .unwrap();
    }

    #[test]
    fn without_a_choice_the_model_suits_the_computer() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert_eq!(Model::recommended(Some(32 * GB), 16), Model::Small);
        assert_eq!(Model::recommended(Some(16 * GB), 4), Model::Base);
        assert_eq!(Model::recommended(Some(8 * GB), 8), Model::Base);
        assert_eq!(Model::recommended(Some(4 * GB), 8), Model::Tiny);
        assert_eq!(Model::recommended(None, 8), Model::Base);
        assert_eq!(Model::recommended(None, 2), Model::Tiny);
        assert_eq!(
            meminfo_total("MemTotal:       65536000 kB\nMemFree: 1 kB\n"),
            Some(65_536_000 * 1024)
        );
        // Older settings named a model; a choice is kept, none follows the computer.
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        paths.ensure().unwrap();
        fs::write(
            paths.config_dir.join("dictation.json"),
            br#"{"mode":"local","model":"small"}"#,
        )
        .unwrap();
        assert_eq!(Preferences::load(&paths).unwrap().model(), Model::Small);
        fs::write(
            paths.config_dir.join("dictation.json"),
            br#"{"mode":"local"}"#,
        )
        .unwrap();
        assert_eq!(
            Preferences::load(&paths).unwrap().model(),
            Model::for_this_computer()
        );
        Preferences {
            mode: Mode::Local,
            model: None,
        }
        .save(&paths)
        .unwrap();
        assert_eq!(
            fs::read(paths.config_dir.join("dictation.json")).unwrap(),
            br#"{"mode":"local"}"#
        );
    }
}
