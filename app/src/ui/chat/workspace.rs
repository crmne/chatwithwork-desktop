//! The native new-chat surface, from the web's `new_chat.css` and its API.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Unique across closed and reopened native screens.
pub fn request_id() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

use cww::tui::chat::Failure;
use cww::workspace::{Instruction, Layer, NewChat, SavedInstruction, Service, ServiceKind};
use egui::{Galley, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::controls;
use super::icons::{Icon, Images};
use super::markdown::{Inline, Style};
use super::paint;
use super::prose::{Clicked, Prose};
use super::tokens::{Palette, Type, scale};
use super::widgets;

#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    pub instruction: Instruction,
    pub body: String,
    pub counted_body: String,
    pub tokens: usize,
    pub changed_at: Option<f64>,
    pub counting: Option<String>,
    pub count_error: Option<String>,
    pub history: bool,
    pub expanded: std::collections::HashSet<u64>,
}

impl Editor {
    fn new(instruction: Instruction) -> Self {
        Self {
            body: instruction.body.clone(),
            counted_body: instruction.body.clone(),
            tokens: instruction.tokens,
            instruction,
            changed_at: None,
            counting: None,
            count_error: None,
            history: false,
            expanded: Default::default(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.body != self.instruction.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instructions {
    pub account: Option<String>,
    pub context: Option<cww::workspace::InstructionContext>,
    pub templates: Option<super::templates::Screen>,
    pub project: Option<u64>,
    pub back_to_project: bool,
    pub generation: u64,
    pub memory: Option<cww::workspace::Memory>,
    pub editors: Vec<Editor>,
    pub focus: Layer,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub scroll_to: Option<Layer>,
    pub confirm_close: bool,
}

impl Instructions {
    pub fn new(focus: Layer) -> Self {
        Self {
            generation: request_id(),
            account: None,
            context: None,
            templates: None,
            project: None,
            back_to_project: false,
            memory: None,
            editors: vec![],
            focus,
            busy: true,
            error: None,
            notice: None,
            scroll_to: Some(focus),
            confirm_close: false,
        }
    }

    pub fn loaded(
        &mut self,
        result: Result<cww::workspace::InstructionPage, Failure>,
    ) -> Vec<super::state::Command> {
        self.busy = false;
        match result {
            Ok(page) => {
                if page.context.as_ref().is_some_and(|context| {
                    self.account
                        .as_ref()
                        .is_some_and(|account| context.account.id != *account)
                        || context.project.as_ref().map(|p| p.id) != self.project
                }) {
                    self.editors.clear();
                    self.memory = None;
                    self.templates = None;
                    self.context = None;
                    self.error = Some(
                        "Your organization or project changed. Open instructions again.".into(),
                    );
                    return vec![];
                }
                self.editors = page.instructions.into_iter().map(Editor::new).collect();
                self.memory = Some(page.memory);
                self.context = page.context;
                self.templates = None;
                self.error = None;
                if let Some(context) = &self.context
                    && let Some(project) = &context.project
                {
                    let mut screen = super::templates::Screen::new(
                        context.account.id.clone(),
                        Some(project.number),
                    );
                    let commands = screen.load();
                    self.templates = Some(screen);
                    return commands;
                }
            }
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "forbidden"
                        | "not_found"
                        | "invalid_token"
                        | "revoked"
                        | "chat_access_required"
                        | "not_paired"
                ) {
                    self.editors.clear();
                    self.memory = None;
                    self.templates = None;
                    self.context = None;
                }
                self.error = Some(error.message);
            }
        }
        vec![]
    }

    pub fn working(&self) -> bool {
        self.busy || self.templates.as_ref().is_some_and(|p| p.busy)
    }

    pub fn saved(&mut self, result: Result<SavedInstruction, Failure>) {
        self.busy = false;
        match result {
            Ok(SavedInstruction {
                instruction,
                notice,
            }) => {
                if let Some(editor) = self
                    .editors
                    .iter_mut()
                    .find(|editor| editor.instruction.layer == instruction.layer)
                {
                    *editor = Editor::new(instruction);
                }
                self.error = None;
                self.notice = Some(notice);
            }
            Err(error) => self.error = Some(error.message),
        }
    }

    pub fn counted(&mut self, layer: Layer, body: String, result: Result<usize, Failure>) {
        if let Some(editor) = self
            .editors
            .iter_mut()
            .find(|e| e.instruction.layer == layer)
        {
            if editor.counting.as_ref() == Some(&body) {
                editor.counting = None;
            }
            if editor.body == body {
                match result {
                    Ok(tokens) => {
                        editor.tokens = tokens;
                        editor.counted_body = body;
                        editor.count_error = None;
                    }
                    Err(failure) => editor.count_error = Some(failure.message),
                }
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.editors.iter().any(Editor::dirty)
    }
}

pub enum Event {
    Connect,
    Pause(Service, bool),
    Remove(Service),
    Ask(String),
    Instructions(Layer),
}

pub struct StartLayout {
    suggestions: Vec<(usize, Rect, Arc<Galley>)>,
    instructions: Vec<Inline>,
    instructions_y: f32,
    pub height: f32,
}

impl StartLayout {
    pub fn new(ui: &Ui, width: f32, data: &NewChat, p: &Palette) -> Self {
        let mut suggestions = Vec::new();
        let mut row = Vec::new();
        let mut used = 0.0;
        let mut y = 4.0 + 44.0 + 58.0 + 32.0;
        let mut row_height: f32 = 0.0;
        let flush = |row: &mut Vec<(usize, Rect, Arc<Galley>)>, used: f32, output: &mut Vec<_>| {
            let shift = (width - used.max(0.0)) / 2.0;
            output.extend(
                row.drain(..)
                    .map(|(index, rect, text)| (index, rect.translate(vec2(shift, 0.0)), text)),
            );
        };
        for (index, suggestion) in data.suggestions.iter().enumerate() {
            let text = paint::layout(
                ui.painter(),
                paint::job(
                    &suggestion.label,
                    Type::sans(13.0, 16.9),
                    p.ink_muted,
                    (width - 46.0).max(1.0),
                ),
            );
            let size = text.size() + vec2(46.0, 14.0);
            if used > 0.0 && used + 8.0 + size.x > width {
                flush(&mut row, used, &mut suggestions);
                y += row_height + 8.0;
                used = 0.0;
                row_height = 0.0;
            }
            let x = if used == 0.0 { 0.0 } else { used + 8.0 };
            row.push((index, Rect::from_min_size(pos2(x, y), size), text));
            used = x + size.x;
            row_height = row_height.max(size.y);
        }
        flush(&mut row, used, &mut suggestions);
        y += row_height + 24.0;
        let instructions = instruction_line(data);
        let text = super::markdown::plain(&instructions);
        let height = paint::layout(
            ui.painter(),
            paint::job(
                &text,
                Type::sans(13.0, 19.5),
                p.ink_faint,
                width.min(544.0) - 24.0,
            ),
        )
        .size()
        .y;
        Self {
            suggestions,
            instructions,
            instructions_y: y,
            height: y + height,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &self,
        ui: &mut Ui,
        rect: Rect,
        data: &NewChat,
        menu: &mut Option<usize>,
        active: bool,
        busy: bool,
        p: &Palette,
        images: &Images,
    ) -> Option<Event> {
        let mut event = None;
        let count = data.services.len() + 1;
        let width = rect.width().min(count as f32 * 112.0);
        let left = rect.center().x - width / 2.0;
        let top = rect.top() + 4.0;
        for index in 0..count {
            let service = data.services.get(index);
            let x = left + (index as f32 + 0.5) * width / count as f32;
            let color = [p.green, p.blue, p.violet, p.orange, p.lime][index % 5];
            let lit = active && service.is_some_and(|s| s.live && s.kind != ServiceKind::Mcp);
            let wire = egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    pos2(rect.center().x, top),
                    pos2(rect.center().x, top + 26.4),
                    pos2(x, top + 17.6),
                    pos2(x, top + 44.0),
                ],
                false,
                egui::Color32::TRANSPARENT,
                Stroke::new(
                    if lit { 2.0 } else { 1.0 },
                    if lit { color } else { p.line_strong },
                ),
            );
            ui.painter().add(wire);
            let center = pos2(x, top + 62.0);
            let hit = Rect::from_center_size(
                pos2(x, top + 73.0),
                vec2((width / count as f32).max(24.0), 58.0),
            );
            let id = Id::new(("reach", index));
            let response = ui.interact(hit, id, Sense::click());
            let label = service.map_or("Connect a service".to_string(), |s| {
                format!("{}, {}", s.name, s.label.to_lowercase())
            });
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            let opacity = if service.is_some_and(|s| !s.live) && !response.hovered() {
                0.5
            } else {
                1.0
            };
            let avatar = Rect::from_center_size(center, vec2(36.0, 36.0));
            ui.painter()
                .circle_filled(center, 18.0, p.surface.gamma_multiply(opacity));
            ui.painter().circle_stroke(
                center,
                18.0,
                Stroke::new(
                    1.0,
                    if response.hovered() {
                        p.ink_muted
                    } else {
                        p.line_strong
                    }
                    .gamma_multiply(opacity),
                ),
            );
            if response.has_focus() {
                widgets::focus_ring(ui, avatar, 18.0, p);
            }
            let icon = service.map_or(Icon::Plus, |s| {
                if s.kind == ServiceKind::Computer {
                    Icon::Laptop
                } else {
                    Icon::PlugsConnected
                }
            });
            if !images.logos.draw(
                ui.painter(),
                service.and_then(|s| s.icon.as_ref()),
                avatar.shrink(9.0),
                0.0,
                p,
                opacity,
            ) {
                images.icon_at(
                    ui.painter(),
                    center,
                    18.0,
                    icon,
                    p.ink_muted.gamma_multiply(opacity),
                );
            }
            if service.is_some_and(|s| s.paused) {
                let badge = center + vec2(14.0, 14.0);
                ui.painter().circle_filled(badge, 7.0, p.surface_raised);
                ui.painter()
                    .circle_stroke(badge, 7.0, Stroke::new(1.0, p.line_strong));
                images.icon_at(ui.painter(), badge, 8.0, Icon::Pause, p.ink);
            }
            let name = service.map_or("Connect", |s| s.name.as_str());
            let text = paint::layout(
                ui.painter(),
                paint::line_job(
                    name,
                    scale::MICRO,
                    p.ink_muted.gamma_multiply(opacity),
                    hit.width() - 4.0,
                ),
            );
            ui.painter()
                .galley(pos2(x - text.size().x / 2.0, top + 87.0), text, p.ink_muted);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                if service.is_some() {
                    *menu = if *menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                } else {
                    event = Some(Event::Connect);
                }
            }
            if *menu == Some(index)
                && let Some(service) = service
            {
                let panel = Rect::from_min_size(
                    pos2(
                        (x - 90.0).clamp(rect.left(), (rect.right() - 180.0).max(rect.left())),
                        top + 112.0,
                    ),
                    vec2(180.0, if service.removable { 106.0 } else { 70.0 }),
                );
                egui::Area::new(id.with("menu"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(panel.min)
                    .show(ui.ctx(), |ui| {
                        if busy {
                            ui.disable();
                        }
                        controls::menu_panel(ui.painter(), panel, p);
                        let text = paint::layout(
                            ui.painter(),
                            paint::line_job(&service.label, scale::MICRO, p.ink_faint, 156.0),
                        );
                        ui.painter()
                            .galley(panel.min + vec2(12.0, 8.0), text, p.ink_faint);
                        if controls::menu_item(
                            ui,
                            id.with("pause"),
                            Rect::from_min_size(panel.min + vec2(4.0, 30.0), vec2(172.0, 32.0)),
                            Some(if service.paused {
                                Icon::Play
                            } else {
                                Icon::Pause
                            }),
                            if service.paused { "Resume" } else { "Pause" },
                            false,
                            false,
                            p,
                            images,
                        )
                        .clicked()
                        {
                            event = Some(Event::Pause(service.clone(), !service.paused));
                            *menu = None;
                        }
                        if service.removable
                            && controls::menu_item(
                                ui,
                                id.with("remove"),
                                Rect::from_min_size(panel.min + vec2(4.0, 66.0), vec2(172.0, 32.0)),
                                Some(Icon::X),
                                "Remove…",
                                true,
                                false,
                                p,
                                images,
                            )
                            .clicked()
                        {
                            event = Some(Event::Remove(service.clone()));
                            *menu = None;
                        }
                    });
                if ui.input(|i| {
                    i.key_pressed(egui::Key::Escape)
                        || (i.pointer.any_pressed()
                            && i.pointer
                                .interact_pos()
                                .is_some_and(|pos| !panel.contains(pos) && !hit.contains(pos)))
                }) {
                    *menu = None;
                }
            }
        }
        for (index, bounds, text) in &self.suggestions {
            let bounds = bounds.translate(rect.min.to_vec2());
            let response = ui.interact(bounds, Id::new(("suggestion", index)), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    &data.suggestions[*index].label,
                )
            });
            ui.painter().rect_filled(
                bounds,
                bounds.height() / 2.0,
                if response.hovered() {
                    p.surface
                } else {
                    p.canvas
                },
            );
            ui.painter().rect_stroke(
                bounds.shrink(0.5),
                bounds.height() / 2.0,
                Stroke::new(
                    1.0,
                    if response.hovered() {
                        p.line_strong
                    } else {
                        p.line
                    },
                ),
                egui::StrokeKind::Middle,
            );
            images.icon_at(
                ui.painter(),
                pos2(bounds.left() + 18.0, bounds.center().y),
                14.0,
                Icon::ArrowUpRight,
                p.ink_faint,
            );
            ui.painter()
                .galley(bounds.min + vec2(31.0, 7.0), Arc::clone(text), p.ink_muted);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                event = Some(Event::Ask(data.suggestions[*index].prompt.clone()));
            }
        }
        let line_width = rect.width().min(544.0);
        let line_left = rect.center().x - line_width / 2.0;
        images.icon_at(
            ui.painter(),
            pos2(line_left + 7.0, rect.top() + self.instructions_y + 10.0),
            14.0,
            Icon::Notebook,
            p.ink_faint,
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
            pos2(line_left + 24.0, rect.top() + self.instructions_y),
            pos2(line_left + line_width, rect.bottom()),
        )));
        let prose = Prose {
            palette: p,
            images,
            sources: &[],
            caret: false,
            time: 0.0,
            id: Id::new("start-instructions"),
        };
        if let Some(Clicked::Link(layer)) = prose.centered_text(
            &mut child,
            &self.instructions,
            Type::sans(13.0, 19.5),
            p.ink_faint,
        ) {
            let layer = match layer.as_str() {
                "organization" => Layer::Organization,
                "project" => Layer::Project,
                _ => Layer::Personal,
            };
            event = Some(Event::Instructions(layer));
        }
        event
    }
}

