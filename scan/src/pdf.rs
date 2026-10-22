//! The searchable PDF: each scanned page as an image, with the words read
//! from it as invisible text placed over them, so the PDF looks exactly
//! like the scan while its text can be selected, copied and searched.
//!
//! This is what Tesseract's own PDF output and OCRmyPDF do, written here
//! with no Ghostscript and no Python: text in render mode 3 (invisible),
//! in a glyph-less font (Tesseract's `pdf.ttf`, Apache-2.0, in `assets/`)
//! whose every character is an empty glyph half an em wide. Characters are
//! written as their UTF-16 code units, as CIDs, with a ToUnicode map that
//! reads each CID as itself, so any language's text copies out as it was
//! read. Each word is stretched (`Tz`) to its box's width and sized to its
//! line's height, so selecting it highlights the word on the scan.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use image::{DynamicImage, GrayImage};
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, TextRenderingMode};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::ColorMode;
use crate::ocr::Word;

/// Tesseract's glyph-less font (Apache-2.0; see `assets/NOTICE`).
const GLYPHLESS_FONT: &[u8] = include_bytes!("../assets/glyphless.ttf");
/// Every character is this share of an em wide (the font's DW is 500).
const CHAR_WIDTH: f32 = 0.5;
/// JPEG quality for grey and colour pages: text stays crisp, a page of
/// letter in grey at 300 dpi is a few hundred kilobytes.
const JPEG_QUALITY: u8 = 80;

const TO_UNICODE: &str = "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfrange
<0000> <FFFF> <0000>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end
";

/// Builds a PDF a page at a time, so only one page's image is held.
pub struct Builder {
    pdf: Pdf,
    next: i32,
    catalog: Ref,
    tree: Ref,
    font: Ref,
    pages: Vec<Ref>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        let mut builder = Self {
            pdf: Pdf::new(),
            next: 1,
            catalog: Ref::new(1),
            tree: Ref::new(1),
            font: Ref::new(1),
            pages: Vec::new(),
        };
        builder.catalog = builder.id();
        builder.tree = builder.id();
        builder.font = builder.id();
        builder.write_font();
        builder
    }

    fn id(&mut self) -> Ref {
        let id = Ref::new(self.next);
        self.next += 1;
        id
    }

    fn write_font(&mut self) {
        let (cid, descriptor, file, map, cmap) =
            (self.id(), self.id(), self.id(), self.id(), self.id());
        let name = Name(b"GlyphLessFont");
        self.pdf
            .type0_font(self.font)
            .base_font(name)
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid)
            .to_unicode(cmap);
        self.pdf
            .cid_font(cid)
            .subtype(CidFontType::Type2)
            .base_font(name)
            .system_info(SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            })
            .font_descriptor(descriptor)
            .default_width(1000.0 * CHAR_WIDTH)
            .cid_to_gid_map_stream(map);
        self.pdf
            .font_descriptor(descriptor)
            .name(name)
            .flags(FontFlags::FIXED_PITCH | FontFlags::SYMBOLIC)
            .bbox(Rect::new(0.0, 0.0, 1000.0 * CHAR_WIDTH, 1000.0))
            .italic_angle(0.0)
            .ascent(1000.0)
            .descent(-1.0)
            .cap_height(1000.0)
            .stem_v(80.0)
            .font_file2(file);
        self.pdf
            .stream(file, GLYPHLESS_FONT)
            .pair(Name(b"Length1"), GLYPHLESS_FONT.len() as i32);
        // Every CID to glyph 1, the empty glyph (glyph 0 is .notdef, which
        // some readers refuse to select).
        let gids: Vec<u8> = (0..65536).flat_map(|_| [0u8, 1u8]).collect();
        self.pdf
            .stream(map, &deflate(&gids))
            .filter(Filter::FlateDecode);
        self.pdf
            .stream(cmap, &deflate(TO_UNICODE.as_bytes()))
            .filter(Filter::FlateDecode);
    }

    /// Adds a page: `image` as the person edited it, scanned at
    /// `resolution`, kept as `color`, with the words read from it.
    pub fn page(
        &mut self,
        image: &DynamicImage,
        resolution: u32,
        color: ColorMode,
        words: &[Word],
    ) -> anyhow::Result<()> {
        let (page, image_id, content_id) = (self.id(), self.id(), self.id());
        let scale = 72.0 / resolution.max(1) as f32;
        // In f64 and divided last, so whole sizes stay whole (850 px at 100 dpi
        // is exactly 612 pt).
        let points = |px: u32| (f64::from(px) * 72.0 / f64::from(resolution.max(1))) as f32;
        let (w, h) = (points(image.width()), points(image.height()));

        match color {
            ColorMode::BlackAndWhite => {
                let bits = pack_bits(&threshold(&image.to_luma8()));
                let data = deflate(&bits);
                let mut x = self.pdf.image_xobject(image_id, &data);
                x.filter(Filter::FlateDecode);
                x.width(image.width() as i32).height(image.height() as i32);
                x.color_space().device_gray();
                x.bits_per_component(1);
            }
            ColorMode::Grayscale | ColorMode::Color => {
                let jpeg = jpeg(image, color == ColorMode::Color)?;
                let mut x = self.pdf.image_xobject(image_id, &jpeg);
                x.filter(Filter::DctDecode);
                x.width(image.width() as i32).height(image.height() as i32);
                if color == ColorMode::Color {
                    x.color_space().device_rgb();
                } else {
                    x.color_space().device_gray();
                }
                x.bits_per_component(8);
            }
        }

        let mut content = Content::new();
        content.save_state();
        content.transform([w, 0.0, 0.0, h, 0.0, 0.0]);
        content.x_object(Name(b"Im1"));
        content.restore_state();
        text_layer(&mut content, words, image.height(), scale);
        let content = deflate(&content.finish());
        self.pdf
            .stream(content_id, &content)
            .filter(Filter::FlateDecode);

        let mut writer = self.pdf.page(page);
        writer.media_box(Rect::new(0.0, 0.0, w, h));
        writer.parent(self.tree);
        writer.contents(content_id);
        let mut resources = writer.resources();
        resources.x_objects().pair(Name(b"Im1"), image_id);
        resources.fonts().pair(Name(b"F1"), self.font);
        resources.finish();
        writer.finish();
        self.pages.push(page);
        Ok(())
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn finish(mut self) -> Vec<u8> {
        let info = self.id();
        self.pdf.catalog(self.catalog).pages(self.tree);
        let count = self.pages.len() as i32;
        self.pdf
            .pages(self.tree)
            .kids(self.pages.iter().copied())
            .count(count);
        self.pdf
            .document_info(info)
            .producer(TextStr("Chat with Work Desktop"));
        self.pdf.finish()
    }
}

