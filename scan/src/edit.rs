//! What the person changes on a page before sending it: a crop and a turn.
//! The scanned image is never changed; each is applied when the page is
//! shown or written into the PDF.

use image::DynamicImage;
use serde::{Deserialize, Serialize};

/// A crop as shares of the turned page's width and height, from its top
/// left: `{x: 0, y: 0, width: 1, height: 1}` is the whole page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Crop {
    /// The smallest a crop may be, as a share of each side.
    const MIN: f64 = 0.05;

    /// The crop kept inside the page and at least [`Self::MIN`] across;
    /// `None` for one that's the whole page or not a number.
    pub fn normalized(self) -> Option<Crop> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let x = self.x.clamp(0.0, 1.0 - Self::MIN);
        let y = self.y.clamp(0.0, 1.0 - Self::MIN);
        let width = self.width.clamp(Self::MIN, 1.0 - x);
        let height = self.height.clamp(Self::MIN, 1.0 - y);
        let whole = x < 1e-3 && y < 1e-3 && width > 1.0 - 2e-3 && height > 1.0 - 2e-3;
        (!whole).then_some(Crop {
            x,
            y,
            width,
            height,
        })
    }
}

/// A page's edits.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Edits {
    /// Clockwise quarter turns, 0 to 3.
    #[serde(default)]
    pub turns: u8,
    #[serde(default)]
    pub crop: Option<Crop>,
}

impl Edits {
    /// The page as it will be sent.
    pub fn apply(&self, image: &DynamicImage) -> DynamicImage {
        let turned = match self.turns % 4 {
            1 => image.rotate90(),
            2 => image.rotate180(),
            3 => image.rotate270(),
            _ => image.clone(),
        };
        match self.crop.and_then(Crop::normalized) {
            Some(crop) => {
                let (w, h) = (f64::from(turned.width()), f64::from(turned.height()));
                let x = (crop.x * w).round() as u32;
                let y = (crop.y * h).round() as u32;
                let cw = ((crop.width * w).round() as u32)
                    .clamp(1, turned.width() - x.min(turned.width() - 1));
                let ch = ((crop.height * h).round() as u32)
                    .clamp(1, turned.height() - y.min(turned.height() - 1));
                turned.crop_imm(x, y, cw, ch)
            }
            None => turned,
        }
    }

    /// Turned once more, clockwise. A crop turns with the page.
    pub fn turned(self) -> Edits {
        Edits {
            turns: (self.turns + 1) % 4,
            crop: self.crop.map(|c| Crop {
                x: 1.0 - c.y - c.height,
                y: c.x,
                width: c.height,
                height: c.width,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_fn(w, h, |x, y| Luma([(x * 10 + y) as u8])))
    }

    #[test]
    fn crops_and_turns() {
        let page = image(100, 200);
        let edits = Edits {
            turns: 0,
            crop: Some(Crop {
                x: 0.1,
                y: 0.5,
                width: 0.5,
                height: 0.25,
            }),
        };
        let out = edits.apply(&page);
        assert_eq!((out.width(), out.height()), (50, 50));
        assert_eq!(
            out.to_luma8().get_pixel(0, 0)[0],
            page.to_luma8().get_pixel(10, 100)[0]
        );
        let turned = Edits {
            turns: 1,
            crop: None,
        }
        .apply(&page);
        assert_eq!((turned.width(), turned.height()), (200, 100));
    }

    #[test]
    fn a_crop_turns_with_the_page() {
        let page = image(100, 200);
        let edits = Edits {
            turns: 0,
            crop: Some(Crop {
                x: 0.1,
                y: 0.5,
                width: 0.5,
                height: 0.25,
            }),
        };
        let before = edits.apply(&page).rotate90();
        let after = edits.turned().apply(&page);
        assert_eq!(before.to_luma8(), after.to_luma8());
    }

    #[test]
    fn whole_page_and_nonsense_crops_are_dropped() {
        assert_eq!(
            Crop {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }
            .normalized(),
            None
        );
        assert_eq!(
            Crop {
                x: f64::NAN,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }
            .normalized(),
            None
        );
        let kept = Crop {
            x: -1.0,
            y: 0.99,
            width: 9.0,
            height: 0.0,
        }
        .normalized()
        .unwrap();
        assert_eq!((kept.x, kept.width), (0.0, 1.0));
        assert!((kept.y - 0.95).abs() < 1e-9 && (kept.height - 0.05).abs() < 1e-9);
    }
}
