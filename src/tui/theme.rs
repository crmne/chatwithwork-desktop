//! Live Wire, in a terminal, in the terminal's own colours: it never paints
//! a background, so every cell keeps the terminal's, light or dark, themed or
//! not. Text is the terminal's foreground, with bold and dim for hierarchy;
//! the selection is reverse video, the terminal's own highlight; signal
//! colours mark state only, through the 16 ANSI colours a theme restyles:
//! green done, blue working, yellow waiting for someone, red failed. The
//! rainbow is only for the composer while an answer is written, and only in
//! the foreground.
//!
//! With `NO_COLOR` set (or `TERM=dumb`) there is no colour at all, only bold,
//! dim and reverse.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// What a colour means. Signals mark state only, never decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Positive,
    Live,
    Attention,
    Negative,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR`: modifiers only.
    Mono,
    /// The 16 ANSI colours, as the terminal's theme draws them.
    Ansi,
}

/// The rainbow's hues in ANSI colours, in the web's order: green, blue,
/// violet, orange, lime.
const RAINBOW: [Color; 5] = [
    Color::Green,
    Color::Blue,
    Color::Magenta,
    Color::Red,
    Color::Yellow,
];

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub depth: Depth,
}

impl Theme {
    pub fn new(depth: Depth) -> Self {
        Self { depth }
    }

    /// Colour unless `NO_COLOR` is set or the terminal is dumb.
    pub fn detect() -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        if !var("NO_COLOR").is_empty() || var("TERM") == "dumb" {
            Self::new(Depth::Mono)
        } else {
            Self::new(Depth::Ansi)
        }
    }

    fn mono(&self) -> bool {
        self.depth == Depth::Mono
    }

    fn fg(&self, color: Color) -> Style {
        if self.mono() {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }

    /// The selected row, or the focused one: reverse video, so it's the
    /// terminal's own colours swapped, whatever its theme.
    pub fn selected(&self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED)
    }

    /// Text: the terminal's foreground.
    pub fn ink(&self) -> Style {
        Style::new()
    }

    pub fn strong(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    /// Secondary text.
    pub fn muted(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    /// Hints, metadata and placeholders.
    pub fn faint(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    /// Inline and fenced code in answers: cyan, which no state uses.
    pub fn code(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            self.fg(Color::Cyan)
        }
    }

    /// Small labels: `RECENT`, `PROJECTS`, `PINNED`.
    pub fn micro(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM | Modifier::BOLD)
    }

    /// Hairlines and borders between regions.
    pub fn line(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    /// A dot or glyph in a signal colour.
    pub fn signal(&self, signal: Signal) -> Style {
        match signal {
            Signal::Positive => self.fg(Color::Green),
            Signal::Live => self.fg(Color::Blue),
            Signal::Attention => self.fg(Color::Yellow),
            Signal::Negative => self.fg(Color::Red),
            Signal::Idle => self.faint(),
        }
    }

    /// A card's ring in its signal's colour, as the web rings a card that
    /// needs someone (never a stripe down one side, never a wash behind it).
    pub fn edge(&self, signal: Signal) -> Style {
        match signal {
            Signal::Idle => self.line(),
            _ if self.mono() => Style::new().add_modifier(Modifier::BOLD),
            other => self.signal(other),
        }
    }

    /// Small text in a signal colour (the web's `-ink` variants); bold
    /// without colour where it needs someone.
    pub fn signal_ink(&self, signal: Signal) -> Style {
        match signal {
            Signal::Negative | Signal::Attention if self.mono() => {
                Style::new().add_modifier(Modifier::BOLD)
            }
            Signal::Idle => self.muted(),
            other => self.signal(other),
        }
    }

    /// The rainbow at `t` (0 to 1) along its length, in the foreground.
    pub fn rainbow(&self, t: f32) -> Style {
        if self.mono() {
            return Style::new().add_modifier(Modifier::BOLD);
        }
        self.fg(rainbow_at(t))
    }

    /// `text` dim, with a band of full-strength text sweeping across it,
    /// the web's `.shimmer` for status words like "Thinking". `t` (0 to 1)
    /// is how far along its two-second sweep the band is.
    pub fn shimmer_text(&self, text: &str, t: f32) -> Vec<Span<'static>> {
        if self.mono() {
            return vec![Span::styled(text.to_string(), self.muted())];
        }
        let chars: Vec<char> = text.chars().collect();
        let width = chars.len().max(1) as f32;
        chars
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let lit = shimmer_at(i as f32 + 0.5, width, t) > 0.5;
                let style = if lit { self.ink() } else { self.muted() };
                Span::styled(c.to_string(), style)
            })
            .collect()
    }
}

/// The rainbow's ANSI colour at `t` (0 to 1).
pub fn rainbow_at(t: f32) -> Color {
    let at = (t.clamp(0.0, 1.0) * RAINBOW.len() as f32) as usize;
    RAINBOW[at.min(RAINBOW.len() - 1)]
}

/// How much of the shimmer's band covers `x` in text `width` wide, `t` of
/// the way through its sweep: 0 outside the band, 1 at its middle. As the
/// web draws it: a gradient twice the text's width, lit from 35% to 65% and
/// peaking at 50%, moving across in one sweep.
pub fn shimmer_at(x: f32, width: f32, t: f32) -> f32 {
    let width = width.max(1.0);
    let offset = -2.0 * width + 4.0 * width * t.rem_euclid(1.0);
    let u = ((x - offset) / (2.0 * width)).rem_euclid(1.0);
    if !(0.35..=0.65).contains(&u) {
        0.0
    } else if u < 0.5 {
        (u - 0.35) / 0.15
    } else {
        (0.65 - u) / 0.15
    }
}

/// Whether a cell is drawn only in the terminal's own colours: its
/// background never set, and its foreground the terminal's or one of the 16
/// colours its theme defines. The tests hold every screen to this.
pub fn terminal_colours_only(fg: Color, bg: Color) -> bool {
    bg == Color::Reset && !matches!(fg, Color::Rgb(..) | Color::Indexed(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rainbow_runs_through_ansi_colours() {
        assert_eq!(rainbow_at(0.0), Color::Green);
        assert_eq!(rainbow_at(0.5), Color::Magenta);
        assert_eq!(rainbow_at(1.0), Color::Yellow);
    }

    #[test]
    fn the_shimmer_band_sweeps_across() {
        // Somewhere along the sweep each letter is under the band, and
        // the band moves on rather than staying put.
        let width = 8.0;
        let at = |x: f32| {
            (0..100)
                .map(|i| shimmer_at(x, width, i as f32 / 100.0))
                .fold(0.0, f32::max)
        };
        for x in 0..8 {
            assert!(at(x as f32 + 0.5) > 0.9, "letter {x}");
        }
        assert_ne!(shimmer_at(0.5, width, 0.3), shimmer_at(0.5, width, 0.4));
        let theme = Theme::new(Depth::Ansi);
        assert_eq!(theme.shimmer_text("Thinking", 0.0).len(), 8);
        assert_eq!(
            Theme::new(Depth::Mono).shimmer_text("Thinking", 0.0).len(),
            1
        );
    }

    #[test]
    fn no_style_paints_a_background() {
        for theme in [Theme::new(Depth::Ansi), Theme::new(Depth::Mono)] {
            let styles = [
                theme.selected(),
                theme.ink(),
                theme.strong(),
                theme.muted(),
                theme.faint(),
                theme.code(),
                theme.micro(),
                theme.line(),
                theme.rainbow(0.3),
            ]
            .into_iter()
            .chain(
                [
                    Signal::Positive,
                    Signal::Live,
                    Signal::Attention,
                    Signal::Negative,
                    Signal::Idle,
                ]
                .into_iter()
                .flat_map(|s| [theme.signal(s), theme.edge(s), theme.signal_ink(s)]),
            );
            for style in styles {
                assert_eq!(style.bg, None, "{style:?}");
                assert!(
                    !matches!(style.fg, Some(Color::Rgb(..) | Color::Indexed(_))),
                    "{style:?}"
                );
            }
        }
    }

    #[test]
    fn no_color_means_no_colour() {
        let theme = Theme::new(Depth::Mono);
        assert_eq!(theme.ink(), Style::new());
        assert_eq!(theme.signal(Signal::Positive), Style::new());
        assert_eq!(theme.rainbow(0.2).fg, None);
        assert!(theme.selected().add_modifier.contains(Modifier::REVERSED));
    }
}
