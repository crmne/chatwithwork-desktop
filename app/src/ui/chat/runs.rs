//! The commands this computer runs for Chat with Work (`run`), docked over
//! the composer: one line while any runs ("Running bin/rails test in App ›
//! api · 0:42", Show, Stop), and, shown, the list with what the chosen one
//! prints as it prints it. They come from the daemon here, never from the
//! server (`cww::commands::follow`, CONTROL.md `commands`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cww::commands::follow::{Follow, Listed, Snapshot, clean, elapsed, one_line};
use egui::{Id, Rect, Sense, Ui, pos2, vec2};

use super::icons::{Icon, Images};
use super::tokens::{self, Palette, scale};
use super::{paint, widgets};

/// The line while it's hidden.
const LINE: f32 = 40.0;
/// The shown panel's parts.
const HEADER: f32 = 36.0;
const ROW: f32 = 32.0;
const ROWS: usize = 5;
const OUTPUT: f32 = 188.0;
const PAD: f32 = 12.0;
/// Code's type, a step smaller: output can be wide.
const OUTPUT_TYPE: tokens::Type = tokens::Type::mono(12.0, 18.0).tracking(0.0);

#[derive(Default)]
pub struct Runs {
    follow: Option<(PathBuf, Arc<Follow>)>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    snapshot: Snapshot,
    /// The list and the output are shown.
    open: bool,
    selected: Option<String>,
    /// A stop that didn't go.
    failure: Arc<Mutex<Option<String>>>,
}

impl Runs {
    /// Follow the commands on the daemon at `socket`: this computer's own,
    /// whichever organization's chats are open.
    pub fn follow(&mut self, socket: PathBuf, ctx: &egui::Context) {
        if self.follow.as_ref().is_some_and(|(s, _)| *s == socket) {
            return;
        }
        let (latest, ctx) = (Arc::clone(&self.latest), ctx.clone());
        let follow = Follow::spawn(socket.clone(), move |snapshot| {
            *latest.lock().expect("commands") = Some(snapshot);
            ctx.request_repaint();
        });
        if self.open {
            follow.focus(self.selected.clone());
        }
        self.follow = Some((socket, Arc::new(follow)));
    }

    /// Take what the daemon said since the last frame.
    pub fn poll(&mut self) {
        if let Some(snapshot) = self.latest.lock().expect("commands").take() {
            self.snapshot = snapshot;
        }
        if self.snapshot.commands.is_empty() {
            self.open = false;
        }
    }

    /// Why a stop didn't go, once.
    pub fn take_failure(&self) -> Option<String> {
        self.failure.lock().expect("failure").take()
    }

    fn visible(&self) -> bool {
        !self.snapshot.commands.is_empty() && (self.open || self.snapshot.running() > 0)
    }

    /// How tall it is this frame, with the gap over the composer.
    pub fn height(&self) -> f32 {
        if !self.visible() {
            return 0.0;
        }
        if !self.open {
            return LINE + 8.0;
        }
        let rows = self.snapshot.commands.len().min(ROWS) as f32;
        HEADER + rows * ROW + 8.0 + OUTPUT + PAD + 8.0
    }

    fn chosen(&self) -> Option<&Listed> {
        let list = &self.snapshot.commands;
        self.selected
            .as_deref()
            .and_then(|id| list.iter().find(|c| c.id() == id))
            .or_else(|| list.first())
    }

    fn choose(&mut self, id: Option<String>) {
        self.selected = id;
        if let Some((_, follow)) = &self.follow {
            follow.focus(self.selected.clone());
        }
    }

    fn show_list(&mut self) {
        self.open = true;
        let first = self.snapshot.commands.first().map(|c| c.id().to_string());
        self.choose(self.selected.clone().or(first));
    }

    fn hide_list(&mut self) {
        self.open = false;
        if let Some((_, follow)) = &self.follow {
            follow.focus(None);
        }
    }

