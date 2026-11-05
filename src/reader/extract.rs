//! Turn files into text: plain text, PDF, DOCX, PPTX and spreadsheets.
//!
//! Parsers consume attacker-controlled input, so every extractor works on a
//! size-capped buffer, stops at a character cap, and runs under
//! `catch_unwind`. In phase 3 this module moves into the sandboxed reader
//! process unchanged.

use std::io::{Cursor, Read};
use std::time::Instant;

use crate::error::{ErrorCode, ToolError};

/// How a file is turned into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Pdf,
    Docx,
    Pptx,
    Spreadsheet,
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "md",
    "markdown",
    "mdx",
    "rst",
    "org",
    "adoc",
    "tex",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "tsx",
    "rb",
    "erb",
    "py",
    "rs",
    "go",
    "java",
    "kt",
    "kts",
    "swift",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "php",
    "pl",
    "lua",
    "sh",
    "bash",
    "zsh",
    "fish",
    "sql",
    "log",
    "ini",
    "cfg",
    "conf",
    "properties",
    "gradle",
    "vue",
    "svelte",
    "ex",
    "exs",
    "erl",
    "hs",
    "ml",
    "scala",
    "clj",
    "r",
    "jl",
    "dart",
    "zig",
    "nim",
    "vim",
    "el",
    "graphql",
    "proto",
    "tf",
    "hcl",
    "rtf",
    "srt",
    "vtt",
    "bib",
    "ipynb",
];

/// Extensions known to be binary, skipped without opening the file.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tiff", "ico", "psd", "mp3", "m4a", "wav",
    "flac", "ogg", "mp4", "mov", "mkv", "avi", "webm", "zip", "gz", "tgz", "bz2", "xz", "zst",
    "7z", "rar", "dmg", "iso", "exe", "dll", "so", "dylib", "o", "a", "class", "jar", "wasm",
    "bin", "sqlite", "db", "ttf", "otf", "woff", "woff2", "doc", "ppt", "key", "pages", "numbers",
];

pub fn format_for(name: &str) -> Option<Format> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("pdf") => Some(Format::Pdf),
        Some("docx" | "docm" | "dotx") => Some(Format::Docx),
        Some("pptx" | "pptm") => Some(Format::Pptx),
        Some("xlsx" | "xlsm" | "xlsb" | "xls" | "ods") => Some(Format::Spreadsheet),
        Some(e) if TEXT_EXTENSIONS.contains(&e) => Some(Format::Text),
        Some(e) if BINARY_EXTENSIONS.contains(&e) => None,
        // No or unknown extension: sniff the content.
        _ => Some(Format::Text),
    }
}

/// Read at most `max_bytes` from `reader`, failing if there is more.
pub fn read_capped(reader: &mut impl Read, max_bytes: u64) -> Result<Vec<u8>, ToolError> {
    let mut buf = Vec::new();
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| ToolError::internal(format!("reading file: {e}")))?;
    if buf.len() as u64 > max_bytes {
        return Err(ToolError::new(
            ErrorCode::TooLarge,
            format!("file is larger than {max_bytes} bytes"),
        ));
    }
    Ok(buf)
}

/// Extract up to `max_chars` characters of text from `bytes`.
pub fn extract(name: &str, bytes: &[u8], max_chars: usize) -> Result<String, ToolError> {
    extract_until(name, bytes, max_chars, None)
}

/// As [`extract`], stopping with an error between a document's pages, parts
/// or rows once `deadline` passes, so a call with a time budget isn't held
/// by one large document. Never a partial text: what's cached is whole.
pub fn extract_until(
    name: &str,
    bytes: &[u8],
    max_chars: usize,
    deadline: Option<Instant>,
) -> Result<String, ToolError> {
    let Some(format) = format_for(name) else {
        return Err(unsupported());
    };
    let result = std::panic::catch_unwind(|| match format {
        Format::Text => text(bytes, max_chars),
        Format::Pdf => pdf(bytes, max_chars, deadline),
        Format::Docx => office_xml(bytes, OfficeKind::Docx, max_chars, deadline),
        Format::Pptx => office_xml(bytes, OfficeKind::Pptx, max_chars, deadline),
        Format::Spreadsheet => spreadsheet(bytes, max_chars, deadline),
    });
    match result {
        Ok(r) => r,
        Err(_) => Err(ToolError::new(
            ErrorCode::Unsupported,
            "the file could not be parsed",
        )),
    }
}

