//! The website's Chats page, including history beyond the sidebar's window.
use cww::tui::chat::HistoryItem;
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::controls::{self, Button, FieldLook, Kind, Size};
use super::icons::{Icon, Images};
use super::paint;
use super::sidebar::Event;
use super::state::ChatState;
use super::tokens::{Palette, Type, scale};
use super::{widgets, workspace};

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPage {
    pub generation: u64,
    pub query: String,
    pub items: Vec<HistoryItem>,
    pub total: Option<u64>,
    pub next_cursor: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    due: Option<f64>,
}

impl Default for HistoryPage {
    fn default() -> Self {
        Self {
            generation: workspace::request_id(),
            query: String::new(),
            items: vec![],
            total: None,
            next_cursor: None,
            loaded: false,
            loading: false,
            error: None,
            due: None,
        }
    }
}

impl HistoryPage {
    pub fn remove(&mut self, number: u64) {
        // A response queued before an access loss must not put the chat back.
        self.generation = workspace::request_id();
        self.loading = false;
        let before = self.items.len();
        self.items.retain(|item| item.chat.number != number);
        if self.items.len() != before {
            self.total = self.total.map(|n| n.saturating_sub(1));
        }
    }

    pub fn search(&mut self, now: f64) {
        self.generation = workspace::request_id();
        self.items.clear();
        self.next_cursor = None;
        self.loaded = false;
        self.loading = false;
        self.error = None;
        self.due = Some(now + 0.25);
    }
}

pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ChatState,
    p: &Palette,
    images: &Images,
) -> (Option<Event>, bool) {
    let locked = state.locked_reason().map(str::to_owned);
    let busy = state.acting.is_some();
    let Some(page) = &mut state.history else {
        return (None, false);
    };
    let now = ui.input(|i| i.time);
    let mut event = None;
    let mut load = !page.loaded
        && !page.loading
        && page.error.is_none()
        && page.due.is_none_or(|due| now >= due);
    if let Some(due) = page.due.filter(|due| *due > now) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(due - now));
    }
    let width = (rect.width() - 32.0).clamp(1.0, 736.0);
    let bounds = Rect::from_min_max(
        pos2(
            rect.center().x - width / 2.0,
            rect.top() + if rect.width() < 640.0 { 40.0 } else { 56.0 },
        ),
        pos2(rect.center().x + width / 2.0, rect.bottom()),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.style_mut().spacing.scroll.floating = false;
    egui::ScrollArea::vertical().id_salt(("chat-history-page", page.generation)).auto_shrink([false, false]).show(&mut child, |ui| {
        let width = ui.available_width();
        let (header, _) = ui.allocate_exact_size(vec2(width, 54.0), Sense::hover());
        paint_at(ui, header.min, "Chats", Type::sans(28.0, 30.8).weight(650.0), p.ink);
        let count = match page.total { Some(1) => "1 chat".into(), Some(n) => format!("{n} chats"), None => "Loading…".into() };
        paint_at(ui, header.min + vec2(0.0, 37.0), &count, Type::mono(12.0, 18.0), p.ink_faint);
        let button = Button::new(Kind::Primary, Size::Small, "New chat").icon(Icon::NotePencil).enabled(locked.is_none());
        let response = button.show_at(ui, Id::new("history-new"), pos2(header.right() - button.width(ui), header.bottom() - 32.0), p, images);
        if response.clicked() { event = Some(Event::NewChat); }
        if let Some(reason) = &locked { response.on_hover_text(reason); }
        ui.add_space(20.0);
        if page.total != Some(0) || !page.query.is_empty() {
            let (field, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
            let response = controls::text_field(ui, Id::new("history-search"), field, &mut page.query, "Search your chats", scale::SMALL, 36.0, FieldLook::new(p), p, "Search your chats");
            images.icon_at(ui.painter(), pos2(field.left() + 20.0, field.center().y), 16.0, Icon::MagnifyingGlass, p.ink_faint);
            if response.changed() {
                page.query = page.query.chars().take(200).collect();
                page.search(now);
                load = false;
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            ui.add_space(20.0);
        }
        let mut group = "";
        let mut first = 0;
        while first < page.items.len() {
            let label = &page.items[first].group;
            if group != label {
                if first > 0 { ui.add_space(20.0); }
                text(ui, label, Type::mono(12.0, 18.0), p.ink_faint);
                ui.add_space(8.0);
                group = label;
            }
            let last = page.items[first..].iter().position(|item| item.group != *label).map_or(page.items.len(), |offset| first + offset);
            let top = ui.cursor().min;
            let fill = ui.painter().add(egui::Shape::Noop);
            for (index, item) in page.items[first..last].iter().enumerate() {
                let (row, _) = ui.allocate_exact_size(vec2(width, 49.0), Sense::hover());
                if index > 0 { ui.painter().hline(row.x_range(), row.top(), Stroke::new(1.0, p.line)); }
                let delete = item.chat.can(|can| can.delete);
                let end = row.right() - if delete {42.0} else {16.0};
                let link = Rect::from_min_max(row.min, pos2(end, row.bottom()));
                let response = ui.interact(link, Id::new(("history-chat", item.chat.number)), Sense::click());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, &item.chat.title));
                if response.hovered() { ui.painter().rect_filled(row.shrink(1.0), 10.0, p.ink_wash(3.0)); ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                if response.has_focus() { widgets::focus_ring(ui, link, 0.0, p); }
                if response.clicked() { event = Some(Event::Open(item.chat.number)); }
                let age = paint::layout(ui.painter(), paint::job(&item.age, Type::mono(12.0, 18.0), p.ink_faint, f32::INFINITY));
                let mut left = row.left() + 16.0;
                if item.chat.state == "processing" || item.chat.state == "error" {
                    let color = if item.chat.processing() { p.blue } else { p.attention };
                    ui.painter().circle_filled(pos2(left + 3.0, row.center().y), 3.0, color);
                    left += 16.0;
                }
                let title_width = (end - left - age.size().x - 16.0).max(1.0);
                let mut job = paint::job(&item.chat.title, Type::sans(15.0, 22.5).weight(500.0), p.ink, title_width);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let title = paint::layout(ui.painter(), job);
                ui.painter().galley(pos2(left, row.center().y - title.size().y / 2.0), title, p.ink);
                ui.painter().galley(pos2(end - age.size().x - 8.0, row.center().y - age.size().y / 2.0), age, p.ink_faint);
                if delete {
                    let response = widgets::IconButton { icon: Icon::Trash, icon_size: 16.0, label: &format!("Delete {}", item.chat.title), tooltip: None,
                        color: if width < 640.0 || ui.rect_contains_pointer(row) || ui.memory(|m| m.has_focus(Id::new(("history-delete", item.chat.number)))) { p.ink_faint } else { egui::Color32::TRANSPARENT }, hover_color: p.negative(), wash: 4.0, radius: 8.0, enabled: !busy }
                        .show(ui, Id::new(("history-delete", item.chat.number)), Rect::from_center_size(pos2(row.right() - 22.0, row.center().y), vec2(32.0, 32.0)), images, p);
                    if response.clicked() { event = Some(Event::Delete(item.chat.number)); }
                }
            }
            let group_rect = Rect::from_min_max(top, pos2(top.x + width, ui.cursor().top()));
            ui.painter().set(fill, egui::Shape::rect_filled(group_rect, 14.0, p.surface));
            ui.painter().rect_stroke(group_rect, 14.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
            first = last;
        }
        if let Some(error) = &page.error {
            ui.add_space(16.0);
            text(ui, error, scale::SMALL, p.negative());
            let button = Button::new(Kind::Subtle, Size::Small, "Try again");
            let (row, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
            if button.show_at(ui, Id::new("history-retry"), row.min, p, images).clicked() { page.search(now - 0.25); load = true; }
        } else if page.loading || !page.loaded {
            text(ui, if page.query.is_empty() {"Loading chats…"} else {"Searching chats…"}, scale::SMALL, p.ink_muted);
        } else if page.items.is_empty() {
            ui.add_space(24.0);
            let (title, note) = if !page.query.is_empty() { ("No matching chats", "Try another search.") }
                else if let Some(reason) = &locked { (reason.as_str(), "") }
                else { ("No chats yet", "Ask about anything in your connected services, and the conversation shows up here.") };
            text(ui, title, scale::SMALL.weight(600.0), p.ink);
            text(ui, note, scale::SMALL, p.ink_muted);
        } else if page.next_cursor.is_some() {
            ui.add_space(16.0);
            let button = Button::new(Kind::Subtle, Size::Small, "Load more chats");
            let (row, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
            if button.show_at(ui, Id::new("history-more"), row.min, p, images).clicked() { load = true; }
        }
        if let Some(notice) = &state.notice { text(ui, notice, scale::SMALL, p.negative()); }
        ui.add_space(40.0);
    });
    (event, load)
}

fn text(ui: &mut Ui, text: &str, ty: Type, color: egui::Color32) {
    let galley = paint::layout(
        ui.painter(),
        paint::job(text, ty, color, ui.available_width()),
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

fn paint_at(ui: &Ui, at: egui::Pos2, text: &str, ty: Type, color: egui::Color32) {
    let galley = paint::layout(ui.painter(), paint::job(text, ty, color, f32::INFINITY));
    ui.painter().galley(at, galley, color);
}
