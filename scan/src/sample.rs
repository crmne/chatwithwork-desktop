//! A scanner made of a folder of page images, for development, tests and
//! pictures: with `CWW_SCAN_SAMPLE_DIR=<folder>` the app offers one scanner,
//! "Sample scanner", whose feeder holds that folder's PNG and JPEG files in
//! name order, at `CWW_SCAN_SAMPLE_DPI` (300 unless set). It replaces the
//! system's backend, so it never shows up for anyone who didn't ask.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Backend, Capabilities, Device, End, Request, ScanError, Scanned};

pub const ENV: &str = "CWW_SCAN_SAMPLE_DIR";
const ID: &str = "sample:folder";

pub struct Sample {
    dir: PathBuf,
    resolution: u32,
}

impl Sample {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os(ENV)?);
        let resolution = std::env::var("CWW_SCAN_SAMPLE_DPI")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Some(Self { dir, resolution })
    }

    fn files(&self) -> Result<Vec<PathBuf>, ScanError> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map_err(|e| {
                ScanError::new("not_found", format!("The sample folder can't be read: {e}"))
            })?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
                })
            })
            .collect();
        files.sort();
        Ok(files)
    }
}

impl Backend for Sample {
    fn devices(&mut self) -> Result<Vec<Device>, ScanError> {
        Ok(vec![Device {
            id: ID.into(),
            name: "Sample scanner".into(),
            kind: "sheetfed scanner".into(),
        }])
    }

    fn capabilities(&mut self, _device: &str) -> Result<Capabilities, ScanError> {
        Ok(Capabilities {
            resolutions: vec![self.resolution],
            color: true,
            feeder: true,
            duplex: true,
            flatbed: false,
        })
    }

    fn scan(
        &mut self,
        request: &Request,
        cancel: &Arc<AtomicBool>,
        page: &mut dyn FnMut(Scanned) -> Result<(), ScanError>,
    ) -> Result<End, ScanError> {
        if request.device != ID {
            return Err(ScanError::new(
                "not_found",
                "That scanner isn't connected any more.",
            ));
        }
        let files = self.files()?;
        if files.is_empty() {
            return Ok(End::NoPaper);
        }
        // One-sided scans take the fronts: every other image.
        for (n, file) in files.iter().enumerate() {
            if !request.duplex && n % 2 == 1 {
                continue;
            }
            if cancel.load(Ordering::SeqCst) {
                return Ok(End::Cancelled);
            }
            // As long as a page takes a real scanner, roughly.
            std::thread::sleep(std::time::Duration::from_millis(400));
            let image = image::open(file).map_err(|e| {
                ScanError::new("scanner_error", format!("A sample page can't be read: {e}"))
            })?;
            let image = if request.color {
                image::DynamicImage::ImageRgb8(image.to_rgb8())
            } else {
                image::DynamicImage::ImageLuma8(image.to_luma8())
            };
            page(Scanned {
                image,
                resolution: self.resolution,
            })?;
        }
        Ok(End::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_the_folder_in_order_fronts_only_when_one_sided() {
        let dir = tempfile::tempdir().unwrap();
        for (name, shade) in [
            ("b.png", 20u8),
            ("a.png", 10),
            ("c.jpg", 30),
            ("notes.txt", 0),
        ] {
            let path = dir.path().join(name);
            if name.ends_with(".txt") {
                std::fs::write(path, "not a page").unwrap();
            } else {
                image::GrayImage::from_pixel(4, 4, image::Luma([shade]))
                    .save(path)
                    .unwrap();
            }
        }
        let mut sample = Sample {
            dir: dir.path().into(),
            resolution: 150,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut request = Request {
            device: ID.into(),
            color: false,
            resolution: 150,
            duplex: true,
        };
        let mut shades = vec![];
        sample
            .scan(&request, &cancel, &mut |p| {
                shades.push(p.image.to_luma8().get_pixel(0, 0)[0]);
                Ok(())
            })
            .unwrap();
        assert_eq!(shades.len(), 3);
        assert_eq!(&shades[..2], &[10, 20]);
        request.duplex = false;
        let mut count = 0;
        sample
            .scan(&request, &cancel, &mut |_| {
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 2);
    }
}
