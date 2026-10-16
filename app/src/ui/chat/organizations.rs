//! The website's account switcher, with native controls and local sign-ins.

use egui::{Align2, FontId, Id, Key, Rect, Sense, Ui, pos2, vec2};

use super::{
    controls,
    icons::{Icon, Images},
    sidebar::Event,
    tokens::Palette,
};
use crate::ui::organizations::Choice;

pub(super) fn show(
    ui: &mut Ui,
    anchor: Rect,
    choices: &[Choice],
    p: &Palette,
    images: &Images,
) -> Option<Event> {
    let current = choices.iter().find(|choice| choice.selected)?;
    let id = Id::new("organization-switcher");
    let mut open = ui
        .ctx()
        .data_mut(|data| data.get_temp::<bool>(id).unwrap_or(false));
    let response = ui
        .interact(anchor, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Organization: {}", current.name),
        )
    });
    if response.clicked() {
        open = !open;
    }
    if response.hovered() || response.has_focus() || open {
        ui.painter().rect_filled(anchor, 8.0, p.surface_hover);
    }
    mark(
        ui,
        Rect::from_min_size(anchor.min + vec2(8.0, 7.0), vec2(24.0, 24.0)),
        &current.name,
        p,
    );
    label(
        ui,
        &current.name,
        Rect::from_min_max(
            pos2(anchor.left() + 42.0, anchor.top()),
            pos2(anchor.right() - 28.0, anchor.bottom()),
        ),
        14.0,
        p.ink,
    );
    images.icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(anchor.right() - 14.0, anchor.center().y),
            vec2(14.0, 14.0),
        ),
        Icon::CaretUpDown,
        p.ink_muted,
    );
    let mut event = None;
    if open {
        let screen = ui.ctx().content_rect();
        let width = 284.0_f32.min(screen.width() - 16.0);
        let left = anchor
            .left()
            .min(screen.right() - width - 8.0)
            .max(screen.left() + 8.0);
        let top = anchor.bottom() + 4.0;
        let height =
            (choices.len() as f32 * 52.0 + 78.0).min((screen.bottom() - top - 8.0).max(100.0));
        let rect = Rect::from_min_size(pos2(left, top), vec2(width, height));
        egui::Area::new(id.with("menu"))
            .order(egui::Order::Tooltip)
            .fixed_pos(rect.min)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(rect.size());
                controls::menu_panel(ui.painter(), rect, p);
                label(
                    ui,
                    "Organizations",
                    Rect::from_min_size(rect.min + vec2(12.0, 4.0), vec2(width - 24.0, 28.0)),
                    12.0,
                    p.ink_muted,
                );
                let list_rect =
                    Rect::from_min_max(rect.min + vec2(6.0, 32.0), rect.max - vec2(6.0, 40.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
                child.set_clip_rect(list_rect);
                egui::ScrollArea::vertical()
                    .id_salt(id.with("list"))
                    .auto_shrink([false, false])
                    .show(&mut child, |ui| {
                        for choice in choices {
                            let (row, response) = ui
                                .allocate_exact_size(vec2(list_rect.width(), 52.0), Sense::click());
                            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    true,
                                    choice.selected,
                                    &choice.name,
                                )
                            });
                            if response.hovered() || response.has_focus() {
                                ui.painter().rect_filled(row, 6.0, p.surface_hover);
                            }
                            mark(
                                ui,
                                Rect::from_center_size(
                                    pos2(row.left() + 18.0, row.center().y),
                                    vec2(20.0, 20.0),
                                ),
                                &choice.name,
                                p,
                            );
                            label(
                                ui,
                                &choice.name,
                                Rect::from_min_max(
                                    pos2(row.left() + 36.0, row.top() + 3.0),
                                    pos2(row.right() - 30.0, row.top() + 29.0),
                                ),
                                14.0,
                                p.ink,
                            );
                            if let Some(role) = &choice.role {
                                let mut chars = role.chars();
                                let role = chars
                                    .next()
                                    .map(|first| {
                                        first.to_uppercase().chain(chars).collect::<String>()
                                    })
                                    .unwrap_or_default();
                                label(
                                    ui,
                                    &role,
                                    Rect::from_min_max(
                                        pos2(row.left() + 36.0, row.top() + 25.0),
                                        pos2(row.right() - 30.0, row.bottom() - 4.0),
                                    ),
                                    12.0,
                                    p.ink_muted,
                                );
                            }
                            if choice.selected {
                                images.icon(
                                    ui.painter(),
                                    Rect::from_center_size(
                                        pos2(row.right() - 14.0, row.center().y),
                                        vec2(16.0, 16.0),
                                    ),
                                    Icon::Check,
                                    p.ink,
                                );
                            }
                            if response.clicked() {
                                event = Some(Event::Organization(choice.id.clone()));
                            }
                        }
                    });
                let row = Rect::from_min_max(
                    pos2(rect.left() + 8.0, rect.bottom() - 36.0),
                    rect.max - vec2(8.0, 4.0),
                );
                if controls::menu_item(
                    ui,
                    id.with("add"),
                    row,
                    Some(Icon::Plus),
                    "Add an organization…",
                    false,
                    false,
                    p,
                    images,
                )
                .clicked()
                {
                    event = Some(Event::Page(crate::ui::Page::Account));
                }
            });
        if event.is_some()
            || controls::pressed_outside(ui, &[rect, anchor])
            || ui.input(|input| input.key_pressed(Key::Escape))
        {
            open = false;
        }
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, open));
    event
}

fn mark(ui: &Ui, rect: Rect, name: &str, p: &Palette) {
    ui.painter().rect_filled(rect, 5.0, p.surface_selected);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initials(name),
        FontId::proportional(10.0),
        p.ink_muted,
    );
}

fn label(ui: &Ui, text: &str, rect: Rect, size: f32, color: egui::Color32) {
    let mut job =
        egui::text::LayoutJob::simple_singleline(text.into(), FontId::proportional(size), color);
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

fn initials(name: &str) -> String {
    let words: Vec<_> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let letters: String = if words.len() > 1 {
        words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .collect()
    } else {
        words
            .first()
            .map_or_else(String::new, |word| word.chars().take(2).collect())
    };
    if letters.is_empty() {
        "?".into()
    } else {
        letters.to_uppercase()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn marks_match_the_web_account_initials() {
        for (name, mark) in [
            ("Northwind", "NO"),
            ("Akamai Robotics", "AR"),
            ("Plenty Labs Research", "PL"),
            ("🚀", "?"),
            ("Über büro", "ÜB"),
        ] {
            assert_eq!(super::initials(name), mark);
        }
    }
}
