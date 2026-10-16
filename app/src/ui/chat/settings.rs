//! The website's shared settings header, navigation and responsive layout.
use super::{
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    tokens::{Palette, Type, scale},
};
use cww::tui::chat::SettingsLink;
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

pub struct Shell<'a> {
    pub rect: Rect,
    pub generation: u64,
    pub header: Option<&'a str>,
    pub active: &'a str,
    pub tabs: &'a [SettingsLink],
    pub busy: bool,
    pub palette: &'a Palette,
    pub images: &'a Images,
}

impl Shell<'_> {
    pub fn show(&self, ui: &mut Ui, contents: impl FnOnce(&mut Ui)) -> Option<Event> {
        let mut event = None;
        let rect = self.rect;
        let p = self.palette;
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
        let nav_id = Id::new(("settings-navigation", self.generation, self.active, wide));
        let reveal = ui.data(|d| d.get_temp::<bool>(nav_id)).is_none();
        egui::ScrollArea::vertical()
            .id_salt(("settings", self.generation))
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
                        if let Some(header) = self.header {
                            text(ui, header, scale::MICRO, p.ink_faint);
                        }
                    });
                });
                ui.add_space(24.0);
                if wide {
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            vec2(176.0, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| navigation(ui, self, true, false, &mut event),
                        );
                        ui.add_space(48.0);
                        ui.vertical(contents);
                    });
                } else {
                    egui::ScrollArea::horizontal()
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .id_salt("settings-tabs")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| navigation(ui, self, false, reveal, &mut event));
                        });
                    ui.add_space(24.0);
                    contents(ui);
                }
                ui.add_space(64.0);
            });
        if !self.tabs.is_empty() {
            ui.data_mut(|d| d.insert_temp(nav_id, true));
        }
        event
    }
}

fn text(ui: &mut Ui, label: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(label, ty, color, ui.available_width())).wrap());
}

#[allow(clippy::too_many_arguments)]
fn navigation(ui: &mut Ui, shell: &Shell, wide: bool, reveal: bool, event: &mut Option<Event>) {
    let p = shell.palette;
    let images = shell.images;
    let busy = shell.busy;
    for tab in shell.tabs {
        let selected = tab.tab == shell.active;
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
            Id::new(("settings-tab", &tab.tab)),
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
            response.scroll_to_me(None);
        }
        if response.clicked() && !selected {
            *event = Some(match tab.tab.as_str() {
                "instructions" => Event::Instructions(cww::workspace::Layer::Personal),
                "schedules" => Event::Schedules(cww::schedules::Request::List),
                "account" => Event::AccountSettings,
                "people" => Event::People,
                "models" => Event::ModelPreferences,
                "templates" => Event::Templates(None),
                _ => Event::OpenUrl(tab.url.clone()),
            });
        }
        if wide {
            ui.add_space(4.0);
        }
    }
}
