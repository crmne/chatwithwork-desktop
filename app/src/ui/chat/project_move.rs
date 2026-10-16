//! The website's Move to a project page, with the same audience explanation.
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::controls::{Button, Kind, Size};
use super::icons::{Icon, Images};
use super::paint;
use super::state::{ChatAction, ChatState, Command};
use super::tokens::{Palette, Type, scale};
use super::widgets;

pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ChatState,
    p: &Palette,
    images: &Images,
) -> Vec<Command> {
    let Some(mut page) = state.moving.clone() else {
        return vec![];
    };
    let summary = state.summary(page.chat).cloned();
    let allowed = summary
        .as_ref()
        .is_some_and(|chat| chat.can(|can| can.r#move));
    let busy = state.acting.is_some();
    let width = (rect.width() - if rect.width() < 640.0 { 32.0 } else { 48.0 }).clamp(1.0, 768.0);
    let bounds = Rect::from_min_max(
        pos2(
            rect.center().x - width / 2.0,
            rect.top() + if rect.width() >= 768.0 { 72.0 } else { 48.0 },
        ),
        pos2(rect.center().x + width / 2.0, rect.bottom()),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds);
    child.style_mut().spacing.scroll.floating = false;
    let mut close = false;
    let mut action = None;
    egui::ScrollArea::vertical().id_salt(("move-chat", page.chat)).auto_shrink([false, false]).show(&mut child, |ui| {
        let width = ui.available_width();
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let title = summary.as_ref().map_or("Chat", |chat| chat.title.as_str());
        let back = ui.add_enabled(!busy, egui::Label::new(paint::job(&format!("← {title}"), scale::MICRO, p.ink_faint, width)).sense(Sense::click()));
        back.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, !busy, "Back to chat"));
        if back.hovered() && !busy { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
        close = back.clicked();
        ui.add_space(6.0);
        text(ui, "Move to a project", Type::sans(24.0, 32.0).weight(600.0), p.ink);
        ui.add_space(8.0);
        let lede = "Everyone in the project will be able to read this chat and continue it with their own sources.";
        let galley = paint::layout(ui.painter(), paint::job(lede, Type::sans(15.0, 22.5), p.ink_muted, width.min(608.0)));
        let (at, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
        widgets::label(ui, at, lede);
        ui.painter().galley(at.min, galley, p.ink_muted);
        ui.add_space(32.0);
        text(ui, "What they'll see", Type::sans(15.0, 22.5).weight(600.0), p.ink);
        ui.add_space(12.0);
        explanation(ui, &state.list.user.name, p, images);
        ui.add_space(32.0);
        text(ui, "Project", Type::sans(15.0, 22.5).weight(600.0), p.ink);
        ui.add_space(12.0);
        if !allowed {
            text(ui, "Only the person who started a chat can change it.", scale::SMALL, p.negative());
        } else if state.list.projects.is_empty() {
            text(ui, "You're not in any projects", scale::SMALL, p.ink_muted);
        }
        let group_start = ui.cursor().min;
        let background = ui.painter().add(egui::Shape::Noop);
        for project in &state.list.projects {
            let audience = if project.hq { format!("Everyone in {}", state.list.account.name) }
                else { match project.people_count { Some(1) => "1 person".into(), Some(n) => format!("{n} people"), None => "People in this project".into() } };
            let name = paint::layout(ui.painter(), paint::job(&project.name, scale::SMALL.weight(560.0), p.ink, width - 68.0));
            let note = paint::layout(ui.painter(), paint::job(&audience, Type::mono(11.0, 16.5), p.ink_faint, width - 68.0));
            let (row, _) = ui.allocate_exact_size(vec2(width, 28.0 + name.size().y + note.size().y + 2.0), Sense::hover());
            let id = Id::new(("move-project", project.id));
            let response = ui.interact(row, id, if allowed && !busy { Sense::click() } else { Sense::hover() });
            let selected = page.project == Some(project.id);
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, allowed && !busy, selected, format!("Project: {}", project.name)));
            if selected || response.hovered() { ui.painter().rect_filled(row.shrink(1.0), 12.0, p.ink_wash(if selected { 4.5 } else { 3.0 })); }
            if row.top() > group_start.y + 1.0 { ui.painter().hline(row.x_range(), row.top(), Stroke::new(1.0, p.line)); }
            let center = pos2(row.left() + 26.0, row.center().y);
            ui.painter().circle_stroke(center, 10.0, Stroke::new(1.0, p.line_strong));
            if selected { ui.painter().circle_filled(center, 3.5, p.ink); }
            ui.painter().galley(row.min + vec2(52.0, 14.0), name.clone(), p.ink);
            ui.painter().galley(row.min + vec2(52.0, 16.0 + name.size().y), note, p.ink_muted);
            if response.has_focus() { widgets::focus_ring(ui, row, 0.0, p); }
            if response.hovered() && allowed && !busy { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
            if response.clicked() { page.project = Some(project.id); }
        }
        if !state.list.projects.is_empty() {
            let group = Rect::from_min_max(group_start, pos2(group_start.x + width, ui.cursor().top()));
            ui.painter().set(background, egui::Shape::rect_filled(group, 14.0, p.surface));
            ui.painter().rect_stroke(group, 14.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
        }
        ui.add_space(12.0);
        let selected = page.project.filter(|id| state.list.projects.iter().any(|project| project.id == *id));
        let confirm = Button::new(Kind::Primary, Size::Small, if busy { "Moving…" } else { "Move chat" }).enabled(allowed && !busy && selected.is_some());
        let (footer, _) = ui.allocate_exact_size(vec2(width, confirm.height()), Sense::hover());
        if confirm.show_at(ui, Id::new("move-confirm"), pos2(footer.right() - confirm.width(ui), footer.top()), p, images).clicked() {
            action = Some(ChatAction::Move(selected));
        }
        if let Some(project) = summary.as_ref().and_then(|chat| chat.project.as_ref()) {
            ui.add_space(32.0);
            text(ui, "Make it private again", Type::sans(15.0, 22.5).weight(600.0), p.ink);
            ui.add_space(16.0);
            if private_row(ui, &project.name, allowed && !busy, p, images) { action = Some(ChatAction::Move(None)); }
        }
        if let Some(error) = &state.notice { ui.add_space(16.0); text(ui, error, scale::SMALL, p.negative()); }
        ui.add_space(32.0);
    });
    state.moving = if close { None } else { Some(page.clone()) };
    action.map_or_else(Vec::new, |action| state.act(page.chat, action))
}

fn text(ui: &mut Ui, text: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(text, ty, color, ui.available_width())).wrap());
}

