//! Reading pages with Apple's Vision framework, built into macOS: no
//! download, every language it knows, and a box for each word.
//!
//! A page goes in as PNG data (the despeckled grey copy, as Tesseract gets),
//! `VNRecognizeTextRequest` reads it in accurate mode in the chosen language,
//! and each line's words get their boxes from `boundingBoxForRange`. Vision's
//! boxes are shares of the page from its bottom left; they're turned into
//! pixels from the top left, as the PDF writer takes them.

use image::DynamicImage;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
    VNRequestTextRecognitionLevel,
};

use crate::ocr::{Engine, Language, Word, despeckle};

pub struct Vision;

fn request() -> Retained<VNRecognizeTextRequest> {
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    request
}

impl Engine for Vision {
    fn languages(&self) -> Result<Vec<Language>, String> {
        autoreleasepool(|_| {
            // SAFETY: the request is only asked what it supports.
            let codes = unsafe { request().supportedRecognitionLanguagesAndReturnError() }
                .map_err(|e| e.localizedDescription().to_string())?;
            let mut languages: Vec<Language> = codes
                .iter()
                .map(|code| {
                    let code = code.to_string();
                    Language {
                        name: language_name(&code),
                        code,
                    }
                })
                .collect();
            languages.sort_by(|a, b| a.name.cmp(&b.name));
            languages.dedup_by(|a, b| a.name == b.name);
            Ok(languages)
        })
    }

    fn recognize(
        &self,
        page: &DynamicImage,
        _resolution: u32,
        language: &str,
    ) -> Result<Vec<Word>, String> {
        if !crate::ocr::plain_code(language) {
            return Err("That isn't a language this Mac reads.".into());
        }
        let gray = despeckle(&page.to_luma8());
        let (width, height) = (gray.width() as f64, gray.height() as f64);
        let mut png = Vec::new();
        gray.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        autoreleasepool(|_| {
            let data = NSData::with_bytes(&png);
            let request = request();
            // SAFETY: the handler reads the PNG data it is given; the request
            // and its results live in this autorelease pool.
            unsafe {
                request.setRecognitionLanguages(&NSArray::from_retained_slice(&[
                    NSString::from_str(language),
                ]));
                let handler = VNImageRequestHandler::initWithData_options(
                    VNImageRequestHandler::alloc(),
                    &data,
                    &NSDictionary::new(),
                );
                let requests: Retained<NSArray<VNRequest>> =
                    NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(
                        request.clone(),
                    ))]);
                handler.performRequests_error(&requests).map_err(|e| {
                    format!(
                        "This Mac couldn't read the page: {}",
                        e.localizedDescription()
                    )
                })?;
                let mut words = Vec::new();
                for observation in request.results().unwrap_or_default().iter() {
                    words.extend(line_words(&observation, width, height));
                }
                Ok(words)
            }
        })
    }
}

/// The words of one line, each with its box in pixels from the top left.
///
/// SAFETY: `observation` is a result of a finished request.
unsafe fn line_words(
    observation: &VNRecognizedTextObservation,
    width: f64,
    height: f64,
) -> Vec<Word> {
    let Some(candidate) = observation.topCandidates(1).firstObject() else {
        return vec![];
    };
    let text = candidate.string().to_string();
    let mut words = Vec::new();
    // Ranges in UTF-16 units, as NSString counts them.
    let mut offset = 0usize;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let start = offset;
        offset += piece.encode_utf16().count();
        if word.is_empty() {
            continue;
        }
        let range = NSRange::new(start, word.encode_utf16().count());
        let Ok(rectangle) = (unsafe { candidate.boundingBoxForRange_error(range) }) else {
            continue;
        };
        let bounds = unsafe { rectangle.boundingBox() };
        let x = (bounds.origin.x * width).max(0.0);
        let w = bounds.size.width * width;
        let h = bounds.size.height * height;
        let y = ((1.0 - bounds.origin.y - bounds.size.height) * height).max(0.0);
        if w >= 1.0 && h >= 1.0 {
            words.push(Word {
                text: word.to_string(),
                x: x as u32,
                y: y as u32,
                width: w as u32,
                height: h as u32,
            });
        }
    }
    words
}

/// Vision's codes ("de-DE") in English, for the picker.
fn language_name(code: &str) -> String {
    let name = match code.split('-').next().unwrap_or(code) {
        "ar" => "Arabic",
        "ars" => "Arabic (Najdi)",
        "cs" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "en" => "English",
        "es" => "Spanish",
        "fi" => "Finnish",
        "fr" => "French",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "mr" => "Marathi",
        "ms" => "Malay",
        "nb" | "no" => "Norwegian",
        "nl" => "Dutch",
        "nn" => "Norwegian (Nynorsk)",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sv" => "Swedish",
        "th" => "Thai",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "yue" if code.ends_with("Hant") => "Cantonese (Traditional)",
        "yue" => "Cantonese (Simplified)",
        "zh" if code.ends_with("Hant") => "Chinese (Traditional)",
        "zh" => "Chinese (Simplified)",
        _ => return code.to_string(),
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_vision_languages() {
        assert_eq!(language_name("de-DE"), "German");
        assert_eq!(language_name("zh-Hant"), "Chinese (Traditional)");
        assert_eq!(language_name("xx-YY"), "xx-YY");
    }

    #[test]
    fn lists_languages_with_german() {
        let languages = Vision.languages().unwrap();
        assert!(
            languages
                .iter()
                .any(|l| l.code == "de-DE" && l.name == "German"),
            "{languages:?}"
        );
    }
}