/// A page as JPEG, with as many components as the PDF declares: three for
/// colour, one for grey. (Encoding a grey DynamicImage wrote three, which
/// viewers then read as a grey image three times as wide.)
fn jpeg(image: &DynamicImage, color: bool) -> anyhow::Result<Vec<u8>> {
    use image::ExtendedColorType;
    use image::codecs::jpeg::JpegEncoder;
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    if color {
        let rgb = image.to_rgb8();
        encoder.encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )?;
    } else {
        let gray = image.to_luma8();
        encoder.encode(
            gray.as_raw(),
            gray.width(),
            gray.height(),
            ExtendedColorType::L8,
        )?;
    }
    Ok(out)
}

/// The words as invisible text, in PDF points from the page's bottom left.
fn text_layer(content: &mut Content, words: &[Word], image_height: u32, scale: f32) {
    let words: Vec<(&Word, Vec<u8>, usize)> = words
        .iter()
        .filter_map(|word| {
            let units: Vec<u16> = word.text.encode_utf16().collect();
            // A word followed by a space, so copied text keeps its spaces.
            let mut bytes: Vec<u8> = units
                .iter()
                .chain(std::iter::once(&0x20))
                .flat_map(|u| u.to_be_bytes())
                .collect();
            if units.is_empty() || word.width == 0 || word.height == 0 {
                bytes.clear();
            }
            (!bytes.is_empty()).then_some((word, bytes, units.len()))
        })
        .collect();
    if words.is_empty() {
        return;
    }
    content.begin_text();
    content.set_text_rendering_mode(TextRenderingMode::Invisible);
    for (word, bytes, chars) in words {
        let size = word.height as f32 * scale;
        let left = word.x as f32 * scale;
        let bottom = image_height.saturating_sub(word.y + word.height) as f32 * scale;
        let width = word.width as f32 * scale;
        let stretch = 100.0 * width / (chars as f32 * size * CHAR_WIDTH);
        content.set_font(Name(b"F1"), size);
        content.set_text_matrix([1.0, 0.0, 0.0, 1.0, left, bottom]);
        content.set_horizontal_scaling(stretch);
        content.show(Str(&bytes));
    }
    content.end_text();
}

/// Black and white by Otsu's threshold, which finds the split between ink
/// and paper on each page, so faint print and grey paper both come out.
pub fn threshold(gray: &GrayImage) -> GrayImage {
    let mut histogram = [0u64; 256];
    for p in gray.pixels() {
        histogram[p[0] as usize] += 1;
    }
    let total: u64 = histogram.iter().sum();
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(i, n)| i as f64 * *n as f64)
        .sum();
    let (mut best, mut best_variance) = (128u8, -1.0);
    let (mut weight, mut weighted) = (0u64, 0f64);
    for (t, n) in histogram.iter().enumerate() {
        weight += n;
        if weight == 0 || weight == total {
            continue;
        }
        weighted += t as f64 * *n as f64;
        let (w0, w1) = (weight as f64, (total - weight) as f64);
        let (m0, m1) = (weighted / w0, (sum - weighted) / w1);
        let variance = w0 * w1 * (m0 - m1).powi(2);
        if variance > best_variance {
            best_variance = variance;
            best = t as u8;
        }
    }
    // A page with almost nothing on it has no real split; keep it white
    // rather than turning paper texture into noise.
    let cut = if best_variance < 0.0 {
        128
    } else {
        best.max(60)
    };
    GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        image::Luma([if gray.get_pixel(x, y)[0] <= cut {
            0
        } else {
            255
        }])
    })
}