    fn stop(&mut self, run: &str, ctx: &egui::Context) {
        let Some((_, follow)) = &self.follow else {
            return;
        };
        let (follow, run, failure, ctx) = (
            Arc::clone(follow),
            run.to_string(),
            Arc::clone(&self.failure),
            ctx.clone(),
        );
        std::thread::spawn(move || {
            if let Err(why) = follow.stop_run(&run) {
                *failure.lock().expect("failure") = Some(why);
                ctx.request_repaint();
            }
        });
    }

    /// Draw it over `composer`, `width` wide. True while something moves.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        composer: Rect,
        p: &Palette,
        images: &Images,
        time: f64,
    ) -> bool {
        let height = self.height();
        if height == 0.0 {
            return false;
        }
        let rect = Rect::from_min_size(
            pos2(composer.left(), composer.top() - height),
            vec2(composer.width(), height - 8.0),
        );
        let painter = ui.painter().clone();
        paint::float_shadow(&painter, rect, tokens::RADIUS_CARD, p);
        painter.rect_filled(rect, tokens::RADIUS_CARD, p.surface_raised);
        painter.rect_stroke(
            rect.shrink(0.5),
            tokens::RADIUS_CARD,
            egui::Stroke::new(1.0, p.line),
            egui::StrokeKind::Middle,
        );
        if self.open {
            self.panel(ui, rect, p, images, time)
        } else {
            self.line(ui, rect, p, time)
        }
    }

    /// One line: the running command, where, for how long; Show and Stop.
    fn line(&mut self, ui: &mut Ui, rect: Rect, p: &Palette, time: f64) -> bool {
        let running: Vec<Listed> = self
            .snapshot
            .commands
            .iter()
            .filter(|c| c.running())
            .cloned()
            .collect();
        let Some(first) = running.first() else {
            return false;
        };
        let painter = ui.painter().clone();
        let cy = rect.center().y;
        let dot = pos2(rect.left() + 18.0, cy);
        live_dot(&painter, dot, p, time);
        let mut right = rect.right() - 8.0;
        let show = Rect::from_min_size(pos2(right - 92.0, cy - 13.0), vec2(92.0, 26.0));
        if widgets::text_button(ui, Id::new("runs-show"), show, "Show output", p).clicked() {
            self.show_list();
        }
        right = show.left() - 6.0;
        if running.len() == 1 {
            let stop = Rect::from_min_size(pos2(right - 52.0, cy - 13.0), vec2(52.0, 26.0));
            if widgets::text_button(ui, Id::new("runs-stop-line"), stop, "Stop", p).clicked() {
                let id = first.id().to_string();
                self.stop(&id, ui.ctx());
            }
            right = stop.left() - 6.0;
        }
        let left = dot.x + 12.0;
        let width = (right - left - 8.0).max(0.0);
        let text = if running.len() == 1 {
            format!(
                "{} · {} · {}",
                one_line(&first.summary.command),
                first.place(),
                elapsed(first.summary.status.duration_ms)
            )
        } else {
            format!(
                "{} commands running · {}",
                running.len(),
                running
                    .iter()
                    .map(|c| one_line(&c.summary.command))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let galley = paint::layout(
            &painter,
            paint::line_job(&text, scale::SMALLER, p.ink_muted, width),
        );
        let pos = pos2(left, cy - galley.size().y / 2.0);
        widgets::label(ui, Rect::from_min_size(pos, galley.size()), &text);
        painter.galley(pos, galley, p.ink_muted);
        true
    }

    /// The list, and what the chosen one printed.
    fn panel(&mut self, ui: &mut Ui, rect: Rect, p: &Palette, images: &Images, time: f64) -> bool {
        let painter = ui.painter().clone();
        let mut animating = false;
        // Header: the label, and Hide.
        let label = "COMMANDS ON THIS COMPUTER";
        let galley = paint::layout(
            &painter,
            paint::job(label, scale::MICRO, p.ink_faint, f32::INFINITY),
        );
        let pos = pos2(
            rect.left() + 16.0,
            rect.top() + HEADER / 2.0 - galley.size().y / 2.0 + 2.0,
        );
        widgets::label(
            ui,
            Rect::from_min_size(pos, galley.size()),
            "Commands on this computer",
        );
        painter.galley(pos, galley, p.ink_faint);
        let hide = Rect::from_center_size(
            pos2(rect.right() - 22.0, rect.top() + HEADER / 2.0 + 2.0),
            vec2(28.0, 28.0),
        );
        let response = ui.interact(hide, Id::new("runs-hide"), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Hide commands")
        });
        let t = widgets::hover(ui, Id::new("runs-hide"), response.hovered(), tokens::FAST);
        if t > 0.0 {
            painter.rect_filled(hide, tokens::RADIUS_CONTROL - 2.0, p.ink_wash(6.0 * t));
        }
        images.icon_at(
            &painter,
            hide.center(),
            14.0,
            Icon::X,
            tokens::lerp_rgb(p.ink_muted, p.ink, t),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.hide_list();
        }
        // The rows.
        let chosen = self.chosen().map(|c| c.id().to_string());
        let list = self.snapshot.commands.clone();
        let at = chosen
            .as_deref()
            .and_then(|id| list.iter().position(|c| c.id() == id))
            .unwrap_or(0);
        let first = at.saturating_sub(ROWS - 1);
        let mut y = rect.top() + HEADER;
        let mut choose = None;
        let mut stop = None;
        for command in list.iter().skip(first).take(ROWS) {
            let row =
                Rect::from_min_size(pos2(rect.left() + 6.0, y), vec2(rect.width() - 12.0, ROW));
            let selected = Some(command.id()) == chosen.as_deref();
            let id = Id::new(("runs-row", command.id()));
            let response = ui.interact(row, id, Sense::click());
            let name = one_line(&command.summary.command);
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
            });
            let t = widgets::hover(ui, id, response.hovered(), tokens::FAST);
            if selected {
                painter.rect_filled(row, tokens::RADIUS_CONTROL - 2.0, p.surface_selected);
            } else if t > 0.0 {
                painter.rect_filled(row, tokens::RADIUS_CONTROL - 2.0, p.ink_wash(6.0 * t));
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                choose = Some(command.id().to_string());
            }
            let cy = row.center().y;
            let dot = pos2(row.left() + 12.0, cy);
            if command.running() {
                live_dot(&painter, dot, p, time);
                animating = true;
            } else {
                painter.circle_filled(dot, 3.0, state_color(command, p));
            }
            let mut right = row.right() - 8.0;
            if command.running() {
                let button = Rect::from_min_size(pos2(right - 52.0, cy - 12.0), vec2(52.0, 24.0));
                let button_id = Id::new(("runs-stop", command.id()));
                if widgets::text_button(ui, button_id, button, "Stop", p).clicked() {
                    stop = Some(command.id().to_string());
                }
                right = button.left() - 10.0;
            }
            let state = if command.running() {
                elapsed(command.summary.status.duration_ms)
            } else {
                command.state_label()
            };
            let state_galley = paint::layout(
                &painter,
                paint::job(&state, scale::MICRO, state_ink(command, p), f32::INFINITY),
            );
            let state_pos = pos2(
                right - state_galley.size().x,
                cy - state_galley.size().y / 2.0,
            );
            painter.galley(state_pos, state_galley, p.ink_muted);
            let left = dot.x + 12.0;
            let width = (state_pos.x - left - 12.0).max(0.0);
            let ink = if selected { p.ink } else { p.ink_muted };
            let galley = paint::layout(
                &painter,
                paint::line_job(&name, tokens::scale::CODE.weight(400.0), ink, width),
            );
            painter.galley(pos2(left, cy - galley.size().y / 2.0), galley, ink);
            y += ROW;
        }
        if let Some(id) = choose {
            self.choose(Some(id));
        }
        if let Some(id) = stop {
            self.stop(&id, ui.ctx());
        }
        // What the chosen one printed, in a well.
        let well = Rect::from_min_size(
            pos2(rect.left() + PAD, y + 8.0),
            vec2(rect.width() - PAD * 2.0, OUTPUT),
        );
        painter.rect_filled(well, tokens::RADIUS_CONTROL, p.canvas_sunken);
        painter.rect_stroke(
            well.shrink(0.5),
            tokens::RADIUS_CONTROL,
            egui::Stroke::new(1.0, p.line),
            egui::StrokeKind::Middle,
        );
        if let Some(chosen) = self.chosen().cloned() {
            self.output(ui, well, &chosen, p);
        }
        animating
    }

    /// The chosen command's line, where it runs, and the end of what it
    /// printed.
    fn output(&self, ui: &mut Ui, well: Rect, chosen: &Listed, p: &Palette) {
        let painter = ui.painter().clone();
        let inner = well.shrink2(vec2(12.0, 10.0));
        let mut about = format!("{} · {}", chosen.place(), chosen.state_label());
        if chosen.summary.network {
            about.push_str(" · with network");
        }
        let header = paint::layout(
            &painter,
            paint::line_job(&about, scale::MICRO, state_ink(chosen, p), inner.width()),
        );
        widgets::label(ui, Rect::from_min_size(inner.min, header.size()), &about);
        let top = inner.top() + header.size().y + 8.0;
        painter.galley(inner.min, header, p.ink_muted);
        let printed = self
            .snapshot
            .printed
            .as_ref()
            .filter(|printed| printed.id == chosen.id());
        let text = match printed {
            Some(printed) if !printed.text.is_empty() => clean(&printed.text),
            Some(printed) if printed.done => "It printed nothing.".to_string(),
            _ => "Nothing printed yet.".to_string(),
        };
        let quiet = printed.is_none_or(|p| p.text.is_empty());
        let rows = ((inner.bottom() - top) / OUTPUT_TYPE.line_height)
            .floor()
            .max(1.0) as usize;
        let lines: Vec<&str> = text.lines().collect();
        let shown = lines[lines.len().saturating_sub(rows)..].join("\n");
        let color = if quiet { p.ink_faint } else { p.ink };
        let mut job = paint::job(&shown, OUTPUT_TYPE, color, inner.width());
        job.wrap.max_rows = rows;
        job.wrap.break_anywhere = true;
        let galley = paint::layout(&painter, job);
        // The end shows: the latest line at the bottom.
        let y = (inner.bottom() - galley.size().y).max(top);
        let clip = Rect::from_min_max(pos2(inner.left(), top), inner.max);
        widgets::label(ui, clip, &shown);
        painter
            .with_clip_rect(clip)
            .galley(pos2(inner.left(), y), galley, color);
    }
}

/// A pulsing live dot.
fn live_dot(painter: &egui::Painter, center: egui::Pos2, p: &Palette, time: f64) {
    let phase = (time / 2.0).fract() as f32;
    paint::pulse_halo(painter, center, 3.0, p.live(), phase);
    painter.circle_filled(center, 3.0, p.live());
}

fn state_color(command: &Listed, p: &Palette) -> egui::Color32 {
    if command.succeeded() {
        p.positive()
    } else if command.summary.status.state == "stopped" && command.summary.status.limit.is_none() {
        p.ink_faint
    } else {
        p.negative()
    }
}

fn state_ink(command: &Listed, p: &Palette) -> egui::Color32 {
    if command.running() || command.succeeded() {
        p.ink_muted
    } else if command.summary.status.state == "stopped" && command.summary.status.limit.is_none() {
        p.ink_faint
    } else {
        p.negative_ink()
    }
}
