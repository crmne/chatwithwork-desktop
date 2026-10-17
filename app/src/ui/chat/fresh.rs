//! Answer text eases in as it streams, as the web's does
//! (`fresh_text_helpers.js`, `.fresh-text` in `conversation.css`): only
//! characters a frame adds fade from transparent to their color over 400 ms
//! on `cubic-bezier(0.2, 0.7, 0.2, 1)`, and text already shown never fades
//! again. The web also eases a 2px blur away; egui draws glyphs from one
//! atlas without filters, so here it's opacity alone. Nothing fades under
//! reduced motion (`motion.rs`).
//!
//! Characters are counted in the order the answer paints them, glyph by
//! glyph across its paragraphs, code and tables, so a re-render that only
//! appends keeps the count of what was there. Each frame that shows more
//! than the last starts a batch at that count, with its own clock: a batch
//! keeps fading where it was when more text arrives, rather than starting
//! over, and the final reply, which replaces the streamed one under the
//! same message, lets the last words finish.

use std::cell::Cell;
use std::sync::Arc;

use egui::Galley;

/// How long a character takes to show.
pub const DURATION: f64 = 0.4;

/// The web's easing.
pub fn ease(t: f32) -> f32 {
    if t <= 0.0 {
        0.0
    } else if t >= 1.0 {
        1.0
    } else {
        super::tokens::cubic_bezier(0.2, 0.7, 0.2, 1.0, t)
    }
}

/// One answer's text, as it arrived.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fresh {
    /// Where each batch starts, in painted characters, and when it came.
    batches: Vec<(usize, f64)>,
    /// Characters shown so far, up to the last that isn't whitespace.
    shown: usize,
}

impl Fresh {
    /// Text already there when the answer is first seen shows as it is.
    pub fn settled(shown: usize) -> Self {
        Self {
            batches: Vec::new(),
            shown,
        }
    }

    /// The opacity of the `index`th painted character at `now`.
    pub fn opacity(&self, index: usize, now: f64) -> f32 {
        let batch = self.batches.partition_point(|(start, _)| *start <= index);
        match batch.checked_sub(1).map(|b| self.batches[b]) {
            Some((_, at)) if index >= self.batches[0].0 => ease(((now - at) / DURATION) as f32),
            _ => 1.0,
        }
    }

    /// A frame painted `shown` characters: what's new starts fading now.
    /// True while anything is still fading.
    pub fn painted(&mut self, shown: usize, now: f64) -> bool {
        if shown > self.shown {
            self.batches.push((self.shown, now));
            self.shown = shown;
        }
        // Batches that are done: their text is as if it was always there.
        if self
            .batches
            .last()
            .is_some_and(|(_, at)| now - at >= DURATION)
        {
            self.batches.clear();
        }
        !self.batches.is_empty()
    }

    pub fn fading(&self) -> bool {
        !self.batches.is_empty()
    }
}

/// One frame's painting of an answer: counts the characters as they're
/// painted and fades the new ones.
pub struct Pass<'a> {
    fresh: &'a Fresh,
    now: f64,
    /// Characters painted so far this frame, and up to the last one that
    /// isn't whitespace.
    count: Cell<usize>,
    inked: Cell<usize>,
}

impl<'a> Pass<'a> {
    pub fn new(fresh: &'a Fresh, now: f64) -> Self {
        Self {
            fresh,
            now,
            count: Cell::new(0),
            inked: Cell::new(0),
        }
    }

    /// Characters painted, ignoring trailing whitespace.
    pub fn shown(&self) -> usize {
        self.inked.get()
    }

    /// Count `galley`'s characters and return it with the new ones faded.
    pub fn apply(&self, galley: Arc<Galley>) -> Arc<Galley> {
        let start = self.count.get();
        let mut index = start;
        let mut faded = false;
        for row in &galley.rows {
            for glyph in &row.glyphs {
                if !glyph.chr.is_whitespace() {
                    self.inked.set(index + 1);
                }
                faded |= self.fresh.opacity(index, self.now) < 1.0;
                index += 1;
            }
        }
        self.count.set(index);
        if !faded {
            return galley;
        }
        let mut galley = (*galley).clone();
        let mut index = start;
        for placed in &mut galley.rows {
            let row = Arc::make_mut(&mut placed.row);
            let glyphs: Vec<u32> = row.glyphs.iter().map(|g| g.first_vertex).collect();
            let end = row.visuals.glyph_vertex_range.end as u32;
            let vertices = &mut row.visuals.mesh.vertices;
            for (n, first) in glyphs.iter().enumerate() {
                let opacity = self.fresh.opacity(index, self.now);
                index += 1;
                if opacity >= 1.0 {
                    continue;
                }
                let last = glyphs
                    .get(n + 1)
                    .copied()
                    .unwrap_or(end)
                    .min(vertices.len() as u32);
                for vertex in &mut vertices[*first as usize..last as usize] {
                    vertex.color = vertex.color.gamma_multiply(opacity);
                }
            }
        }
        Arc::new(galley)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_text_fades_and_each_batch_keeps_its_clock() {
        let mut fresh = Fresh::settled(10);
        assert_eq!(fresh.opacity(3, 0.0), 1.0, "what was there shows as is");
        assert!(fresh.painted(20, 1.0));
        assert_eq!(fresh.opacity(5, 1.0), 1.0);
        assert_eq!(fresh.opacity(15, 1.0), 0.0, "new text starts transparent");
        assert!(fresh.painted(30, 1.2));
        let older = fresh.opacity(15, 1.2);
        let newer = fresh.opacity(25, 1.2);
        assert!(older > 0.5 && newer == 0.0, "{older} {newer}");
        assert!(fresh.painted(30, 1.5), "the last batch still fades");
        assert!(!fresh.painted(30, 1.61), "all done");
        assert_eq!(fresh.opacity(25, 1.61), 1.0);
        assert!(!fresh.painted(30, 2.0), "nothing new, nothing fades");
    }

    /// Only the new glyphs' vertices lose opacity; the galley shown before
    /// is left as it was.
    #[test]
    fn a_pass_fades_only_the_new_glyphs() {
        let ctx = egui::Context::default();
        let mut galley = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            galley = Some(ui.painter().layout_no_wrap(
                "old new".into(),
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            ));
        });
        output.textures_delta.clear();
        let galley = galley.unwrap();
        let mut fresh = Fresh::settled(3);
        fresh.painted(7, 1.0);
        let pass = Pass::new(&fresh, 1.1);
        let faded = pass.apply(Arc::clone(&galley));
        assert_eq!(pass.shown(), 7);
        let row = &faded.rows[0].row;
        let alpha = |glyph: usize| {
            row.visuals.mesh.vertices[row.glyphs[glyph].first_vertex as usize]
                .color
                .a()
        };
        assert_eq!(alpha(0), 255, "old text as it was");
        assert!(alpha(4) < 255 && alpha(4) > 0, "{}", alpha(4));
        let settled = Pass::new(&fresh, 2.0).apply(Arc::clone(&galley));
        assert!(
            Arc::ptr_eq(&settled, &galley),
            "nothing to fade, nothing copied"
        );
    }

    #[test]
    fn the_easing_is_the_webs() {
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(1.0) - 1.0).abs() < 1e-4);
        // cubic-bezier(0.2, 0.7, 0.2, 1) is well past halfway at a quarter.
        assert!(ease(0.25) > 0.5, "{}", ease(0.25));
    }
}