/// One bit a pixel, rows padded to a byte, 1 for white (DeviceGray).
fn pack_bits(bw: &GrayImage) -> Vec<u8> {
    let row = bw.width().div_ceil(8) as usize;
    let mut out = vec![0u8; row * bw.height() as usize];
    for (x, y, p) in bw.enumerate_pixels() {
        if p[0] > 127 {
            out[y as usize * row + x as usize / 8] |= 0x80 >> (x % 8);
        }
    }
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("writing to memory");
    encoder.finish().expect("writing to memory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn page() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_fn(850, 1100, |x, y| {
            Luma([if (300..320).contains(&y) && x > 100 {
                0
            } else {
                250
            }])
        }))
    }

    fn words() -> Vec<Word> {
        vec![
            Word {
                text: "Grüße".into(),
                x: 100,
                y: 290,
                width: 200,
                height: 40,
            },
            Word {
                text: "aus".into(),
                x: 320,
                y: 290,
                width: 90,
                height: 40,
            },
        ]
    }

    #[test]
    fn writes_pages_with_their_images_and_text() {
        for color in [
            ColorMode::BlackAndWhite,
            ColorMode::Grayscale,
            ColorMode::Color,
        ] {
            let mut builder = Builder::new();
            builder.page(&page(), 100, color, &words()).unwrap();
            builder.page(&page(), 100, color, &[]).unwrap();
            assert_eq!(builder.page_count(), 2);
            let pdf = builder.finish();
            let text = String::from_utf8_lossy(&pdf);
            assert!(pdf.starts_with(b"%PDF-"));
            assert!(text.contains("/Count 2"));
            // 850 x 1100 px at 100 dpi is 612 x 792 pt, a US letter page.
            assert!(text.contains("/MediaBox [0 0 612 792]"), "{color:?}");
            assert!(text.contains(if color == ColorMode::BlackAndWhite {
                "/BitsPerComponent 1"
            } else {
                "/DCTDecode"
            }));
            assert!(text.contains("/GlyphLessFont"));
        }
    }

    /// The number of components a baseline or progressive JPEG says it has.
    fn jpeg_components(jpeg: &[u8]) -> u8 {
        let at = jpeg
            .windows(2)
            .position(|w| w == [0xFF, 0xC0] || w == [0xFF, 0xC2])
            .expect("a frame header");
        jpeg[at + 9]
    }

    #[test]
    fn grey_pages_are_one_component_jpegs_and_colour_three() {
        assert_eq!(jpeg_components(&jpeg(&page(), false).unwrap()), 1);
        assert_eq!(jpeg_components(&jpeg(&page(), true).unwrap()), 3);
    }

    #[test]
    fn text_is_invisible_utf16_stretched_to_each_word() {
        let mut content = Content::new();
        text_layer(&mut content, &words(), 1100, 0.72);
        let ops = String::from_utf8_lossy(&content.finish()).into_owned();
        assert!(ops.contains("3 Tr"));
        // "Grüße " in UTF-16BE: ü is 00FC, ß is 00DF.
        assert!(
            ops.contains("<004700720075030800DF00650020>")
                || ops.contains("<0047007200FC00DF00650020>"),
            "{ops}"
        );
        // 200 px wide at 0.72 pt/px, 5 characters of half an em at 28.8 pt.
        let stretch = 100.0 * (200.0 * 0.72) / (5.0 * 28.8 * 0.5);
        assert!(ops.contains(&format!("{} Tz", stretch as f32)), "{ops}");
        // Baseline at the box's bottom: 1100 - 330 = 770 px.
        assert!(ops.contains(&format!("{} Tm", 770.0f32 * 0.72)), "{ops}");
    }

    #[test]
    fn thresholds_ink_and_paper() {
        let bw = threshold(&page().to_luma8());
        assert_eq!(bw.get_pixel(200, 310)[0], 0);
        assert_eq!(bw.get_pixel(200, 10)[0], 255);
        let blank = threshold(&GrayImage::from_pixel(10, 10, Luma([240])));
        assert!(blank.pixels().all(|p| p[0] == 255));
    }

    #[test]
    fn packs_one_bit_per_pixel_with_padded_rows() {
        let bw = GrayImage::from_fn(10, 2, |x, _| Luma([if x == 0 { 0 } else { 255 }]));
        assert_eq!(pack_bits(&bw), vec![0x7F, 0xC0, 0x7F, 0xC0]);
    }
}
