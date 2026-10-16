//! The website's Models settings, including its personal default and rates.
use cww::tui::chat::{Failure, Model, ModelPreferences, Models, SettingsLink};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::{
    controls::{self, Button, Kind, Size},
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    state::Command,
    tokens::{Palette, Type, scale},
    widgets,
    workspace::request_id,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    account: String,
    pub preferences: Option<ModelPreferences>,
    pub selected: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub confirm_close: bool,
    saving: bool,
    reveal_nav: bool,
}

impl Screen {
    pub fn new(account: String) -> Self {
        Self {
            generation: request_id(),
            account,
            preferences: None,
            selected: None,
            busy: false,
            error: None,
            notice: None,
            confirm_close: false,
            saving: false,
            reveal_nav: true,
        }
    }

    pub fn dirty(&self) -> bool {
        self.preferences
            .as_ref()
            .is_some_and(|p| p.preferred_model_id != self.selected)
    }

    pub fn load(&mut self) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.saving = false;
        self.error = None;
        vec![Command::ModelPreferences(self.generation, None)]
    }

    pub fn save(&mut self) -> Vec<Command> {
        if self.busy || self.preferences.is_none() || !self.dirty() {
            return vec![];
        }
        self.busy = true;
        self.saving = true;
        self.error = None;
        self.notice = None;
        vec![Command::ModelPreferences(
            self.generation,
            Some((self.account.clone(), self.selected.clone())),
        )]
    }

    pub fn loaded(&mut self, result: Result<ModelPreferences, Failure>) -> Option<Models> {
        self.busy = false;
        let saving = std::mem::take(&mut self.saving);
        match result {
            Ok(preferences) if preferences.account.id == self.account => {
                self.selected = preferences.preferred_model_id.clone();
                let catalog = preferences.catalog.clone();
                self.preferences = Some(preferences);
                self.error = None;
                self.notice = saving.then(|| "Default model updated.".into());
                Some(catalog)
            }
            Ok(_) => {
                self.preferences = None;
                self.selected = None;
                self.error = Some("Your organization changed. Open model settings again.".into());
                None
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
                ) {
                    self.preferences = None;
                    self.selected = None;
                }
                self.error = Some(error.message);
                None
            }
        }
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
    let mut outcome = Outcome::default();
    // Website .settings: max-w-5xl, 11rem navigation and a 3rem column gap.
    let wide = rect.width() >= 768.0;
    let width = (rect.width() - if wide { 64.0 } else { 32.0 }).clamp(1.0, 992.0);
    let bounds = Rect::from_min_max(
        pos2(rect.center().x - width / 2.0, rect.top()),
        pos2(rect.center().x + width / 2.0, rect.bottom()),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds);
    child.style_mut().spacing.scroll.floating = false;
    child.style_mut().spacing.scroll.foreground_color = false;
    child.visuals_mut().widgets.inactive.bg_fill = p.line_strong;
    child.visuals_mut().widgets.hovered.bg_fill = p.ink_faint;
    child.visuals_mut().widgets.active.bg_fill = p.ink_muted;
    egui::ScrollArea::vertical()
        .id_salt(("model-preferences", screen.generation))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add_space(if wide { 48.0 } else { 0.0 });
            ui.horizontal(|ui| {
                if !wide {
                    ui.add_space(32.0);
                }
                ui.vertical(|ui| {
                    text(
                        ui,
                        "Settings",
                        Type::sans(
                            if wide { 24.0 } else { 18.0 },
                            if wide { 32.0 } else { 27.0 },
                        )
                        .weight(600.0),
                        p.ink,
                    );
                    if let Some(preferences) = &screen.preferences {
                        let label = format!("{} · {}", preferences.email, preferences.account.name);
                        text(ui, &label, scale::MICRO, p.ink_faint);
                    }
                });
            });
            ui.add_space(24.0);
            if wide {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        vec2(176.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            navigation(ui, tabs, true, false, screen.busy, p, images, &mut outcome);
                        },
                    );
                    ui.add_space(48.0);
                    ui.vertical(|ui| content(ui, screen, p, images, &mut outcome));
                });
            } else {
                egui::ScrollArea::horizontal()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .id_salt("model-settings-tabs")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            navigation(
                                ui,
                                tabs,
                                false,
                                screen.reveal_nav,
                                screen.busy,
                                p,
                                images,
                                &mut outcome,
                            )
                        });
                    });
                ui.add_space(24.0);
                content(ui, screen, p, images, &mut outcome);
            }
            ui.add_space(64.0);
        });
    if !tabs.is_empty() {
        screen.reveal_nav = false;
    }
    if screen.confirm_close {
        let mut keep = false;
        let open = controls::modal(
            ui.ctx(),
            Id::new("discard-model-preferences"),
            420.0,
            204.0,
            p,
            |ui, rect| {
                let height = controls::dialog_header(
                    ui,
                    rect.min,
                    rect.width(),
                    "Discard model selection?",
                    &[("Your default model has not been saved.", false)],
                    p,
                );
                let at = pos2(rect.left(), rect.top() + height + 28.0);
                let keep_button = Button::new(Kind::Subtle, Size::Small, "Keep editing");
                keep = keep_button
                    .show_at(ui, Id::new("keep-model-selection"), at, p, images)
                    .clicked();
                outcome.discarded = Button::new(Kind::Primary, Size::Small, "Discard changes")
                    .show_at(
                        ui,
                        Id::new("discard-model-selection"),
                        pos2(at.x + keep_button.width(ui) + 8.0, at.y),
                        p,
                        images,
                    )
                    .clicked();
            },
        );
        if !open || keep {
            screen.confirm_close = false;
        }
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn navigation(
    ui: &mut Ui,
    tabs: &[SettingsLink],
    wide: bool,
    reveal: bool,
    busy: bool,
    p: &Palette,
    images: &Images,
    outcome: &mut Outcome,
) {
    for tab in tabs {
        let selected = tab.tab == "models";
        let galley = paint::layout(
            ui.painter(),
            paint::line_job(
                &tab.label,
                scale::SMALL.weight(if selected { 550.0 } else { 400.0 }),
                if selected { p.ink } else { p.ink_muted },
                150.0,
            ),
        );
        let width = if wide { 176.0 } else { galley.size().x + 24.0 };
        let (row, _) = ui.allocate_exact_size(vec2(width, 33.0), Sense::hover());
        let response = ui.interact(
            row,
            Id::new(("model-settings-tab", &tab.tab)),
            if busy { Sense::hover() } else { Sense::click() },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                !busy,
                selected,
                &tab.label,
            )
        });
        if selected || response.hovered() {
            ui.painter()
                .rect_filled(row, 8.0, if selected { p.surface } else { p.ink_wash(5.0) });
        }
        if selected {
            ui.painter()
                .rect_stroke(row, 8.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
        }
        if wide {
            images.icon_at(
                ui.painter(),
                pos2(row.left() + 20.0, row.center().y),
                16.0,
                tab.icon
                    .as_deref()
                    .and_then(Icon::named)
                    .unwrap_or(Icon::GearSix),
                p.ink_muted,
            );
        }
        ui.painter().galley(
            pos2(
                row.left() + if wide { 38.0 } else { 12.0 },
                row.center().y - galley.size().y / 2.0,
            ),
            galley,
            p.ink,
        );
        if !busy {
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if selected && reveal {
            response.scroll_to_me(Some(egui::Align::Center));
        }
        if response.clicked() && !selected {
            outcome.event = Some(Event::OpenUrl(tab.url.clone()));
        }
        if wide {
            ui.add_space(4.0);
        }
    }
}

fn content(ui: &mut Ui, screen: &mut Screen, p: &Palette, images: &Images, outcome: &mut Outcome) {
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
        text(
            ui,
            "Default model",
            Type::sans(15.0, 22.5).weight(600.0),
            p.ink,
        );
        if let Some(preferences) = &screen.preferences {
            let count = preferences.catalog.models.len();
            let label = format!("{count} {}", if count == 1 { "model" } else { "models" });
            let g = paint::layout(
                ui.painter(),
                paint::line_job(&label, scale::MICRO, p.ink_faint, 100.0),
            );
            ui.add_space((ui.available_width() - g.size().x).max(0.0));
            ui.add(egui::Label::new(g));
        }
    });
    ui.add_space(4.0);
    text(
        ui,
        "New chats start with this model, and you can switch models in any chat. Rates are for a typical answer that searches and reads a few files.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    if let Some(error) = &screen.error {
        text(ui, error, scale::SMALL, p.negative_ink());
        ui.add_space(12.0);
    }
    if let Some(notice) = &screen.notice {
        text(ui, notice, scale::SMALL, p.ink);
        ui.add_space(12.0);
    }
    let Some(preferences) = &screen.preferences else {
        text(
            ui,
            if screen.busy {
                "Loading model settings…"
            } else {
                "Model settings could not be loaded."
            },
            scale::SMALL,
            p.ink_muted,
        );
        ui.add_space(16.0);
        if !screen.busy && button(ui, "Retry", Kind::Subtle, true, p, images) {
            outcome.commands.extend(screen.load());
        }
        return;
    };
    if preferences.catalog.models.is_empty() {
        text(
            ui,
            "No models are available right now.",
            scale::SMALL,
            p.ink_muted,
        );
        return;
    }
    let top = ui.cursor().min;
    let width = ui.available_width();
    let bg = ui.painter().add(egui::Shape::Noop);
    for (index, model) in preferences.catalog.models.iter().enumerate() {
        if index > 0 {
            ui.painter().hline(
                top.x..=top.x + width,
                ui.cursor().top(),
                Stroke::new(1.0, p.line),
            );
        }
        if model_row(
            ui,
            model,
            screen.selected.as_ref() == Some(&model.id),
            preferences.preferred_model_id.as_ref() == Some(&model.id),
            !screen.busy,
            p,
            images,
        ) {
            screen.selected = Some(model.id.clone());
            screen.notice = None;
        }
    }
    let group = Rect::from_min_max(top, pos2(top.x + width, ui.cursor().top()));
    ui.painter()
        .set(bg, egui::Shape::rect_filled(group, 14.0, p.surface));
    ui.painter().rect_stroke(
        group,
        14.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    ui.add_space(16.0);
    let save = Button::new(Kind::Primary, Size::Small, "Save default model")
        .enabled(!screen.busy && screen.dirty());
    let (row, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), save.height()), Sense::hover());
    if save
        .show_at(
            ui,
            Id::new("save-default-model"),
            pos2(row.right() - save.width(ui), row.top()),
            p,
            images,
        )
        .clicked()
    {
        outcome.commands.extend(screen.save());
    }
}