fn private_row(ui: &mut Ui, project: &str, enabled: bool, p: &Palette, images: &Images) -> bool {
    let width = ui.available_width();
    let narrow = width < 640.0;
    let button = Button::new(Kind::Subtle, Size::Small, "Make private").enabled(enabled);
    let text_width = width
        - if narrow {
            32.0
        } else {
            48.0 + button.width(ui)
        };
    let heading = format!("Take it out of {project}");
    let description = "Only you will see it. Anything people already read stays with them.";
    let title = paint::layout(
        ui.painter(),
        paint::job(&heading, scale::SMALL.weight(550.0), p.ink, text_width),
    );
    let body = paint::layout(
        ui.painter(),
        paint::job(description, scale::SMALL, p.ink_muted, text_width),
    );
    let height = (title.size().y + body.size().y).max(button.height())
        + 28.0
        + if narrow { button.height() + 12.0 } else { 0.0 };
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(row, 14.0, p.surface);
    ui.painter().rect_stroke(
        row,
        14.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    let at = row.min + vec2(16.0, 14.0);
    widgets::label(ui, Rect::from_min_size(at, title.size()), &heading);
    widgets::label(
        ui,
        Rect::from_min_size(at + vec2(0.0, title.size().y), body.size()),
        description,
    );
    ui.painter()
        .galley(at + vec2(0.0, title.size().y), body, p.ink_muted);
    ui.painter().galley(at, title, p.ink);
    button
        .show_at(
            ui,
            Id::new("move-private"),
            pos2(
                row.right() - 16.0 - button.width(ui),
                if narrow {
                    row.bottom() - 14.0 - button.height()
                } else {
                    row.center().y - button.height() / 2.0
                },
            ),
            p,
            images,
        )
        .clicked()
}

fn explanation(ui: &mut Ui, person: &str, p: &Palette, images: &Images) {
    let rows = [
        (Icon::Eye, "Your questions and the answers", "Including what the answers quote from your documents, and their citations. Links open only for people the source itself lets in.".to_string()),
        (Icon::EyeSlash, "Not what your tools returned", format!("They see where results came from, like \"Searched {person}'s Drive\", not the results. Your connectors are never used for their questions.")),
    ];
    let start = ui.cursor().min;
    let width = ui.available_width();
    let background = ui.painter().add(egui::Shape::Noop);
    for (index, (icon, heading, description)) in rows.iter().enumerate() {
        let title = paint::layout(
            ui.painter(),
            paint::job(heading, scale::SMALL.weight(550.0), p.ink, width - 84.0),
        );
        let body = paint::layout(
            ui.painter(),
            paint::job(description, scale::SMALL, p.ink_muted, width - 84.0),
        );
        let (row, _) = ui.allocate_exact_size(
            vec2(width, (title.size().y + body.size().y).max(36.0) + 28.0),
            Sense::hover(),
        );
        let badge =
            Rect::from_center_size(pos2(row.left() + 34.0, row.center().y), vec2(36.0, 36.0));
        ui.painter().rect_filled(badge, 10.0, p.canvas);
        ui.painter().rect_stroke(
            badge,
            10.0,
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Inside,
        );
        images.icon_at(ui.painter(), badge.center(), 16.0, *icon, p.ink);
        let at = row.min + vec2(68.0, 14.0);
        widgets::label(ui, Rect::from_min_size(at, title.size()), heading);
        widgets::label(
            ui,
            Rect::from_min_size(at + vec2(0.0, title.size().y), body.size()),
            description,
        );
        ui.painter()
            .galley(at + vec2(0.0, title.size().y), body, p.ink_muted);
        ui.painter().galley(at, title, p.ink);
        if index != 0 {
            ui.painter()
                .hline(row.x_range(), row.top(), Stroke::new(1.0, p.line));
        }
    }
    let group = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().top()));
    ui.painter()
        .set(background, egui::Shape::rect_filled(group, 14.0, p.surface));
    ui.painter().rect_stroke(
        group,
        14.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
}