fn instruction_line(data: &NewChat) -> Vec<Inline> {
    let text = |text: String, link: Option<String>| Inline::Text {
        text,
        style: Style::default(),
        link,
    };
    let personal_written = data
        .instructions
        .iter()
        .any(|i| i.layer == Layer::Personal && i.written);
    let written: Vec<_> = data.instructions.iter().filter(|i| i.written).collect();
    let mut runs = Vec::new();
    if !written.is_empty() {
        runs.push(text("Starts with ".into(), None));
        for (index, instruction) in written.iter().enumerate() {
            if index > 0 {
                runs.push(text(
                    if index + 1 == written.len() {
                        " and "
                    } else {
                        ", "
                    }
                    .into(),
                    None,
                ));
            }
            let label = if instruction.layer == Layer::Personal {
                "your instructions".into()
            } else if personal_written {
                format!("{}'s", instruction.owner)
            } else {
                format!("{}'s instructions", instruction.owner)
            };
            runs.push(text(label, Some(instruction.layer.path().into())));
        }
        runs.push(text(". ".into(), None));
    }
    if !personal_written {
        runs.push(text(
            "Tell the assistant who you are and how you like answers, once: ".into(),
            None,
        ));
        runs.push(text(
            "write your instructions".into(),
            Some("personal".into()),
        ));
        runs.push(text(".".into(), None));
    }
    runs
}
