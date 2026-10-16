//! Calendar and time controls edit the wall-clock fields the server expects.
//! The selected schedule's zone, not the computer's zone, seeds an empty date.
use super::{FieldLook, Icon, Images, Palette, controls, scale};
use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use jiff::civil::{Date, Time};

pub(super) fn input(
    ui: &mut Ui,
    name: &str,
    value: &mut String,
    with_date: bool,
    zone: &str,
    p: &Palette,
    images: &Images,
) {
    let id = ui.make_persistent_id("value");
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    controls::field_frame(
        ui,
        id,
        rect,
        ui.memory(|m| m.has_focus(id)),
        ui.rect_contains_pointer(rect),
        FieldLook::new(p),
        p,
    );
    let buttons = if with_date { 64.0 } else { 32.0 };
    let inner = Rect::from_min_max(
        pos2(rect.left() + 12.0, rect.center().y - 10.0),
        pos2(rect.right() - buttons - 8.0, rect.center().y + 10.0),
    );
    let mut displayed = if with_date {
        value.replacen('T', " ", 1)
    } else {
        value.clone()
    };
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(&mut displayed)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(scale::SMALL.font_id())
            .text_color(p.ink)
            .hint_text(if with_date {
                "YYYY-MM-DD HH:MM"
            } else {
                "HH:MM"
            }),
    );
    if response.changed() {
        *value = if with_date {
            displayed.replacen(' ', "T", 1)
        } else {
            displayed
        };
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), name)
    });
    let clock =
        Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(32.0, 32.0));
    if with_date {
        let calendar = clock.translate(vec2(-32.0, 0.0));
        calendar_button(ui, calendar, value, zone, p);
    }
    let response = ui
        .interact(clock, ui.make_persistent_id("clock"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Choose time");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Choose time")
    });
    images.icon_at(ui.painter(), clock.center(), 16.0, Icon::Clock, p.ink_muted);
    let reveal = response.clicked();
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let time = selected_time(value, with_date).unwrap_or(Time::new(8, 0, 0, 0).unwrap());
            let (mut hour, mut minute) = (time.hour(), time.minute());
            ui.horizontal(|ui| {
                time_part(ui, "Hour", &mut hour, 24, reveal);
                time_part(ui, "Minute", &mut minute, 60, reveal);
            });
            if (hour, minute) != (time.hour(), time.minute()) {
                set_time(value, with_date, zone, hour, minute);
            }
        });
}

fn calendar_button(ui: &mut Ui, rect: Rect, value: &mut String, zone: &str, p: &Palette) {
    // The upstream calendar builds complete weeks around a month. Its extreme
    // year boundaries cannot represent those adjacent days; leave typed values
    // intact and open such a value's picker at today instead.
    let mut date = selected_date(value)
        .filter(|date| (1..=9998).contains(&date.year()))
        .unwrap_or_else(|| today(zone));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.spacing_mut().interact_size = rect.size();
    // The surrounding input already supplies the field's border.
    let widgets = &mut child.visuals_mut().widgets;
    widgets.inactive.bg_fill = p.surface;
    widgets.inactive.weak_bg_fill = p.surface;
    widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
    widgets.hovered.bg_fill = p.surface_hover;
    widgets.hovered.weak_bg_fill = p.surface_hover;
    widgets.open.bg_fill = p.surface_hover;
    widgets.open.weak_bg_fill = p.surface_hover;
    let response = child
        .add(
            egui_extras::DatePickerButton::new(&mut date)
                .id_salt("calendar")
                .show_icon(false)
                .calendar_week(false)
                .highlight_weekends(false)
                .arrows(false)
                .format(" "),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Choose date");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Choose date")
    });
    let icon = Rect::from_center_size(rect.center(), vec2(14.0, 12.0));
    let stroke = Stroke::new(1.5, p.ink_muted);
    ui.painter()
        .rect_stroke(icon, 2.0, stroke, egui::StrokeKind::Inside);
    ui.painter().hline(icon.x_range(), icon.top() + 4.0, stroke);
    for x in [icon.left() + 3.5, icon.right() - 3.5] {
        ui.painter()
            .vline(x, icon.top() - 2.0..=icon.top() + 2.0, stroke);
    }
    if response.changed() {
        let time = selected_time(value, true).unwrap_or(Time::new(8, 0, 0, 0).unwrap());
        *value = format!("{date}T{:02}:{:02}", time.hour(), time.minute());
    }
}

fn time_part(ui: &mut Ui, label: &str, value: &mut i8, max: i8, reveal: bool) {
    ui.vertical(|ui| {
        ui.set_width(72.0);
        ui.label(label);
        egui::ScrollArea::vertical()
            .id_salt(label)
            .min_scrolled_height(200.0)
            .max_height(200.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for part in 0..max {
                    let selected = *value == part;
                    let response = ui.selectable_value(value, part, format!("{part:02}"));
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            selected,
                            format!("{label} {part:02}"),
                        )
                    });
                    if reveal && selected {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                }
            });
    });
}

fn today(zone: &str) -> Date {
    let zone = jiff::tz::TimeZone::get(zone).unwrap_or(jiff::tz::TimeZone::UTC);
    jiff::Timestamp::now().to_zoned(zone).date()
}
fn selected_date(value: &str) -> Option<Date> {
    value.split_once('T')?.0.parse().ok()
}
fn selected_time(value: &str, with_date: bool) -> Option<Time> {
    let time = if with_date {
        value.split_once('T')?.1
    } else {
        value
    };
    time.parse().ok()
}
fn set_time(value: &mut String, with_date: bool, zone: &str, hour: i8, minute: i8) {
    let time = format!("{hour:02}:{minute:02}");
    *value = if with_date {
        let date = selected_date(value).unwrap_or_else(|| today(zone));
        format!("{date}T{time}")
    } else {
        time
    };
}
