//! Scanning for Chat with Work Desktop.
//!
//! A scanner is driven through the system's own scanning API, behind one
//! [`Backend`] trait: SANE on Linux, ImageCaptureCore on macOS, WIA 2.0 on
//! Windows. Backends only answer "which scanners", "what can this one do"
//! and "scan with these settings", handing back pages as 8-bit grey or
//! colour images; everything after that is the same on every system:
//!
//! - [`blank`] tells blank pages (the backs of one-sided letters) apart,
//! - [`edit`] crops and turns pages as the person asks,
//! - [`ocr`] reads each page on the computer (Tesseract on Linux, Vision on
//!   macOS, `Windows.Media.Ocr` on Windows) into words with their boxes,
//! - [`pdf`] writes the pages and an invisible, selectable text layer into
//!   one PDF.
//!
//! Drivers are vendor code that hangs and crashes, so the app runs the
//! backend in a helper process of its own and talks to it with
//! [`helper`]'s JSON lines. Nothing here touches the network: a scan stays on
//! the computer until the person sends it.

pub mod blank;
pub mod edit;
pub mod helper;
pub mod languages;
pub mod ocr;
pub mod pdf;
pub mod sample;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod sane;
#[cfg(windows)]
pub mod twain;
#[cfg(windows)]
pub mod twain_locale;
#[cfg(target_os = "macos")]
pub mod vision;
#[cfg(windows)]
pub mod windows_ocr;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

/// How a page is scanned and kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    /// Scanned in grey and made pure black and white on the computer, so
    /// the same threshold applies whatever the scanner's own "lineart" does.
    BlackAndWhite,
    #[default]
    Grayscale,
    Color,
}

impl ColorMode {
    /// What the scanner is asked for: grey for black and white too.
    pub fn scanned_in_color(self) -> bool {
        self == ColorMode::Color
    }
}

/// What the person chose on the scan page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub color: ColorMode,
    /// Dots per inch.
    pub resolution: u32,
    /// Both sides of each sheet, where the scanner can.
    pub duplex: bool,
    pub remove_blank_pages: bool,
    /// The language to read pages in (an [`ocr::Language`] code), or none
    /// for no text layer.
    pub ocr: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            color: ColorMode::Grayscale,
            resolution: 300,
            duplex: true,
            remove_blank_pages: true,
            ocr: Some("eng".into()),
        }
    }
}

/// A scanner the system knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// The backend's own name for it, passed back to open it.
    pub id: String,
    /// "Canon P-208II", for people.
    pub name: String,
    /// "sheetfed scanner", "flatbed scanner", or empty.
    #[serde(default)]
    pub kind: String,
}

/// What a scanner can do, for the settings the page offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    /// Resolutions it takes, in dpi, smallest first.
    pub resolutions: Vec<u32>,
    pub color: bool,
    /// It has a document feeder (several pages per scan).
    pub feeder: bool,
    /// The feeder scans both sides.
    pub duplex: bool,
    pub flatbed: bool,
}

impl Capabilities {
    /// The resolution closest to `wanted` that the scanner takes.
    pub fn nearest_resolution(&self, wanted: u32) -> u32 {
        self.resolutions
            .iter()
            .copied()
            .min_by_key(|r| r.abs_diff(wanted))
            .unwrap_or(wanted)
    }
}

/// What a backend is asked to scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub device: String,
    pub color: bool,
    pub resolution: u32,
    pub duplex: bool,
}

impl Request {
    pub fn new(device: &str, settings: &Settings) -> Self {
        Self {
            device: device.to_string(),
            color: settings.color.scanned_in_color(),
            resolution: settings.resolution,
            duplex: settings.duplex,
        }
    }
}

/// A scanned page, as the scanner gave it.
pub struct Scanned {
    pub image: image::DynamicImage,
    /// The resolution it was scanned at, which sets its size on paper.
    pub resolution: u32,
}

/// How a scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The feeder ran out, or the flatbed's one page is done.
    Done,
    /// The feeder was empty before the first page.
    NoPaper,
    Cancelled,
}

/// A failure a person can act on, in their words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    pub code: &'static str,
    pub message: String,
}

impl ScanError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScanError {}

/// One system's scanning API.
pub trait Backend {
    /// The scanners attached now.
    fn devices(&mut self) -> Result<Vec<Device>, ScanError>;

    fn capabilities(&mut self, device: &str) -> Result<Capabilities, ScanError>;

    /// Scans until the feeder is empty (or one page from a flatbed), handing
    /// each page to `page` as it arrives. `cancel` stops it between reads.
    fn scan(
        &mut self,
        request: &Request,
        cancel: &Arc<AtomicBool>,
        page: &mut dyn FnMut(Scanned) -> Result<(), ScanError>,
    ) -> Result<End, ScanError>;
}

/// This system's backend, or why there is none.
pub fn system_backend() -> Result<Box<dyn Backend>, ScanError> {
    if let Some(sample) = sample::Sample::from_env() {
        return Ok(Box::new(sample));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        Ok(Box::new(sane::Sane::load()?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(twain::Twain::load()?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err(ScanError::new(
            "unsupported",
            "Scanning isn't available on this system yet.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_is_scanned_in_grey() {
        assert!(!ColorMode::BlackAndWhite.scanned_in_color());
        assert!(!ColorMode::Grayscale.scanned_in_color());
        assert!(ColorMode::Color.scanned_in_color());
    }

    #[test]
    fn picks_the_nearest_resolution() {
        let caps = Capabilities {
            resolutions: vec![150, 200, 300, 600],
            ..Default::default()
        };
        assert_eq!(caps.nearest_resolution(300), 300);
        assert_eq!(caps.nearest_resolution(250), 200);
        assert_eq!(caps.nearest_resolution(1200), 600);
        assert_eq!(Capabilities::default().nearest_resolution(300), 300);
    }
}
