//! Reading pages with Windows' own OCR, `Windows.Media.Ocr`, built into
//! Windows 10 and 11: no download, a box for each word, in the languages
//! whose OCR packs Windows has (each display language brings its own;
//! Settings, Time & language, Language adds more).
//!
//! The page goes in as a grey SoftwareBitmap, as scanned: not the despeckled
//! copy Tesseract and Vision get. Windows reads a letter printed on grey
//! halftone well by itself, and the median filter took the dots off its
//! umlauts ("fur", "fallig" on half the tries; every umlaut without it). Each
//! line's words come back with their boxes in the page's pixels from the top
//! left, as the PDF writer takes them.

use image::DynamicImage;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

use crate::ocr::{Engine, Language as OcrLanguage, Word};

pub struct WindowsOcr;

/// WinRT on this thread; already initialised is fine.
fn winrt() {
    // SAFETY: initialising the Windows Runtime for the calling thread.
    let _ = unsafe {
        windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        )
    };
}

impl Engine for WindowsOcr {
    fn languages(&self) -> Result<Vec<OcrLanguage>, String> {
        winrt();
        let available =
            OcrEngine::AvailableRecognizerLanguages().map_err(|e| e.message().to_string())?;
        let mut languages: Vec<OcrLanguage> = available
            .into_iter()
            .filter_map(|l| {
                Some(OcrLanguage {
                    code: l.LanguageTag().ok()?.to_string(),
                    name: l.DisplayName().ok()?.to_string(),
                })
            })
            .collect();
        languages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(languages)
    }

    fn recognize(
        &self,
        page: &DynamicImage,
        _resolution: u32,
        language: &str,
    ) -> Result<Vec<Word>, String> {
        if !crate::ocr::plain_code(language) {
            return Err("That isn't a language Windows reads.".into());
        }
        winrt();
        let failed =
            |e: windows::core::Error| format!("Windows couldn't read the page: {}", e.message());
        let tag = Language::CreateLanguage(&HSTRING::from(language)).map_err(failed)?;
        let engine = OcrEngine::TryCreateFromLanguage(&tag).map_err(|_| {
            format!("Windows can't read {language}: add its language (with OCR) in Settings, Time & language, Language.")
        })?;
        let mut gray = page.to_luma8();
        // Windows reads pages up to MaxImageDimension pixels on a side.
        let max = OcrEngine::MaxImageDimension().unwrap_or(10_000);
        let scale = f64::from(max) / f64::from(gray.width().max(gray.height()));
        if scale < 1.0 {
            gray = image::imageops::resize(
                &gray,
                (f64::from(gray.width()) * scale) as u32,
                (f64::from(gray.height()) * scale) as u32,
                image::imageops::FilterType::Triangle,
            );
        }
        let back = if scale < 1.0 { 1.0 / scale } else { 1.0 };
        let (w, h) = gray.dimensions();
        let writer = DataWriter::new().map_err(failed)?;
        writer.WriteBytes(gray.as_raw()).map_err(failed)?;
        let buffer = writer.DetachBuffer().map_err(failed)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Gray8,
            w as i32,
            h as i32,
        )
        .map_err(failed)?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(failed)?
            .join()
            .map_err(failed)?;
        let mut words = Vec::new();
        for line in result.Lines().map_err(failed)? {
            for word in line.Words().map_err(failed)? {
                let (Ok(text), Ok(rect)) = (word.Text(), word.BoundingRect()) else {
                    continue;
                };
                if rect.Width < 1.0 || rect.Height < 1.0 {
                    continue;
                }
                words.push(Word {
                    text: text.to_string(),
                    x: (f64::from(rect.X) * back) as u32,
                    y: (f64::from(rect.Y) * back) as u32,
                    width: (f64::from(rect.Width) * back) as u32,
                    height: (f64::from(rect.Height) * back) as u32,
                });
            }
        }
        Ok(words)
    }
}
