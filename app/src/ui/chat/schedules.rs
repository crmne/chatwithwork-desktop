//! Personal schedule pages. The server owns recurrence, access and run rules.
use super::{
    controls::{self, Button, FieldLook, Kind, Size},
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    state::Command,
    tokens::{Palette, Type, scale},
    workspace::request_id,
};
use cww::{
    schedules::{Fields, Page, Reply, Request, Schedule},
    tui::chat::{Failure, SettingsLink},
};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    account: String,
    pub page: Option<Page>,
    pub draft: Option<Fields>,
    pub busy: bool,
    pub error: Option<String>,
    pub confirm_close: bool,
    pub deleting: Option<Schedule>,
    pending: Option<Request>,
    refresh: bool,
}
impl Screen {
    pub fn new(account: String) -> Self {
        Self {
            generation: request_id(),
            account,
            page: None,
            draft: None,
            busy: false,
            error: None,
            confirm_close: false,
            deleting: None,
            pending: None,
            refresh: false,
        }
    }
    pub fn dirty(&self) -> bool {
        self.page
            .as_ref()
            .and_then(|p| p.form.as_ref())
            .zip(self.draft.as_ref())
            .is_some_and(|(form, draft)| &form.fields != draft)
    }
    pub fn request(&mut self, request: Request) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        self.pending = Some(request.clone());
        vec![Command::Schedules(self.generation, request)]
    }
    pub fn invalidate(&mut self) -> Vec<Command> {
        if self.busy {
            self.refresh = true;
            return vec![];
        }
        // Keep an in-progress form, including values not saved yet. Saving
        // rechecks project access, limits and timing on the server.
        if self.draft.is_some() {
            return vec![];
        }
        self.request(Request::List)
    }
    pub fn save(&mut self) -> Vec<Command> {
        let Some(form) = self.page.as_ref().and_then(|p| p.form.as_ref()) else {
            return vec![];
        };
        let Some(fields) = self.draft.clone() else {
            return vec![];
        };
        if fields.name.trim().is_empty() || fields.prompt.trim().is_empty() {
            self.error = Some("Give the schedule a name and write what to ask.".into());
            return vec![];
        }
        let account = self.account.clone();
        self.request(match form.id {
            Some(id) => Request::Update {
                account,
                id,
                fields,
            },
            None => Request::Create { account, fields },
        })
    }
    pub fn loaded(&mut self, result: Result<Reply, Failure>) -> (Vec<Command>, Option<u64>) {
        self.busy = false;
        let pending = self.pending.take();
        match result {
            Ok(Reply::Page(page))
                if page.account.id == self.account
                    && match pending {
                        Some(Request::New) => {
                            page.form.as_ref().is_some_and(|form| form.id.is_none())
                        }
                        Some(Request::Edit { id }) => {
                            page.form.as_ref().is_some_and(|form| form.id == Some(id))
                        }
                        _ => page.form.is_none(),
                    } =>
            {
                self.draft = page.form.as_ref().map(|form| form.fields.clone());
                self.page = Some(*page);
                self.error = None;
                self.deleting = None;
                if std::mem::take(&mut self.refresh) && self.draft.is_none() {
                    return (self.request(Request::List), None);
                }
            }
            Ok(Reply::Run { account, chat })
                if account.id == self.account && matches!(pending, Some(Request::Run { .. })) =>
            {
                return (vec![], Some(chat));
            }
            Ok(_) => {
                self.clear();
                self.error =
                    Some("Your organization or schedule changed. Open schedules again.".into());
            }
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "forbidden"
                        | "invalid_token"
                        | "revoked"
                        | "chat_access_required"
                        | "not_paired"
                        | "stale_account"
                        | "not_found"
                ) {
                    self.clear();
                }
                self.error = Some(error.message);
            }
        }
        (vec![], None)
    }
    fn clear(&mut self) {
        self.page = None;
        self.draft = None;
        self.deleting = None;
        self.refresh = false;
    }
}
#[derive(Default)]
pub struct Outcome {
    pub commands: Vec<Command>,
    pub event: Option<Event>,
    pub discarded: bool,
}
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    screen: &mut Screen,
    tabs: &[SettingsLink],
    p: &Palette,
    images: &Images,
) -> Outcome {
    let mut out = Outcome::default();
    let header = screen
        .page
        .as_ref()
        .map(|page| format!("{} · {}", page.email, page.account.name));
    if screen.draft.is_some() {
        // Forms use the website's centered page layout rather than Settings' nav.
        let width = (rect.width() - 32.0).clamp(1.0, 768.0);
        let bounds = Rect::from_min_max(
            pos2(rect.center().x - width / 2.0, rect.top()),
            pos2(rect.center().x + width / 2.0, rect.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
        child.set_clip_rect(bounds);
        egui::ScrollArea::vertical()
            .id_salt(("schedule-form", screen.generation))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(if rect.width() >= 768.0 { 72.0 } else { 48.0 });
                form(ui, screen, p, images, &mut out);
                ui.add_space(64.0);
            });
    } else {
        let event = super::settings::Shell {
            rect,
            generation: screen.generation,
            header: header.as_deref(),
            active: "schedules",
            tabs,
            busy: screen.busy,
            palette: p,
            images,
        }
        .show(ui, |ui| list(ui, screen, p, images, &mut out));
        if event.is_some() {
            out.event = event;
        }
    }
    if screen.confirm_close {
        let (keep, discard) = confirmation(
            ui,
            "discard-schedule",
            "Discard schedule changes?",
            "Your schedule has not been saved.",
            "Discard changes",
            p,
            images,
        );
        if keep {
            screen.confirm_close = false;
        }
        out.discarded = discard;
    }
    if let Some(schedule) = screen.deleting.clone() {
        let (keep, remove) = confirmation(
            ui,
            "delete-schedule",
            &format!("Delete {}?", schedule.name),
            "The chats it ran stay.",
            "Delete schedule",
            p,
            images,
        );
        if keep {
            screen.deleting = None;
        }
        if remove {
            screen.deleting = None;
            out.commands.extend(screen.request(Request::Delete {
                account: screen.account.clone(),
                id: schedule.id,
            }));
        }
    }
    out
}
fn list(ui: &mut Ui, screen: &mut Screen, p: &Palette, images: &Images, out: &mut Outcome) {
    ui.horizontal(|ui| {
        heading(ui, "Schedules", p);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(page) = &screen.page
                && !page.schedules.is_empty()
            {
                text(
                    ui,
                    &format!("{} of {} active", page.active_count, page.limit),
                    scale::MICRO,
                    p.ink_faint,
                );
            }
        });
    });
    ui.add_space(8.0);
    text(
        ui,
        "Each run starts a new chat as you, with your connectors at their access levels. It spends credits like any chat, asks before any change, and emails you when it waits for you.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    status(ui, screen, p);
    let Some(page) = screen.page.clone() else {
        text(
            ui,
            if screen.busy {
                "Loading schedules…"
            } else {
                "Schedules could not be loaded."
            },
            scale::SMALL,
            p.ink_muted,
        );
        if !screen.busy && button(ui, "Retry", Kind::Subtle, true, p, images) {
            out.commands.extend(screen.request(Request::List));
        }
        return;
    };
    if page.schedules.is_empty() {
        egui::Frame::new().inner_margin(32.0).show(ui, |ui| {
            heading(ui, "No schedules yet", p);
            text(
                ui,
                "Ask the assistant to do something on a schedule, or add one here.",
                scale::SMALL,
                p.ink_muted,
            );
        });
    } else {
        group(ui, p, |ui| {
            for (index, schedule) in page.schedules.iter().enumerate() {
                if index > 0 {
                    separator(ui, p);
                }
                ui.push_id(schedule.id, |ui| {
                    egui::Frame::new()
                        .inner_margin(16.0)
                        .fill(if schedule.attention {
                            p.attention_wash()
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .show(ui, |ui| {
                            let wide = ui.available_width() >= 640.0;
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 12.0;
                                let (icon, _) =
                                    ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                                ui.painter().rect_filled(icon, 10.0, p.canvas);
                                ui.painter().rect_stroke(
                                    icon,
                                    10.0,
                                    Stroke::new(1.0, p.line),
                                    egui::StrokeKind::Inside,
                                );
                                images.icon_at(
                                    ui.painter(),
                                    icon.center(),
                                    20.0,
                                    Icon::Clock,
                                    p.ink_muted,
                                );
                                if wide {
                                    let actions_width = if schedule.paused { 254.0 } else { 210.0 };
                                    let width =
                                        (ui.available_width() - actions_width - 12.0).max(80.0);
                                    ui.allocate_ui_with_layout(
                                        vec2(width, 0.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_min_width(width);
                                            schedule_body(ui, schedule, p, out);
                                        },
                                    );
                                    ui.vertical(|ui| {
                                        schedule_actions(ui, schedule, screen, p, images, out)
                                    });
                                } else {
                                    ui.vertical(|ui| {
                                        schedule_body(ui, schedule, p, out);
                                        ui.add_space(12.0);
                                        schedule_actions(ui, schedule, screen, p, images, out);
                                    });
                                }
                            });
                        });
                });
            }
        });
    }
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if button(ui, "Add schedule", Kind::Subtle, !screen.busy, p, images) {
                out.event = Some(Event::Schedules(Request::New));
            }
        });
    });
}
fn schedule_body(ui: &mut Ui, schedule: &Schedule, p: &Palette, out: &mut Outcome) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        text(ui, &schedule.name, scale::SMALL.weight(550.0), p.ink);
        let color = match schedule.tone.as_deref() {
            Some("attention") => p.attention_ink,
            Some("positive") => p.green_ink,
            _ => p.ink_muted,
        };
        let (dot, _) = ui.allocate_exact_size(vec2(5.0, 18.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 2.5, color);
        text(ui, &schedule.state, Type::sans(12.0, 18.0), color);
    });
    text(
        ui,
        &format!("{} · {}", schedule.timing, schedule.location),
        scale::SMALL,
        p.ink_muted,
    );
    let next = schedule
        .next_run
        .as_ref()
        .map(|v| format!("Next: {v}"))
        .unwrap_or_else(|| schedule.time_zone.clone());
    text(ui, &next, scale::MICRO, p.ink_faint);
    if let Some(last) = &schedule.last_run {
        let label = format!("Last run {}", last.time);
        let response = ui
            .add(
                egui::Label::new(paint::job(
                    &label,
                    scale::MICRO,
                    p.ink_faint,
                    ui.available_width(),
                ))
                .wrap()
                .sense(Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            out.event = Some(Event::Open(last.chat));
        }
    }
    if let Some(skipped) = &schedule.skipped {
        text(
            ui,
            &format!("Last skipped: {skipped}"),
            scale::MICRO,
            p.ink_faint,
        );
    }
}
fn schedule_actions(
    ui: &mut Ui,
    schedule: &Schedule,
    screen: &mut Screen,
    p: &Palette,
    images: &Images,
    out: &mut Outcome,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if button(ui, "Run now", Kind::Subtle, !screen.busy, p, images) {
            out.commands.extend(screen.request(Request::Run {
                account: screen.account.clone(),
                id: schedule.id,
            }));
        }
        if schedule.paused && button(ui, "Resume", Kind::Subtle, !screen.busy, p, images) {
            out.commands.extend(screen.request(Request::Resume {
                account: screen.account.clone(),
                id: schedule.id,
            }));
        }
        if schedule.active
            && icon_action(
                ui,
                Icon::Pause,
                &format!("Pause {}", schedule.name),
                !screen.busy,
                p,
                images,
            )
        {
            out.commands.extend(screen.request(Request::Pause {
                account: screen.account.clone(),
                id: schedule.id,
            }));
        }
        if button(ui, "Edit", Kind::Subtle, !screen.busy, p, images) {
            out.event = Some(Event::Schedules(Request::Edit { id: schedule.id }));
        }
        if icon_action(
            ui,
            Icon::Trash,
            &format!("Delete {}", schedule.name),
            !screen.busy,
            p,
            images,
        ) {
            screen.deleting = Some(schedule.clone());
        }
    });
}
fn icon_action(
    ui: &mut Ui,
    icon: Icon,
    name: &str,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
    super::widgets::IconButton {
        icon,
        icon_size: 16.0,
        label: name,
        tooltip: Some(name),
        color: p.ink_muted,
        hover_color: if icon == Icon::Trash {
            p.negative_ink()
        } else {
            p.ink
        },
        wash: 4.0,
        radius: 8.0,
        enabled,
    }
    .show(ui, ui.make_persistent_id(name), rect, images, p)
    .clicked()
}
fn form(ui: &mut Ui, screen: &mut Screen, p: &Palette, images: &Images, out: &mut Outcome) {
    let Some(form) = screen.page.as_ref().and_then(|page| page.form.clone()) else {
        return;
    };
    let editing = form.id.is_some();
    text(
        ui,
        if editing {
            "Schedule"
        } else {
            &screen.page.as_ref().unwrap().account.name
        },
        scale::MICRO,
        p.ink_faint,
    );
    ui.add_space(8.0);
    text(
        ui,
        if editing {
            &form.fields.name
        } else {
            "New schedule"
        },
        Type::sans(24.0, 32.0).weight(600.0),
        p.ink,
    );
    ui.add_space(8.0);
    text(
        ui,
        if editing {
            "Changes apply from its next run. The chats it already ran stay as they are."
        } else {
            "Ask something on a schedule, like a morning briefing. Each run starts a new chat as you."
        },
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(32.0);
    status(ui, screen, p);
    let Some(draft) = &mut screen.draft else {
        return;
    };
    ui.add_enabled_ui(!screen.busy, |ui| {
        group(ui, p, |ui| {
            field(ui, "Name", &mut draft.name, false, "e.g. Morning briefing", None, screen.generation, p);
            separator(ui, p);
            let placeholder = "e.g. Summarize what changed in the Q4 folder since yesterday, and what's on my calendar today.";
            let hint = "Each run starts a new chat without this page's context, so say what to look at and when.";
            field(ui, "Ask", &mut draft.prompt, true, placeholder, Some(hint), screen.generation, p);
        });
        ui.add_space(14.0);
        group(ui, p, |ui| {
            row(ui, "When", p, |ui| repeats(ui, &mut draft.repeats, screen.generation, p));
            separator(ui, p);
            if draft.repeats == "once" {
                field(ui, "Date and time", &mut draft.run_at, false, "YYYY-MM-DDTHH:MM", None, screen.generation, p);
            } else {
                select(ui, "Every", &mut draft.frequency, &[("day", "Day"), ("weekday", "Weekday"), ("week", "Week, on"), ("month", "Month, on the"), ("custom", "Custom")], screen.generation, p);
                separator(ui, p);
                if draft.frequency == "week" {
                    select(ui, "Day", &mut draft.on_weekday, &[("monday", "Monday"), ("tuesday", "Tuesday"), ("wednesday", "Wednesday"), ("thursday", "Thursday"), ("friday", "Friday"), ("saturday", "Saturday"), ("sunday", "Sunday")], screen.generation, p);
                    separator(ui, p);
                }
                if draft.frequency == "month" {
                    field(ui, "Day of the month", &mut draft.on_day, false, "1–31", None, screen.generation, p);
                    separator(ui, p);
                }
                if draft.frequency == "custom" {
                    let hint = "In plain words, like “every monday at 9:00”, or a cron line. At most once an hour.";
                    field(ui, "Custom", &mut draft.recurrence, false, "e.g. every 2 hours, or 0 9 * * 1", Some(hint), screen.generation, p);
                } else {
                    field(ui, "At", &mut draft.at, false, "08:30", None, screen.generation, p);
                }
            }
            separator(ui, p);
            let zones: Vec<(&str, &str)> = form.time_zones.iter().map(|zone| (zone.as_str(), zone.as_str())).collect();
            select(ui, "Time zone", &mut draft.time_zone, &zones, screen.generation, p);
        });
        ui.add_space(14.0);
        group(ui, p, |ui| {
            choice(ui, "Where", &mut draft.project_id, &form.projects, "Private, only you see the runs", screen.generation, p);
            separator(ui, p);
            choice(ui, "Model", &mut draft.model_setting_id, &form.models, "Your default model", screen.generation, p);
        });
    });
    ui.add_space(16.0);
    text(
        ui,
        "Each run starts a new chat as you, spends credits, and asks before any change.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if button(ui, "Cancel", Kind::Subtle, !screen.busy, p, images) {
            out.event = Some(Event::Schedules(Request::List));
        }
        if button(
            ui,
            if form.id.is_some() {
                "Save schedule"
            } else {
                "Create schedule"
            },
            Kind::Primary,
            !screen.busy,
            p,
            images,
        ) {
            out.commands.extend(screen.save());
        }
    });
}
fn status(ui: &mut Ui, screen: &Screen, p: &Palette) {
    if let Some(error) = &screen.error {
        text(ui, error, scale::SMALL, p.negative_ink());
        ui.add_space(16.0);
    }
    if let Some(notice) = screen.page.as_ref().and_then(|page| page.notice.as_ref()) {
        text(ui, notice, scale::SMALL, p.ink);
        ui.add_space(16.0);
    }
}
fn text(ui: &mut Ui, label: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(label, ty, color, ui.available_width())).wrap());
}
fn heading(ui: &mut Ui, label: &str, p: &Palette) {
    text(ui, label, Type::sans(15.0, 22.5).weight(600.0), p.ink);
}
fn group(ui: &mut Ui, p: &Palette, content: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}
fn separator(ui: &mut Ui, p: &Palette) {
    ui.painter().hline(
        ui.cursor().left()..=ui.cursor().left() + ui.available_width(),
        ui.cursor().top(),
        Stroke::new(1.0, p.line),
    );
}
fn row(ui: &mut Ui, label: &str, p: &Palette, content: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            let width = ui.available_width();
            let label_width = if width >= 520.0 { 208.0 } else { width };
            let galley = paint::layout(
                ui.painter(),
                paint::job(label, scale::SMALL.weight(550.0), p.ink, label_width),
            );
            if width >= 520.0 {
                let start = ui.cursor().min;
                let bounds = Rect::from_min_max(
                    pos2(start.x + label_width + 24.0, start.y),
                    pos2(start.x + width, ui.max_rect().bottom()),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                child.set_width(bounds.width());
                content(&mut child);
                let height = child.min_rect().height().max(galley.size().y);
                ui.allocate_exact_size(vec2(width, height), Sense::hover());
                ui.painter().galley(
                    pos2(start.x, start.y + (height - galley.size().y) / 2.0),
                    galley,
                    p.ink,
                );
            } else {
                let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                ui.painter().galley(rect.min, galley, p.ink);
                ui.add_space(8.0);
                content(ui);
            }
        });
}
#[allow(clippy::too_many_arguments)]
fn field(
    ui: &mut Ui,
    name: &str,
    value: &mut String,
    multiline: bool,
    placeholder: &str,
    hint: Option<&str>,
    generation: u64,
    p: &Palette,
) {
    row(ui, name, p, |ui| {
        let id = Id::new(("schedule-field", generation, name));
        let (rect, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), if multiline { 102.0 } else { 40.0 }),
            Sense::hover(),
        );
        if multiline {
            controls::field_frame(
                ui,
                id,
                rect,
                ui.memory(|m| m.has_focus(id)),
                ui.rect_contains_pointer(rect),
                FieldLook::new(p),
                p,
            );
            let inner = rect.shrink2(vec2(12.0, 10.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            egui::ScrollArea::vertical()
                .id_salt(id)
                .show(&mut child, |ui| {
                    let response = ui.add(
                        egui::TextEdit::multiline(value)
                            .id(id)
                            .frame(egui::Frame::NONE)
                            .font(scale::SMALL.font_id())
                            .text_color(p.ink)
                            .desired_width(inner.width())
                            .min_size(inner.size())
                            .char_limit(10_000)
                            .hint_text(placeholder),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), name)
                    });
                });
        } else {
            controls::text_field(
                ui,
                id,
                rect,
                value,
                placeholder,
                scale::SMALL,
                12.0,
                FieldLook::new(p),
                p,
                name,
            );
        }
        if let Some(hint) = hint {
            ui.add_space(6.0);
            text(ui, hint, Type::sans(12.0, 18.0), p.ink_muted);
        }
    });
}
fn select(
    ui: &mut Ui,
    name: &str,
    value: &mut String,
    options: &[(&str, &str)],
    generation: u64,
    p: &Palette,
) {
    row(ui, name, p, |ui| {
        let current = options
            .iter()
            .find(|(key, _)| *key == value)
            .map(|(_, label)| *label)
            .unwrap_or(value);
        ui.spacing_mut().interact_size.y = 40.0;
        ui.spacing_mut().button_padding = vec2(12.0, 10.0);
        let visuals = ui.visuals_mut();
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            widget.bg_fill = p.surface;
            widget.weak_bg_fill = p.surface;
            widget.bg_stroke = Stroke::new(1.0, p.line);
            widget.corner_radius = 10.0.into();
        }
        let response = egui::ComboBox::from_id_salt(("schedule-select", generation, name))
            .selected_text(
                egui::RichText::new(current)
                    .font(scale::SMALL.font_id())
                    .color(p.ink),
            )
            .width(ui.available_width().min(320.0))
            .height(240.0)
            .show_ui(ui, |ui| {
                for (key, label) in options {
                    ui.selectable_value(
                        value,
                        (*key).into(),
                        egui::RichText::new(*label).font(scale::SMALL.font_id()),
                    );
                }
            })
            .response;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), name)
        });
    });
}
fn choice(
    ui: &mut Ui,
    name: &str,
    value: &mut Option<u64>,
    options: &[cww::schedules::Choice],
    empty: &str,
    generation: u64,
    p: &Palette,
) {
    let mut current = value.map(|id| id.to_string()).unwrap_or_default();
    let mut entries = vec![(String::new(), empty.to_owned())];
    entries.extend(
        options
            .iter()
            .map(|option| (option.id.to_string(), option.name.clone())),
    );
    if let Some(id) = value
        && !options.iter().any(|option| option.id == *id)
    {
        entries.push((id.to_string(), "No longer available".into()));
    }
    let borrowed: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, label)| (key.as_str(), label.as_str()))
        .collect();
    select(ui, name, &mut current, &borrowed, generation, p);
    *value = current.parse().ok();
}
fn repeats(ui: &mut Ui, value: &mut String, generation: u64, p: &Palette) {
    let selected = value.as_str();
    let enabled = ui.is_enabled();
    let width = if ui.available_width() < 350.0 {
        ui.available_width()
    } else {
        136.0
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, p.canvas);
    ui.painter().rect_stroke(
        rect,
        8.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    let mut chosen = selected;
    for (index, (value, label)) in [("once", "Once"), ("repeat", "Repeat")]
        .into_iter()
        .enumerate()
    {
        let segment = Rect::from_min_size(
            rect.min + vec2(index as f32 * width / 2.0 + 2.0, 2.0),
            vec2(width / 2.0 - 4.0, 30.0),
        );
        let response = ui.interact(
            segment,
            Id::new(("schedule-repeat", generation, label)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                enabled,
                selected == value,
                label,
            )
        });
        if selected == value {
            ui.painter().rect_filled(segment, 6.0, p.surface);
            ui.painter().rect_stroke(
                segment,
                6.0,
                Stroke::new(1.0, p.line),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() && enabled {
            ui.painter().rect_filled(segment, 6.0, p.ink_wash(4.0));
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                segment,
                6.0,
                Stroke::new(2.0, p.ink_muted),
                egui::StrokeKind::Inside,
            );
        }
        let galley = paint::layout(
            ui.painter(),
            paint::line_job(
                label,
                scale::SMALL.weight(if selected == value { 550.0 } else { 400.0 }),
                if selected == value {
                    p.ink
                } else {
                    p.ink_muted
                },
                segment.width(),
            ),
        );
        ui.painter()
            .galley(segment.center() - galley.size() / 2.0, galley, p.ink);
        if enabled {
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            chosen = value;
        }
    }
    *value = chosen.to_owned();
}

fn button(
    ui: &mut Ui,
    label: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let mut b = Button::new(kind, Size::Small, label).enabled(enabled);
    if label == "Add schedule" {
        b = b.icon(Icon::Plus);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(b.width(ui), b.height()), Sense::hover());
    b.show_at(
        ui,
        ui.make_persistent_id(("schedule-button", label)),
        rect.min,
        p,
        images,
    )
    .clicked()
}
fn confirmation(
    ui: &mut Ui,
    id: &str,
    title: &str,
    description: &str,
    action: &str,
    p: &Palette,
    images: &Images,
) -> (bool, bool) {
    let mut keep = false;
    let mut confirmed = false;
    let open = controls::modal(ui.ctx(), Id::new(id), 440.0, 220.0, p, |ui, rect| {
        let height = controls::dialog_header(
            ui,
            rect.min,
            rect.width(),
            title,
            &[(description, false)],
            p,
        );
        let at = pos2(rect.left(), rect.top() + height + 24.0);
        let b = Button::new(
            Kind::Subtle,
            Size::Small,
            if id == "delete-schedule" {
                "Cancel"
            } else {
                "Keep editing"
            },
        );
        keep = b
            .show_at(ui, Id::new((id, "keep")), at, p, images)
            .clicked();
        confirmed = Button::new(Kind::Primary, Size::Small, action)
            .show_at(
                ui,
                Id::new((id, "confirm")),
                at + vec2(b.width(ui) + 8.0, 0.0),
                p,
                images,
            )
            .clicked();
    });
    (keep || !open, confirmed)
}
