//! Reading pages on the computer, for the PDF's text layer.
//!
//! Each system's own engine where it has one: Vision on macOS and
//! `Windows.Media.Ocr` on Windows, both built in; on Linux, Tesseract from
//! the distribution (the `tesseract` command and its language packages).
//! An engine returns words with their boxes; [`crate::pdf`] places them.
//! Nothing is sent anywhere: Chat with Work's servers do no OCR.
//!
//! The language is the person's choice. Guessing it would mean reading each
//! page several times, or asking a model, which costs credits.

use std::io::Write;
use std::process::{Command, Stdio};

use image::{DynamicImage, GrayImage};
use serde::{Deserialize, Serialize};

/// A language an engine reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Language {
    /// The engine's code, passed back to read with it ("deu", "de-DE").
    pub code: String,
    /// In English, for the picker.
    pub name: String,
}

/// A word, in pixels of the page it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What reads text on this computer.
pub trait Engine: Send + Sync {
    /// The languages it reads now, by name.
    fn languages(&self) -> Result<Vec<Language>, String>;

    /// The words on `page`, scanned at `resolution`.
    fn recognize(
        &self,
        page: &DynamicImage,
        resolution: u32,
        language: &str,
    ) -> Result<Vec<Word>, String>;
}

/// This system's engine, or why there is none (with what to install).
pub fn system_engine() -> Result<Box<dyn Engine>, String> {
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Tesseract::find().map(|t| Box::new(t) as Box<dyn Engine>)
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::vision::Vision))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(crate::windows_ocr::WindowsOcr))
    }
}

/// Tesseract, through the `tesseract` command the distribution installs.
pub struct Tesseract {
    program: std::path::PathBuf,
}

/// What to say when Tesseract is missing.
pub const TESSERACT_MISSING: &str = "Making scans searchable needs Tesseract. Install your distribution's tesseract package with the languages you read (for example tesseract-data-deu on Arch, tesseract-ocr-deu on Debian and Ubuntu, tesseract-langpack-deu on Fedora).";

