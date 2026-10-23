//! Blank pages: the backs of one-sided letters a duplex scan brings in.
//!
//! A page is looked at in grey at 150 dpi (enough to keep a 10-point
//! letter's strokes dark), without its margins (where hole punches, staples
//! and the feeder's shadows are), as a grid of cells about 4 mm across. A
//! cell holds ink when a share of its pixels are much darker than the
//! paper; specks of dust and scanner noise darken a few. A page is blank
//! when no more than [`MAX_INKED_CELLS`] cells hold ink, which a single
//! word ("Anlage", a page number) already exceeds. Show-through from the
//! other side is light grey and stays under the darkness threshold.

use image::{DynamicImage, GrayImage, imageops::FilterType};

/// The resolution pages are judged at.
const LOOK_DPI: f64 = 150.0;
/// The cell size at that resolution, in pixels (about 4 mm).
const CELL: u32 = 24;
/// Pixels in a cell that must be dark for it to hold ink (2% of it).
const DARK_PIXELS_PER_CELL: usize = 12;
/// Cells with ink a blank page may still have (dust, a crease).
pub const MAX_INKED_CELLS: usize = 2;
/// Margins left out, as a share of each side.
const MARGIN_X: f64 = 0.07;
const MARGIN_Y: f64 = 0.05;
/// A row or column darker than this share of the way across is an edge.
const EDGE_SHARE: f64 = 0.6;

/// Whether a page scanned at `resolution` is blank.
pub fn is_blank(image: &DynamicImage, resolution: u32) -> bool {
    inked_cells(image, resolution) <= MAX_INKED_CELLS
}

/// How many cells of the page hold ink.
pub fn inked_cells(image: &DynamicImage, resolution: u32) -> usize {
    let gray = small_gray(image, resolution);
    let (w, h) = gray.dimensions();
    if w < CELL * 2 || h < CELL * 2 {
        return 0;
    }
    let (x0, x1) = (
        (f64::from(w) * MARGIN_X) as u32,
        w - (f64::from(w) * MARGIN_X) as u32,
    );
    let (y0, y1) = (
        (f64::from(h) * MARGIN_Y) as u32,
        h - (f64::from(h) * MARGIN_Y) as u32,
    );
    let threshold = dark_threshold(&gray, x0, x1, y0, y1);
    let dark = |x: u32, y: u32| gray.get_pixel(x, y)[0] < threshold;
    // Rows and columns dark nearly all the way across are the paper's edge
    // or the scanner's background past it, never writing, whose letters
    // always leave gaps.
    let rule = |count: usize, span: u32| count as f64 > f64::from(span) * EDGE_SHARE;
    let rows: Vec<bool> = (0..h)
        .map(|y| rule((x0..x1).filter(|&x| dark(x, y)).count(), x1 - x0))
        .collect();
    let cols: Vec<bool> = (0..w)
        .map(|x| rule((y0..y1).filter(|&y| dark(x, y)).count(), y1 - y0))
        .collect();
    let mut inked = 0;
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let mut dark_pixels = 0;
            for cy in y..(y + CELL).min(y1) {
                for cx in x..(x + CELL).min(x1) {
                    if !rows[cy as usize] && !cols[cx as usize] && dark(cx, cy) {
                        dark_pixels += 1;
                    }
                }
            }
            if dark_pixels >= DARK_PIXELS_PER_CELL {
                inked += 1;
            }
            x += CELL;
        }
        y += CELL;
    }
    inked
}

fn small_gray(image: &DynamicImage, resolution: u32) -> GrayImage {
    let scale = (LOOK_DPI / f64::from(resolution.max(1))).min(1.0);
    let (w, h) = (
        ((f64::from(image.width()) * scale).round() as u32).max(1),
        ((f64::from(image.height()) * scale).round() as u32).max(1),
    );
    // Area averaging keeps thin strokes as grey rather than dropping them.
    image.resize_exact(w, h, FilterType::Triangle).into_luma8()
}

/// Darker than this is ink: well below the paper's own brightness (its
/// most common grey), and never lighter than mid-grey, so show-through and
/// grey recycled paper aren't ink.
fn dark_threshold(gray: &GrayImage, x0: u32, x1: u32, y0: u32, y1: u32) -> u8 {
    let mut histogram = [0usize; 256];
    for y in y0..y1 {
        for x in x0..x1 {
            histogram[gray.get_pixel(x, y)[0] as usize] += 1;
        }
    }
    let paper = histogram
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(v, _)| v as i32)
        .unwrap_or(255);
    (paper - 70).clamp(20, 160) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    /// An A4 page at 150 dpi, white, with dark rectangles where asked.
    fn page(marks: &[(u32, u32, u32, u32)], paper: u8) -> DynamicImage {
        let mut img = GrayImage::from_pixel(1240, 1754, Luma([paper]));
        for &(x, y, w, h) in marks {
            for py in y..y + h {
                for px in x..x + w {
                    img.put_pixel(px, py, Luma([20]));
                }
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn white_and_grey_paper_are_blank() {
        assert!(is_blank(&page(&[], 255), 150));
        assert!(is_blank(&page(&[], 200), 150));
    }

    #[test]
    fn dust_and_hole_punches_in_the_margin_are_blank() {
        // Two specks in the page, and two punched holes at the left edge.
        let marks = [
            (600, 700, 2, 2),
            (300, 1200, 3, 3),
            (30, 600, 40, 40),
            (30, 1100, 40, 40),
        ];
        assert!(is_blank(&page(&marks, 250), 150));
    }

    #[test]
    fn a_single_word_is_not_blank() {
        // "Anlage" at 12 pt: six letters about 7 x 10 px each, 2 px strokes.
        let marks: Vec<_> = (0..6)
            .map(|i| (500 + i * 14, 800, 2, 20))
            .chain((0..6).map(|i| (500 + i * 14, 800, 9, 2)))
            .collect();
        assert!(!is_blank(&page(&marks, 255), 150));
    }

    #[test]
    fn show_through_is_blank() {
        let mut img = GrayImage::from_pixel(1240, 1754, Luma([250]));
        for y in 300..1400 {
            for x in 200..1000 {
                if (y / 20) % 2 == 0 && (x / 9) % 3 != 0 {
                    img.put_pixel(x, y, Luma([215]));
                }
            }
        }
        assert!(is_blank(&DynamicImage::ImageLuma8(img), 150));
    }

    #[test]
    fn the_papers_edge_and_the_scanners_background_are_blank() {
        // A back the feeder scanned past the paper: the paper's edge as a line
        // across, the scanner's grey below it, and a shadow down one side.
        let mut img = GrayImage::from_pixel(1240, 1754, Luma([250]));
        for y in 1500..1754 {
            for x in 0..1240 {
                img.put_pixel(x, y, Luma([if y < 1504 { 30 } else { 120 }]));
            }
        }
        for y in 0..1500 {
            for x in 1150..1158 {
                img.put_pixel(x, y, Luma([60]));
            }
        }
        assert!(is_blank(&DynamicImage::ImageLuma8(img), 150));
    }

    #[test]
    fn a_letter_is_not_blank() {
        // Thirty lines of words: 50 px each, with the gaps between them.
        let marks: Vec<_> = (0..30)
            .flat_map(|line| (0..11).map(move |word| (180 + word * 70, 300 + line * 40, 50, 12)))
            .collect();
        assert!(!is_blank(&page(&marks, 255), 150));
    }
}