fn model_row(
    ui: &mut Ui,
    model: &Model,
    selected: bool,
    default: bool,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let width = ui.available_width();
    let text_width = (width - 120.0).max(1.0);
    let title = paint::layout(
        ui.painter(),
        paint::job(&model.name, scale::SMALL.weight(550.0), p.ink, text_width),
    );
    let description = paint::layout(
        ui.painter(),
        paint::job(
            model
                .description
                .as_deref()
                .or(model.rate.as_deref())
                .unwrap_or(""),
            scale::SMALL,
            p.ink_muted,
            text_width,
        ),
    );
    let rate = paint::layout(
        ui.painter(),
        paint::job(
            model.rate.as_deref().unwrap_or("Pricing varies"),
            scale::MICRO,
            p.ink_faint,
            text_width,
        ),
    );
    let tag = default.then(|| {
        paint::layout(
            ui.painter(),
            paint::line_job("Default", Type::mono(10.0, 15.0), p.ink_muted, text_width),
        )
    });
    let tag_height = tag.as_ref().map_or(0.0, |tag| {
        if title.size().x + tag.size().x + 14.0 <= text_width {
            0.0
        } else {
            20.0
        }
    });
    let height = title.size().y + tag_height + description.size().y + 2.0 + rate.size().y + 28.0;
    let (row, _) = ui.allocate_exact_size(vec2(width, height.max(64.0)), Sense::hover());
    let response = ui.interact(
        row,
        Id::new(("default-model", &model.id)),
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
            selected,
            &model.name,
        )
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            row.shrink(1.0),
            12.0,
            p.ink_wash(if selected { 4.5 } else { 3.0 }),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            row.shrink(2.0),
            12.0,
            Stroke::new(2.0, p.ink_muted),
            egui::StrokeKind::Inside,
        );
    }
    let mark = Rect::from_center_size(pos2(row.left() + 34.0, row.center().y), vec2(36.0, 36.0));
    ui.painter().rect_filled(mark, 8.0, p.canvas);
    ui.painter().rect_stroke(
        mark,
        8.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    if let Some(logo) = &model.logo {
        images.logos.draw(
            ui.painter(),
            Some(logo),
            Rect::from_center_size(mark.center(), vec2(20.0, 20.0)),
            0.0,
            p,
            1.0,
        );
    } else {
        images.icon_at(
            ui.painter(),
            mark.center(),
            20.0,
            Icon::Sparkle,
            p.ink_muted,
        );
    }
    let mut at = pos2(row.left() + 68.0, row.top() + 14.0);
    let title_height = title.size().y;
    let title_width = title.size().x;
    ui.painter().galley(at, title, p.ink);
    at.y += title_height;
    if let Some(tag) = tag {
        let tag_at = if tag_height == 0.0 {
            pos2(at.x + title_width + 10.0, at.y - title_height + 3.0)
        } else {
            at
        };
        ui.painter().rect_stroke(
            Rect::from_min_size(tag_at - vec2(4.0, 1.0), tag.size() + vec2(8.0, 2.0)),
            4.0,
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(tag_at, tag, p.ink_muted);
        at.y += tag_height;
    }
    widgets::label(
        ui,
        Rect::from_min_size(at, description.size()),
        model.description.as_deref().unwrap_or_default(),
    );
    let description_height = description.size().y;
    ui.painter().galley(at, description, p.ink_muted);
    at.y += description_height + 2.0;
    widgets::label(
        ui,
        Rect::from_min_size(at, rate.size()),
        model.rate.as_deref().unwrap_or("Pricing varies"),
    );
    ui.painter().galley(at, rate, p.ink_faint);
    let center = pos2(row.right() - 26.0, row.center().y);
    ui.painter().circle_stroke(
        center,
        9.0,
        Stroke::new(1.0, if selected { p.ink } else { p.line_strong }),
    );
    if selected {
        ui.painter().circle_filled(center, 4.5, p.ink);
    }
    if enabled {
        response
            .clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn text(ui: &mut Ui, text: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(text, ty, color, ui.available_width())).wrap());
}
fn button(
    ui: &mut Ui,
    label: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let b = Button::new(kind, Size::Small, label).enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(vec2(b.width(ui), b.height()), Sense::hover());
    b.show_at(ui, Id::new(("model-button", label)), rect.min, p, images)
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn preferences(account: &str) -> ModelPreferences {
        serde_json::from_value(serde_json::json!({"account":{"id":account,"name":"Northwind"},"email":"alex@example.com","preferred_model_id":4,"default_model_id":3,"models":[{"id":3,"name":"Small"},{"id":4,"name":"Large","selectable":false}]})).unwrap()
    }

    #[test]
    fn a_failed_save_keeps_the_choice_and_an_access_loss_drops_private_settings() {
        let mut screen = Screen::new("482139075".into());
        screen.load();
        screen.loaded(Ok(preferences("482139075")));
        screen.selected = Some("3".into());
        assert_eq!(
            screen.save(),
            vec![Command::ModelPreferences(
                screen.generation,
                Some(("482139075".into(), Some("3".into())))
            )]
        );
        assert!(screen.save().is_empty());
        screen.loaded(Err(Failure::new("unavailable", "Try again.")));
        assert!(screen.dirty());
        assert_eq!(screen.selected.as_deref(), Some("3"));
        assert!(screen.notice.is_none());
        screen.save();
        screen.loaded(Err(Failure::new("forbidden", "Chats were revoked.")));
        assert!(screen.preferences.is_none());
        assert!(screen.selected.is_none());
        assert!(screen.save().is_empty());
    }

    #[test]
    fn a_mismatched_account_cannot_supply_settings_or_update_the_composer() {
        let mut screen = Screen::new("482139075".into());
        screen.load();
        assert!(screen.loaded(Ok(preferences("918273645"))).is_none());
        assert!(screen.preferences.is_none());
        assert!(!screen.busy);
        assert!(screen.save().is_empty());
    }
}