impl Tesseract {
    pub fn find() -> Result<Self, String> {
        let program = std::env::var_os("CWW_TESSERACT")
            .map(std::path::PathBuf::from)
            .or_else(|| find_on_path("tesseract"))
            .ok_or_else(|| TESSERACT_MISSING.to_string())?;
        Ok(Self { program })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        // One thread per page: the app reads several pages at once.
        command.env("OMP_THREAD_LIMIT", "1");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// A language code is letters, digits and `_` only, as Tesseract's are, so
/// it can't reach the command as anything but a language.
pub fn plain_code(code: &str) -> bool {
    !code.is_empty()
        && !code.starts_with('-')
        && code.len() <= 32
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Engine for Tesseract {
    fn languages(&self) -> Result<Vec<Language>, String> {
        let output = self
            .command()
            .arg("--list-langs")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("Couldn't run Tesseract: {e}"))?;
        let mut languages: Vec<Language> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip_while(|line| !line.starts_with("List of available languages"))
            .skip(1)
            .map(str::trim)
            .filter(|code| plain_code(code) && !matches!(*code, "osd" | "equ" | "snum"))
            .map(|code| Language {
                code: code.to_string(),
                name: tesseract_name(code),
            })
            .collect();
        languages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(languages)
    }

    fn recognize(
        &self,
        page: &DynamicImage,
        resolution: u32,
        language: &str,
    ) -> Result<Vec<Word>, String> {
        if !plain_code(language) {
            return Err("That isn't a language Tesseract knows.".into());
        }
        let mut png = Vec::new();
        despeckle(&page.to_luma8())
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let mut child = self
            .command()
            // TSV through its parameter rather than the `tsv` config file, which
            // only some tessdata folders carry.
            .args([
                "stdin",
                "stdout",
                "-l",
                language,
                "--dpi",
                &resolution.to_string(),
                "-c",
                "tessedit_create_tsv=1",
            ])
            .spawn()
            .map_err(|e| format!("Couldn't run Tesseract: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped");
        let writer = std::thread::spawn(move || stdin.write_all(&png));
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        let _ = writer.join();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(if stderr.contains("Failed loading language") {
                format!(
                    "Tesseract can't read {}: install its language package.",
                    tesseract_name(language)
                )
            } else {
                "Tesseract couldn't read the page.".into()
            });
        }
        let tsv = String::from_utf8_lossy(&output.stdout);
        if !tsv.starts_with("level\t") {
            return Err("Tesseract didn't say where the words are.".into());
        }
        Ok(parse_tsv(&tsv))
    }
}

/// The copy of a page Tesseract reads, never the one in the PDF: a 3 by 3
/// median, which takes out isolated dots (the halftone of a printed gray
/// paper tone, toner specks, scanner noise) and keeps strokes, which are
/// wider. Without it Tesseract took a letter printed on gray for a picture
/// and read two words of it; with it, every word.
pub fn despeckle(gray: &GrayImage) -> GrayImage {
    let (w, h) = gray.dimensions();
    if w < 3 || h < 3 {
        return gray.clone();
    }
    let src = gray.as_raw();
    let width = w as usize;
    let mut out = src.clone();
    for y in 1..h as usize - 1 {
        for x in 1..width - 1 {
            let mut window = [0u8; 9];
            for (i, dy) in [y - 1, y, y + 1].into_iter().enumerate() {
                window[i * 3..i * 3 + 3]
                    .copy_from_slice(&src[dy * width + x - 1..dy * width + x + 2]);
            }
            window.sort_unstable();
            out[y * width + x] = window[4];
        }
    }
    GrayImage::from_raw(w, h, out).expect("same size")
}

/// Words from Tesseract's TSV, each given its line's top and height so the
/// words of a line select as one even row.
pub fn parse_tsv(tsv: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut line_box = (0u32, 0u32);
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.splitn(12, '\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let num = |i: usize| cols[i].parse::<i64>().unwrap_or(-1);
        let (level, left, top, width, height) = (num(0), num(6), num(7), num(8), num(9));
        if left < 0 || top < 0 || width <= 0 || height <= 0 {
            continue;
        }
        match level {
            4 => line_box = (top as u32, height as u32),
            5 => {
                let text = cols[11].trim();
                if text.is_empty() {
                    continue;
                }
                let (line_top, line_height) = line_box;
                let inside = line_height > 0
                    && (top as u32) >= line_top
                    && (top + height) as u32 <= line_top + line_height;
                let (y, h) = if inside {
                    line_box
                } else {
                    (top as u32, height as u32)
                };
                words.push(Word {
                    text: text.to_string(),
                    x: left as u32,
                    y,
                    width: width as u32,
                    height: h,
                });
            }
            _ => {}
        }
    }
    words
}

/// Tesseract's language codes in English. Codes not listed show as they are.
pub fn tesseract_name(code: &str) -> String {
    let name = match code {
        "afr" => "Afrikaans",
        "ara" => "Arabic",
        "bul" => "Bulgarian",
        "cat" => "Catalan",
        "ces" => "Czech",
        "chi_sim" => "Chinese (Simplified)",
        "chi_tra" => "Chinese (Traditional)",
        "dan" => "Danish",
        "deu" => "German",
        "deu_frak" | "frk" => "German (Fraktur)",
        "ell" => "Greek",
        "eng" => "English",
        "est" => "Estonian",
        "fin" => "Finnish",
        "fra" => "French",
        "gle" => "Irish",
        "heb" => "Hebrew",
        "hin" => "Hindi",
        "hrv" => "Croatian",
        "hun" => "Hungarian",
        "ind" => "Indonesian",
        "isl" => "Icelandic",
        "ita" => "Italian",
        "jpn" => "Japanese",
        "kor" => "Korean",
        "lat" => "Latin",
        "lav" => "Latvian",
        "lit" => "Lithuanian",
        "mlt" => "Maltese",
        "nld" => "Dutch",
        "nor" => "Norwegian",
        "pol" => "Polish",
        "por" => "Portuguese",
        "ron" => "Romanian",
        "rus" => "Russian",
        "slk" => "Slovak",
        "slv" => "Slovenian",
        "spa" => "Spanish",
        "srp" => "Serbian",
        "swe" => "Swedish",
        "tha" => "Thai",
        "tur" => "Turkish",
        "ukr" => "Ukrainian",
        "vie" => "Vietnamese",
        other => return other.to_string(),
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t2480\t3508\t-1\t
4\t1\t1\t1\t1\t0\t300\t400\t900\t50\t-1\t
5\t1\t1\t1\t1\t1\t300\t405\t200\t40\t95.1\tSehr
5\t1\t1\t1\t1\t2\t520\t402\t380\t46\t93.0\tgeehrte
5\t1\t1\t1\t1\t3\t920\t402\t30\t46\t10.0\t
4\t1\t1\t1\t2\t0\t300\t500\t400\t50\t-1\t
5\t1\t1\t1\t2\t1\t300\t500\t400\t50\t91.0\tDamen\tund
";

    #[test]
    fn reads_words_with_their_lines_boxes() {
        let words = parse_tsv(TSV);
        assert_eq!(words.len(), 3);
        assert_eq!(
            words[0],
            Word {
                text: "Sehr".into(),
                x: 300,
                y: 400,
                width: 200,
                height: 50
            }
        );
        assert_eq!(words[1].text, "geehrte");
        assert_eq!((words[1].y, words[1].height), (400, 50));
        // A tab inside the text column stays text.
        assert_eq!(words[2].text, "Damen\tund");
    }

    #[test]
    fn despeckling_takes_out_dots_and_keeps_strokes() {
        // Gray paper printed as a halftone: a dark dot every third pixel, and
        // a stroke three pixels wide.
        let page = GrayImage::from_fn(30, 30, |x, y| {
            let stroke = (10..13).contains(&x);
            let dot = x % 3 == 0 && y % 3 == 0;
            image::Luma([if stroke || dot { 20 } else { 230 }])
        });
        let clean = despeckle(&page);
        assert_eq!(clean.get_pixel(21, 21)[0], 230, "a dot is gone");
        assert_eq!(clean.get_pixel(11, 15)[0], 20, "the stroke stays");
    }

    #[test]
    fn language_codes_are_plain() {
        assert!(plain_code("deu") && plain_code("chi_sim"));
        assert!(
            !plain_code("")
                && !plain_code("deu;rm")
                && !plain_code("../x")
                && !plain_code("--psm")
                && !plain_code("deu+eng x")
        );
        assert_eq!(tesseract_name("deu"), "German");
        assert_eq!(tesseract_name("xyz"), "xyz");
    }
}