/// Stop if `deadline` has passed.
fn in_time(deadline: Option<Instant>) -> Result<(), ToolError> {
    match deadline {
        Some(deadline) if Instant::now() >= deadline => Err(ToolError::new(
            ErrorCode::Internal,
            "the document took too long to turn into text",
        )),
        _ => Ok(()),
    }
}

fn unsupported() -> ToolError {
    ToolError::new(
        ErrorCode::Unsupported,
        "this file type can't be read as text",
    )
}

fn text(bytes: &[u8], max_chars: usize) -> Result<String, ToolError> {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return Err(unsupported());
    }
    let s = String::from_utf8_lossy(bytes);
    Ok(truncate_chars(&s, max_chars).to_string())
}

fn pdf(bytes: &[u8], max_chars: usize, deadline: Option<Instant>) -> Result<String, ToolError> {
    let doc = lopdf::Document::load_mem(bytes).map_err(|e| {
        ToolError::new(
            ErrorCode::Unsupported,
            format!("the PDF could not be parsed: {e}"),
        )
    })?;
    if doc.is_encrypted() {
        return Err(ToolError::new(
            ErrorCode::Unsupported,
            "the PDF is encrypted",
        ));
    }
    let mut out = String::new();
    for page in doc.get_pages().keys() {
        in_time(deadline)?;
        if let Ok(text) = doc.extract_text_with_limit(&[*page], 16 * 1024 * 1024) {
            out.push_str(text.trim_end());
            out.push_str("\n\n");
        }
        if out.chars().count() >= max_chars {
            break;
        }
    }
    Ok(truncate_chars(&out, max_chars).trim_end().to_string())
}

#[derive(Clone, Copy)]
enum OfficeKind {
    Docx,
    Pptx,
}

/// Largest decompressed XML part accepted, against zip bombs.
const MAX_XML_PART: u64 = 64 * 1024 * 1024;

fn office_xml(
    bytes: &[u8],
    kind: OfficeKind,
    max_chars: usize,
    deadline: Option<Instant>,
) -> Result<String, ToolError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ToolError::new(ErrorCode::Unsupported, "the document could not be opened"))?;
    let parts: Vec<String> = match kind {
        OfficeKind::Docx => vec!["word/document.xml".to_string()],
        OfficeKind::Pptx => {
            let mut slides: Vec<(u32, String)> = archive
                .file_names()
                .collect::<zip::result::ZipResult<Vec<_>>>()
                .map_err(|_| ToolError::new(ErrorCode::Unsupported, "the document is damaged"))?
                .into_iter()
                .filter_map(|n| {
                    let num = n
                        .strip_prefix("ppt/slides/slide")?
                        .strip_suffix(".xml")?
                        .parse()
                        .ok()?;
                    Some((num, n.to_string()))
                })
                .collect();
            slides.sort();
            slides.into_iter().map(|(_, n)| n).collect()
        }
    };
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        in_time(deadline)?;
        let Ok(entry) = archive.by_name(part) else {
            continue;
        };
        let mut xml = Vec::new();
        entry
            .take(MAX_XML_PART)
            .read_to_end(&mut xml)
            .map_err(|_| ToolError::new(ErrorCode::Unsupported, "the document is damaged"))?;
        if let OfficeKind::Pptx = kind {
            out.push_str(&format!("--- Slide {} ---\n", i + 1));
        }
        xml_text(&xml, kind, &mut out, max_chars);
        out.push('\n');
        if out.len() >= max_chars * 4 && out.chars().count() >= max_chars {
            break;
        }
    }
    Ok(truncate_chars(out.trim_end(), max_chars).to_string())
}

fn xml_text(xml: &[u8], kind: OfficeKind, out: &mut String, max_chars: usize) {
    use quick_xml::events::Event;

    let (text_tag, para_tag) = match kind {
        OfficeKind::Docx => ("w:t", "w:p"),
        OfficeKind::Pptx => ("a:t", "a:p"),
    };
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut in_text = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == text_tag => in_text = true,
            Ok(Event::End(e)) if e.name().as_ref() == text_tag => in_text = false,
            Ok(Event::End(e)) if e.name().as_ref() == para_tag => out.push('\n'),
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:tab" => out.push('\t'),
                "w:br" | "w:cr" | "a:br" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => out.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else {
                    match &*r {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        if out.len() > max_chars * 4 {
            break;
        }
        buf.clear();
    }
}

/// Most cells of a workbook read as they come, blank ones included.
const MAX_CELLS: usize = 20_000_000;

/// An `.xlsx`'s cells one at a time, its rows' cells apart by tabs, from the
/// first column. Never through `worksheet_range`, which sets aside room for
/// every cell between a sheet's first and last: two cells far apart, in a
/// file of a few bytes, would ask for terabytes, which ends the process.
fn xlsx_text<RS: Read + std::io::Seek>(
    workbook: &mut calamine::Xlsx<RS>,
    max_chars: usize,
    deadline: Option<Instant>,
) -> Result<String, ToolError> {
    use calamine::{Data, Reader};

    let mut out = String::new();
    let mut cells_read = 0usize;
    'sheets: for name in workbook.sheet_names() {
        let Ok(mut cells) = workbook.worksheet_cells_reader(&name) else {
            continue;
        };
        out.push_str(&format!("## {name}\n"));
        let mut row = None;
        let mut next_column = 0;
        let mut line = String::new();
        while let Ok(Some(cell)) = cells.next_cell() {
            cells_read += 1;
            if cells_read.is_multiple_of(4096) {
                in_time(deadline)?;
            }
            let (at_row, column) = cell.get_position();
            if row != Some(at_row) {
                if !line.is_empty() {
                    out.push_str(&line);
                    out.push('\n');
                    line.clear();
                }
                row = Some(at_row);
                next_column = 0;
            }
            if out.len() > max_chars * 4 || cells_read >= MAX_CELLS {
                line.clear();
                break 'sheets;
            }
            let value = Data::from(cell.get_value().clone());
            if value == Data::Empty {
                continue;
            }
            for _ in next_column..column {
                line.push('\t');
            }
            line.push_str(&value.to_string());
            next_column = column;
        }
        if !line.is_empty() {
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(truncate_chars(out.trim_end(), max_chars).to_string())
}

fn spreadsheet(
    bytes: &[u8],
    max_chars: usize,
    deadline: Option<Instant>,
) -> Result<String, ToolError> {
    use calamine::{Data, Reader};

    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| {
        ToolError::new(
            ErrorCode::Unsupported,
            format!("the spreadsheet could not be opened: {e}"),
        )
    })?;
    if let calamine::Sheets::Xlsx(xlsx) = &mut workbook {
        return xlsx_text(xlsx, max_chars, deadline);
    }
    let mut out = String::new();
    for name in workbook.sheet_names() {
        in_time(deadline)?;
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        out.push_str(&format!("## {name}\n"));
        for row in range.rows() {
            in_time(deadline)?;
            let cells: Vec<String> = row
                .iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    other => other.to_string(),
                })
                .collect();
            if cells.iter().all(String::is_empty) {
                continue;
            }
            out.push_str(&cells.join("\t"));
            out.push('\n');
            if out.len() > max_chars * 4 {
                break;
            }
        }
        out.push('\n');
        if out.len() > max_chars * 4 {
            break;
        }
    }
    Ok(truncate_chars(out.trim_end(), max_chars).to_string())
}

/// The first `max_chars` characters of `s`.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Build small office documents and PDFs in memory for tests.
    use std::io::Write;

    pub fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, body) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    pub fn docx(paragraphs: &[&str]) -> Vec<u8> {
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
            .collect();
        zip(&[(
            "word/document.xml",
            &format!(
                r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
            ),
        )])
    }

    pub fn pptx(slides: &[&str]) -> Vec<u8> {
        let xml: Vec<(String, String)> = slides
            .iter()
            .enumerate()
            .map(|(i, text)| {
                (
                    format!("ppt/slides/slide{}.xml", i + 1),
                    format!(
                        r#"<?xml version="1.0"?><p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
                    ),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = xml.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        zip(&refs)
    }

    pub fn pdf(text: &str) -> Vec<u8> {
        pdf_pages(&[text])
    }

    /// A PDF with one line of text on each page.
    pub fn pdf_pages(pages: &[&str]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Object, Stream, dictionary};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let kids: Vec<Object> = pages
            .iter()
            .map(|text| {
                let content = Content {
                    operations: vec![
                        Operation::new("BT", vec![]),
                        Operation::new("Tf", vec!["F1".into(), 12.into()]),
                        Operation::new("Td", vec![72.into(), 720.into()]),
                        Operation::new("Tj", vec![Object::string_literal(*text)]),
                        Operation::new("ET", vec![]),
                    ],
                };
                let content_id =
                    doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
                doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => content_id,
                    "Resources" => resources_id,
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                })
                .into()
            })
            .collect();
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => kids.len() as i64,
                "Kids" => kids,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_and_binary() {
        assert_eq!(
            extract("a.md", b"# Title\nbody", 100).unwrap(),
            "# Title\nbody"
        );
        assert_eq!(extract("README", b"hello", 3).unwrap(), "hel");
        assert_eq!(
            extract("blob", b"\x7fELF\0\0", 100).unwrap_err().code,
            ErrorCode::Unsupported
        );
        assert_eq!(
            extract("photo.jpg", b"whatever", 100).unwrap_err().code,
            ErrorCode::Unsupported
        );
    }

    #[test]
    fn docx_and_pptx() {
        let docx = fixtures::docx(&[
            "Quarterly plan",
            "Budget &amp; hiring",
            "Caf&#233; &#x2014; lunch",
        ]);
        let text = extract("plan.docx", &docx, 1000).unwrap();
        assert!(text.contains("Quarterly plan\nBudget & hiring"), "{text:?}");
        assert!(text.contains("Caf\u{e9} \u{2014} lunch"), "{text:?}");

        let pptx = fixtures::pptx(&["Intro", "Roadmap"]);
        let text = extract("deck.pptx", &pptx, 1000).unwrap();
        assert!(text.contains("Slide 1") && text.contains("Intro"));
        assert!(text.find("Roadmap").unwrap() > text.find("Intro").unwrap());
    }

    #[test]
    fn pdf_text() {
        let pdf = fixtures::pdf("Invoice number 4711");
        let text = extract("invoice.pdf", &pdf, 1000).unwrap();
        assert!(text.contains("Invoice number 4711"), "{text:?}");
    }

    #[test]
    fn an_xlsx_reads_cell_by_cell_however_far_apart_its_cells_are() {
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
            <row r="1"><c r="A1" t="inlineStr"><is><t>Termination</t></is></c><c r="C1"><v>90</v></c></row>
            <row r="3"><c r="B3" t="inlineStr"><is><t>Interest</t></is></c></row>
            <row r="1048576"><c r="XFD1048576" t="inlineStr"><is><t>far</t></is></c></row>
            </sheetData></worksheet>"#;
        let xlsx = fixtures::zip(&[
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Terms" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            ("xl/worksheets/sheet1.xml", sheet),
        ]);
        // `worksheet_range` would ask for room for 17 billion cells here.
        let text = extract("terms.xlsx", &xlsx, 100_000).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "## Terms");
        assert_eq!(lines[1], "Termination\t\t90");
        assert_eq!(lines[2], "\tInterest");
        assert!(
            lines[3].ends_with("far") && lines[3].starts_with('\t'),
            "{text}"
        );
        let past = Some(Instant::now() - std::time::Duration::from_secs(1));
        assert!(extract_until("a.pdf", &fixtures::pdf("x"), 100, past).is_err());
        assert!(extract_until("a.docx", &fixtures::docx(&["x"]), 100, past).is_err());
        // An xlsx is asked between cells; a small one ends before it asks.
        assert_eq!(
            extract_until("b.pdf", &fixtures::pdf("x"), 100, None).unwrap(),
            "x"
        );
    }

    #[test]
    fn damaged_documents_are_errors() {
        for name in ["x.pdf", "x.docx", "x.xlsx", "x.pptx"] {
            let err = extract(name, b"not really", 100).unwrap_err();
            assert_eq!(err.code, ErrorCode::Unsupported, "{name}");
        }
    }

    #[test]
    fn caps_reads() {
        let mut data: &[u8] = b"0123456789";
        assert_eq!(
            read_capped(&mut data, 5).unwrap_err().code,
            ErrorCode::TooLarge
        );
        let mut data: &[u8] = b"01234";
        assert_eq!(read_capped(&mut data, 5).unwrap(), b"01234");
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 10), "hi");
    }
}
