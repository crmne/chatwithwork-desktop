//! Native project navigation and editing, with organization-bound requests.
use cww::projects::{Fields, Form, Index, Page, Reply, Request};
use cww::tui::chat::Failure;

use super::state::Command;
use super::workspace::request_id;
use super::{
    controls::{self, Button, Kind, Size},
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    tokens::{Palette, Type, scale},
    widgets,
};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    List(Index),
    Detail(Page),
    Edit(Editor),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    pub form: Form,
    pub fields: Fields,
    original: Fields,
}

impl Editor {
    fn new(form: Form) -> Self {
        let project = form.project.as_ref();
        let fields = Fields {
            name: project.map_or(String::new(), |p| p.name.clone()),
            description: project
                .and_then(|p| p.description.clone())
                .unwrap_or_default(),
            all_access: project.is_some_and(|p| p.all_access),
            user_ids: form
                .people
                .iter()
                .filter(|p| p.member)
                .map(|p| p.id)
                .collect(),
        };
        Self {
            form,
            original: fields.clone(),
            fields,
        }
    }

    pub fn dirty(&self) -> bool {
        self.fields != self.original
    }

    pub fn save(&self) -> Request {
        match &self.form.project {
            Some(project) => Request::Update {
                project: project.number,
                fields: self.fields.clone(),
            },
            None => Request::Create {
                fields: self.fields.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    scroll: u64,
    pub view: Option<View>,
    pub busy: bool,
    pub error: Option<String>,
    pub confirm_close: bool,
    pub confirm: Option<Request>,
    last: Request,
    refresh: Option<Request>,
    draft: Option<Editor>,
    known_project: Option<cww::projects::Project>,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            generation: request_id(),
            scroll: request_id(),
            view: None,
            busy: false,
            error: None,
            confirm_close: false,
            confirm: None,
            last: Request::List,
            refresh: None,
            draft: None,
            known_project: None,
        }
    }
}

impl Screen {
    pub fn dirty(&self) -> bool {
        matches!(&self.view, Some(View::Edit(editor)) if editor.dirty())
            || self.draft.as_ref().is_some_and(Editor::dirty)
    }

    pub fn request(&mut self, request: Request) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        if matches!(
            request,
            Request::List
                | Request::New
                | Request::Edit { .. }
                | Request::Show { before: None, .. }
        ) {
            self.view = None;
            self.scroll = request_id();
        }
        self.last = request.clone();
        vec![Command::Projects(self.generation, request)]
    }

    pub fn reconcile_memberships(&mut self, projects: &[cww::tui::chat::Project]) -> Vec<Command> {
        if let Some(known) = &self.known_project {
            let active = projects.iter().find(|p| p.id == known.id);
            if active.is_some() == (known.member && !known.archived)
                && active.is_none_or(|p| {
                    p.name == known.name && p.hq == known.hq && p.all_access == known.all_access
                })
            {
                return vec![];
            }
        } else if !matches!(self.view, Some(View::List(_))) {
            return vec![];
        }
        self.invalidate()
    }

    /// Re-read after a live change. Hide cached content immediately and wait
    /// for the one request in flight before rechecking access. Never replay a
    /// write whose result may already be queued.
    pub fn invalidate(&mut self) -> Vec<Command> {
        let read = self.refresh.clone().unwrap_or_else(|| match &self.view {
            Some(View::Edit(editor)) => editor
                .form
                .project
                .as_ref()
                .map_or(Request::New, |p| Request::Edit { project: p.number }),
            Some(View::Detail(page)) => Request::Show {
                project: page.project.number,
                before: None,
            },
            _ => self.read_request(),
        });
        if let Some(View::Edit(editor)) = self.view.take()
            && editor.dirty()
        {
            self.draft = Some(editor);
        }
        self.confirm = None;
        if self.busy {
            self.refresh = Some(read);
            vec![]
        } else {
            self.request(read)
        }
    }

    fn read_request(&self) -> Request {
        match &self.last {
            Request::New => Request::New,
            Request::Edit { project } | Request::Update { project, .. } => {
                Request::Edit { project: *project }
            }
            Request::Show { project, .. }
            | Request::Pin { project, .. }
            | Request::Archive { project, .. }
            | Request::Membership { project, .. } => Request::Show {
                project: *project,
                before: None,
            },
            _ => Request::List,
        }
    }

    pub fn retry(&mut self) -> Vec<Command> {
        self.invalidate()
    }

    pub fn loaded(&mut self, result: Result<Reply, Failure>) -> Vec<Command> {
        self.busy = false;
        let changed = result.is_ok()
            && matches!(
                self.last,
                Request::Create { .. }
                    | Request::Update { .. }
                    | Request::Delete { .. }
                    | Request::Membership { .. }
                    | Request::Pin { .. }
                    | Request::Archive { .. }
            );
        if let Some(read) = self.refresh.take() {
            let mut commands = self.request(if matches!(result, Ok(Reply::Removed)) {
                Request::List
            } else {
                read
            });
            if changed {
                commands.push(Command::List);
            }
            return commands;
        }
        match result {
            Ok(Reply::List(index)) => {
                self.known_project = None;
                self.view = Some(View::List(index));
            }
            Ok(Reply::Form(form)) => {
                self.known_project = form.project.clone();
                let mut editor = Editor::new(form);
                if let Some(draft) = self.draft.take()
                    && draft.form.project.as_ref().map(|p| p.id)
                        == editor.form.project.as_ref().map(|p| p.id)
                {
                    editor.fields = draft.fields;
                    // A removed person cannot be granted access by a stale form.
                    editor
                        .fields
                        .user_ids
                        .retain(|id| editor.form.people.iter().any(|p| p.id == *id));
                    for person in &editor.form.people {
                        if person.fixed
                            && person.member
                            && !editor.fields.user_ids.contains(&person.id)
                        {
                            editor.fields.user_ids.push(person.id);
                        }
                    }
                }
                self.view = Some(View::Edit(editor));
            }
            Ok(Reply::Page(mut page)) => {
                self.known_project = Some(page.project.clone());
                if let Request::Show {
                    project,
                    before: Some(cursor),
                } = &self.last
                    && let Some(View::Detail(old)) = &mut self.view
                    && old.project.number == *project
                {
                    if page.next_cursor.as_ref() == Some(cursor) {
                        self.error = Some(
                            "The chat list could not continue. Open the project again.".into(),
                        );
                        old.next_cursor = None;
                        return vec![];
                    }
                    for item in page.chats.drain(..) {
                        old.chats
                            .retain(|previous| previous.chat.number != item.chat.number);
                        old.chats.push(item);
                    }
                    page.chats = std::mem::take(&mut old.chats);
                }
                if !matches!(
                    self.last,
                    Request::Show {
                        before: Some(_),
                        ..
                    }
                ) {
                    self.scroll = request_id();
                }
                self.view = Some(View::Detail(page));
            }
            Ok(Reply::Pins(_)) => {
                if let Request::Pin { project, pinned } = &self.last
                    && let Some(View::Detail(page)) = &mut self.view
                    && page.project.number == *project
                {
                    page.project.pinned = *pinned;
                }
            }
            Ok(Reply::Removed) => {
                let mut commands = self.request(Request::List);
                commands.push(Command::List);
                return commands;
            }
            Err(failure) => {
                if matches!(
                    failure.code.as_str(),
                    "forbidden"
                        | "invalid_token"
                        | "revoked"
                        | "chat_access_required"
                        | "not_paired"
                        | "not_found"
                ) {
                    self.view = None;
                    self.draft = None;
                }
                self.error = Some(failure.message);
            }
        }
        if changed { vec![Command::List] } else { vec![] }
    }
}

#[derive(Default)]
pub struct Outcome {
    pub event: Option<Event>,
    pub commands: Vec<Command>,
    pub discarded: bool,
}

pub fn show(
    ui: &mut Ui,
    rect: Rect,
    screen: &mut Screen,
    account: &str,
    p: &Palette,
    images: &Images,
) -> Outcome {
    let mut outcome = Outcome::default();
    let width = (rect.width() - if rect.width() < 640.0 { 32.0 } else { 48.0 }).clamp(1.0, 768.0);
    let bounds = Rect::from_min_max(
        pos2(
            rect.center().x - width / 2.0,
            rect.top() + if rect.width() < 768.0 { 48.0 } else { 72.0 },
        ),
        pos2(rect.center().x + width / 2.0, rect.bottom()),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds);
    child.style_mut().spacing.scroll.floating = false;
    child.style_mut().spacing.scroll.foreground_color = false;
    child.visuals_mut().widgets.inactive.bg_fill = p.line_strong;
    child.visuals_mut().widgets.hovered.bg_fill = p.ink_faint;
    child.visuals_mut().widgets.active.bg_fill = p.ink_muted;
    let busy = screen.busy;
    egui::ScrollArea::vertical().id_salt(("projects", screen.scroll)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        match &mut screen.view {
            Some(View::List(index)) => {
                outcome.event = header(ui, &index.account, "Projects", Some("Chats in a project are shared with the people on it. Everything else stays private to whoever started it."),
                    vec![(make_button("New project", Kind::Primary, !busy), Event::Projects(Request::New))], p, images);
                ui.add_space(32.0);
                if !index.others.is_empty() { section(ui, "Your projects", None, p); }
                if index.projects.is_empty() {
                    empty(ui, "No projects yet", "Start one for a team or a piece of work, then move chats into it to share them.", p);
                } else {
                    group(ui, p, |ui| {
                        for project in &index.projects {
                            let meta = format!("{} · {}", project.summary, count(project.chat_count.unwrap_or(0), "chat"));
                            let title = format!("{}{}", project.name, if project.archived { " · Archived" } else { "" });
                            if link_row(ui, ("project", project.number), &title, &meta, icon(project), !busy, p, images) {
                                outcome.event = Some(Event::Projects(Request::Show { project: project.number, before: None }));
                            }
                        }
                    });
                }
                if !index.others.is_empty() {
                    ui.add_space(32.0);
                    section(ui, if index.admin { "Projects you're not on" } else { "All-Access projects you can join" }, Some(index.others.len().to_string()), p);
                    group(ui, p, |ui| {
                        for project in &index.others {
                            let (open, join) = action_row(ui, ("other-project", project.number), &project.name, &project.summary, RowMark::Icon(icon(project)), Some(if project.all_access { "Join" } else { "Add me" }), true, !busy, p, images);
                            if open { outcome.event = Some(Event::Projects(Request::Show { project: project.number, before: None })); }
                            if join { outcome.event = Some(Event::Projects(Request::Membership { project: project.number, join: true })); }
                        }
                    });
                    ui.add_space(8.0);
                    let note = if index.admin { format!("As {} you see every project in {}. Its chats open once you're on it, and adding yourself to an invite-only project is recorded.", if index.owner { "an owner" } else { "an admin" }, account) }
                        else { format!("Everyone in {account} can join these. Their chats open once you're on them.") };
                    text(ui, &note, scale::SMALLER, p.ink_muted);
                }
            }
            Some(View::Detail(page)) => {
                let project = &page.project;
                let mut actions = vec![];
                if project.can.pin { actions.push((make_button(if project.pinned { "Unpin from sidebar" } else { "Pin to sidebar" }, Kind::Subtle, !busy), Event::Projects(Request::Pin { project: project.number, pinned: !project.pinned }))); }
                if project.can.instructions { actions.push((make_button("Instructions", Kind::Subtle, !busy), Event::ProjectInstructions(project.id))); }
                if project.can.edit { actions.push((make_button("Settings", Kind::Subtle, !busy), Event::Projects(Request::Edit { project: project.number }))); }
                if project.can.chat { actions.push((make_button("New chat here", Kind::Primary, !busy), Event::ProjectChat(project.clone()))); }
                if project.can.join { actions.push((make_button(if project.all_access { "Join" } else { "Add me" }, Kind::Primary, !busy), Event::Projects(Request::Membership { project: project.number, join: true }))); }
                outcome.event = header(ui, &project.summary, &project.name, project.description.as_deref(), actions, p, images);
                if project.archived { ui.add_space(24.0); text(ui, "This project is archived. Its chats stay readable, but no new ones start here.", scale::SMALL, p.ink_muted); }
                ui.add_space(32.0);
                section(ui, "Chats", project.chat_count.map(|n| count(n, "chat")), p);
                if !project.member {
                    empty(ui, "You're not on this project", &if project.all_access { format!("Join it to read its chats and start new ones. Everyone in {account} can.") }
                        else { format!("It's invite-only. You can see it because you're {}; add yourself to read its chats. That's recorded.", if page.owner { "an owner" } else { "an admin" }) }, p);
                } else if page.chats.is_empty() {
                    empty(ui, if project.archived { "No chats here" } else { "No chats here yet" }, if project.archived { "This project was archived before anyone chatted in it." } else { "Start a chat in this project, or move one of your private chats in from its menu in the sidebar." }, p);
                } else {
                    group(ui, p, |ui| {
                        for chat in &page.chats {
                            let meta = format!("Started by {} · {}{}{}", chat.creator, chat.age, if chat.visible_to_guests { " · Guests can read" } else { "" }, if chat.chat.processing() { " · Answering" } else { "" });
                            if link_row(ui, ("project-chat", chat.chat.number), &chat.chat.title, &meta, Icon::UserCircle, !busy, p, images) { outcome.event = Some(Event::Open(chat.chat.number)); }
                        }
                    });
                    if let Some(cursor) = &page.next_cursor {
                        ui.add_space(12.0);
                        if button(ui, "project-more", "Load more chats", Kind::Subtle, !busy, p, images) { outcome.event = Some(Event::Projects(Request::Show { project: project.number, before: Some(cursor.clone()) })); }
                    }
                }
                ui.add_space(32.0);
                section(ui, "People", Some(project.audience.clone()), p);
                group(ui, p, |ui| {
                    for person in &page.people {
                        let name = format!("{}{}", person.name, if person.me { " · you" } else { "" });
                        let (_, leave) = action_row(ui, ("project-person", person.id), &name, if person.creator { "Started this project" } else { &person.role }, RowMark::Initial(person.name.chars().next().unwrap_or('?')), (person.me && project.can.leave).then_some("Leave"), false, !busy, p, images);
                        if leave { screen.confirm = Some(Request::Membership { project: project.number, join: false }); }
                    }
                });
                ui.add_space(8.0);
                if project.hq { text(ui, &format!("Everyone who works in {account} is in HQ; guests never are."), scale::SMALLER, p.ink_muted); }
                text(ui, "Everyone here reads the project's chats. Each person's questions run with their own connected sources, and only they see what their tools returned.", scale::SMALLER, p.ink_muted);
            }
            Some(View::Edit(editor)) => {
                let project = editor.form.project.as_ref();
                let back = project.map_or(Request::List, |p| Request::Show { project: p.number, before: None });
                if back_link(ui, &format!("← {}", project.map_or("Projects", |p| p.name.as_str())), !busy, p) { outcome.event = Some(Event::Projects(back.clone())); }
                ui.add_space(6.0);
                heading(ui, if project.is_some() { "Project settings" } else { "New project" }, p);
                ui.add_space(32.0);
                group(ui, p, |ui| {
                    form_field(ui, "Name", &mut editor.fields.name, "e.g. Q4 launch", false, !busy, p);
                    form_field(ui, "Description", &mut editor.fields.description, "What it's for. Optional.", true, !busy, p);
                });
                ui.add_space(12.0);
                if project.is_none_or(|p| p.can.choose_people) {
                    group(ui, p, |ui| {
                        for (all, name, note) in [(false, "Invite-only", "Only the people on it can see it. Owners and admins can find it and add themselves.".to_owned()), (true, "All-Access", format!("Everyone in {account} can find it and join. Guests only if you add them."))] {
                            if choice_row(ui, ("project-access", all), name, &note, editor.fields.all_access == all, !busy, true, p, images) { editor.fields.all_access = all; }
                        }
                    });
                    ui.add_space(16.0);
                    group(ui, p, |ui| {
                        for person in &editor.form.people {
                            let selected = editor.fields.user_ids.contains(&person.id);
                            let note = format!("{} · {}", person.email.as_deref().unwrap_or_default(), person.role);
                            if choice_row(ui, ("project-member", person.id), &person.name, &note, selected, !busy && !person.fixed, false, p, images) {
                                if selected { editor.fields.user_ids.retain(|id| *id != person.id); } else { editor.fields.user_ids.push(person.id); }
                            }
                        }
                    });
                    ui.add_space(8.0);
                    text(ui, "Everyone on the project reads its chats and can continue them with their own connected sources. Guests only read the chats shown to them.", scale::SMALLER, p.ink_muted);
                } else {
                    empty(ui, &format!("Everyone in {account}"), "Owners, admins, and members are in HQ from the day they join. Guests never are.", p);
                }
                ui.add_space(24.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let save = if project.is_some() { "Save project" } else { "Create project" };
                    let buttons_width = make_button("Cancel", Kind::Subtle, true).width(ui) + make_button(save, Kind::Primary, true).width(ui) + 8.0;
                    ui.add_space((ui.available_width() - buttons_width).max(0.0));
                    if button(ui, "project-cancel", "Cancel", Kind::Subtle, !busy, p, images) { outcome.event = Some(Event::Projects(back)); }
                    if button(ui, "project-save", if project.is_some() { "Save project" } else { "Create project" }, Kind::Primary, !busy && !editor.fields.name.trim().is_empty(), p, images) { outcome.event = Some(Event::Projects(editor.save())); }
                });
                if let Some(project) = project && (project.can.archive || project.can.delete) {
                    ui.add_space(32.0);
                    section(ui, "Archive or delete", None, p);
                    group(ui, p, |ui| {
                        if project.can.archive {
                            let (_, archive) = action_row(ui, "project-archive", if project.archived { "Bring it back" } else { "Archive" }, if project.archived { "Start new chats here again." } else { "Keep its chats readable, but stop new ones." }, RowMark::None, Some(if project.archived { "Unarchive" } else { "Archive" }), false, !busy, p, images);
                            if archive { outcome.event = Some(Event::Projects(Request::Archive { project: project.number, archived: !project.archived })); }
                        }
                        if project.can.delete {
                            let (_, delete) = action_row(ui, "project-delete", "Delete project", "Its chats go back to the people who started them, private again.", RowMark::None, Some("Delete project"), false, !busy, p, images);
                            if delete { screen.confirm = Some(Request::Delete { project: project.number }); }
                        }
                    });
                }
            }
            None => { heading(ui, "Projects", p); }
        }
        if busy { ui.add_space(16.0); text(ui, "Loading…", scale::SMALL, p.ink_muted); }
        if let Some(error) = &screen.error {
            ui.add_space(16.0);
            text(ui, error, scale::SMALL, p.negative());
            if button(ui, "project-retry", "Try again", Kind::Subtle, !busy, p, images) { outcome.commands.extend(screen.retry()); }
            if button(ui, "project-error-back", "All projects", Kind::Subtle, !busy, p, images) { outcome.event = Some(Event::Projects(Request::List)); }
        }
        ui.add_space(64.0);
    });
    if screen.confirm_close {
        if let Some(confirmed) = confirmation(
            ui,
            "Discard unsaved project changes?",
            "Your saved project will stay as it is.",
            "Discard changes",
            p,
            images,
        ) {
            screen.confirm_close = false;
            outcome.discarded = confirmed;
        }
    } else if let Some(request) = screen.confirm.clone() {
        let (title, note, action) = if matches!(request, Request::Delete { .. }) {
            (
                "Delete project?",
                "Its chats become private to whoever started them.",
                "Delete",
            )
        } else {
            (
                "Leave project?",
                "You'll stop seeing its chats. You may need an invitation to join again.",
                "Leave",
            )
        };
        if let Some(confirmed) = confirmation(ui, title, note, action, p, images) {
            screen.confirm = None;
            if confirmed {
                outcome.event = Some(Event::Projects(request));
            }
        }
    }
    outcome
}

fn count(n: u64, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}
fn icon(project: &cww::projects::Project) -> Icon {
    if project.hq {
        Icon::Buildings
    } else if project.all_access {
        Icon::UsersThree
    } else {
        Icon::FolderSimple
    }
}
fn heading(ui: &mut Ui, label: &str, p: &Palette) {
    text(ui, label, Type::sans(24.0, 32.0).weight(600.0), p.ink);
}
fn text(ui: &mut Ui, label: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(label, ty, color, ui.available_width())).wrap());
}
fn section(ui: &mut Ui, label: &str, count: Option<String>, p: &Palette) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.5), Sense::hover());
    let heading = paint::layout(
        ui.painter(),
        paint::job(
            label,
            Type::sans(15.0, 22.5).weight(600.0),
            p.ink,
            row.width(),
        ),
    );
    widgets::label(ui, Rect::from_min_size(row.min, heading.size()), label);
    ui.painter().galley(row.min, heading, p.ink);
    if let Some(count) = count {
        let galley = paint::layout(
            ui.painter(),
            paint::job(&count, scale::MICRO, p.ink_faint, row.width()),
        );
        ui.painter().galley(
            pos2(
                row.right() - galley.size().x,
                row.bottom() - galley.size().y,
            ),
            galley,
            p.ink_faint,
        );
    }
    ui.add_space(12.0);
}

fn empty(ui: &mut Ui, title: &str, note: &str, p: &Palette) {
    ui.add_space(16.0);
    text(ui, title, scale::SMALL.weight(600.0), p.ink);
    ui.add_space(8.0);
    text(ui, note, scale::SMALL, p.ink_muted);
    ui.add_space(16.0);
}
fn group(ui: &mut Ui, p: &Palette, contents: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().min;
    let width = ui.available_width();
    let bg = ui.painter().add(egui::Shape::Noop);
    ui.data_mut(|data| data.insert_temp(Id::new("project-group-top"), top.y));
    contents(ui);
    let rect = Rect::from_min_max(top, pos2(top.x + width, ui.cursor().top()));
    ui.painter()
        .set(bg, egui::Shape::rect_filled(rect, 14.0, p.surface));
    ui.painter().rect_stroke(
        rect,
        14.0,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
}
fn button(
    ui: &mut Ui,
    id: impl egui::AsId,
    label: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let b = make_button(label, kind, enabled);
    let (r, _) = ui.allocate_exact_size(vec2(b.width(ui), b.height()), Sense::hover());
    b.show_at(ui, Id::new(id), r.min, p, images).clicked()
}
#[allow(clippy::too_many_arguments)]
fn link_row(
    ui: &mut Ui,
    id: impl egui::AsId,
    title: &str,
    note: &str,
    icon: Icon,
    clickable: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let mark = if matches!(icon, Icon::UserCircle) {
        RowMark::Initial(
            note.strip_prefix("Started by ")
                .unwrap_or(title)
                .chars()
                .next()
                .unwrap_or('?'),
        )
    } else {
        RowMark::Icon(icon)
    };
    action_row(ui, id, title, note, mark, None, clickable, true, p, images).0
}

#[derive(Clone, Copy)]
enum RowMark {
    Icon(Icon),
    Initial(char),
    None,
}

#[allow(clippy::too_many_arguments)]
fn action_row(
    ui: &mut Ui,
    id: impl egui::AsId,
    title: &str,
    note: &str,
    mark: RowMark,
    action: Option<&str>,
    clickable: bool,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> (bool, bool) {
    let id = Id::new(id);
    let width = ui.available_width();
    let button = action.map(|label| make_button(label, Kind::Subtle, enabled));
    let narrow = width < 640.0;
    let action_width = button.as_ref().map_or(0.0, |b| b.width(ui) + 16.0);
    let text_left = if matches!(mark, RowMark::None) {
        16.0
    } else {
        68.0
    };
    let text_width = width - text_left - 20.0 - if narrow { 0.0 } else { action_width };
    let title_g = paint::layout(
        ui.painter(),
        paint::job(title, scale::SMALL.weight(550.0), p.ink, text_width),
    );
    let note_g = paint::layout(
        ui.painter(),
        paint::job(note, scale::MICRO, p.ink_faint, text_width),
    );
    let height = (title_g.size().y + note_g.size().y + 30.0).max(68.0);
    let (row, _) = ui.allocate_exact_size(
        vec2(
            width,
            height
                + if narrow && button.is_some() {
                    44.0
                } else {
                    0.0
                },
        ),
        Sense::hover(),
    );
    let response = ui.interact(
        Rect::from_min_size(
            row.min,
            vec2(width - if narrow { 0.0 } else { action_width }, height),
        ),
        id,
        if clickable && enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Link, clickable && enabled, title)
    });
    if clickable && enabled && response.hovered() {
        ui.painter()
            .rect_filled(row.shrink(1.0), 12.0, p.ink_wash(3.0));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mark_rect = Rect::from_center_size(
        pos2(row.left() + 34.0, row.top() + height / 2.0),
        vec2(36.0, 36.0),
    );
    match mark {
        RowMark::Icon(icon) => {
            ui.painter().rect_filled(mark_rect, 10.0, p.canvas);
            ui.painter().rect_stroke(
                mark_rect,
                10.0,
                Stroke::new(1.0, p.line),
                egui::StrokeKind::Inside,
            );
            images.icon_at(ui.painter(), mark_rect.center(), 16.0, icon, p.ink);
        }
        RowMark::Initial(initial) => {
            avatar(ui, mark_rect.center(), initial, p);
        }
        RowMark::None => {}
    }
    let h = title_g.size().y;
    ui.painter()
        .galley(row.min + vec2(text_left, 14.0), title_g, p.ink);
    ui.painter()
        .galley(row.min + vec2(text_left, 16.0 + h), note_g, p.ink_faint);
    if ui
        .data(|data| data.get_temp::<f32>(Id::new("project-group-top")))
        .is_some_and(|top| row.top() > top + 1.0)
    {
        ui.painter()
            .hline(row.x_range(), row.top(), Stroke::new(1.0, p.line));
    }
    if clickable && matches!(mark, RowMark::Icon(_)) && action.is_none() {
        images.icon_at(
            ui.painter(),
            pos2(row.right() - 24.0, row.top() + height / 2.0),
            16.0,
            Icon::CaretRight,
            p.ink_faint,
        );
    }
    if response.has_focus() {
        widgets::focus_ring(ui, row, 0.0, p);
    }
    let acted = button.is_some_and(|b| {
        b.show_at(
            ui,
            id.with("action"),
            pos2(
                row.right() - 16.0 - b.width(ui),
                if narrow {
                    row.bottom() - 14.0 - b.height()
                } else {
                    row.top() + (height - b.height()) / 2.0
                },
            ),
            p,
            images,
        )
        .clicked()
    });
    (response.clicked(), acted)
}

#[allow(clippy::too_many_arguments)]
fn choice_row(
    ui: &mut Ui,
    id: impl egui::AsId,
    title: &str,
    note: &str,
    selected: bool,
    enabled: bool,
    radio: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let width = ui.available_width();
    let text_left = if radio { 52.0 } else { 100.0 };
    let title_g = paint::layout(
        ui.painter(),
        paint::job(
            title,
            scale::SMALL.weight(550.0),
            p.ink,
            width - text_left - 16.0,
        ),
    );
    let note_g = paint::layout(
        ui.painter(),
        paint::job(
            note,
            if radio { scale::SMALLER } else { scale::MICRO },
            p.ink_muted,
            width - text_left - 16.0,
        ),
    );
    let (row, _) = ui.allocate_exact_size(
        vec2(width, title_g.size().y + note_g.size().y + 32.0),
        Sense::hover(),
    );
    let response = ui.interact(
        row,
        Id::new(id),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            if radio {
                egui::WidgetType::RadioButton
            } else {
                egui::WidgetType::Checkbox
            },
            enabled,
            selected,
            title,
        )
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            row.shrink(1.0),
            12.0,
            p.ink_wash(if selected { 4.5 } else { 3.0 }),
        );
    }
    let mark = Rect::from_center_size(pos2(row.left() + 26.0, row.center().y), vec2(18.0, 18.0));
    ui.painter().rect_stroke(
        mark,
        if radio { 9.0 } else { 4.0 },
        Stroke::new(1.0, p.line_strong),
        egui::StrokeKind::Inside,
    );
    if selected {
        if radio {
            ui.painter().circle_filled(mark.center(), 3.5, p.ink);
        } else {
            images.icon_at(ui.painter(), mark.center(), 14.0, Icon::Check, p.ink);
        }
    }
    if !radio {
        avatar(
            ui,
            pos2(row.left() + 68.0, row.center().y),
            title.chars().next().unwrap_or('?'),
            p,
        );
    }
    if ui
        .data(|data| data.get_temp::<f32>(Id::new("project-group-top")))
        .is_some_and(|top| row.top() > top + 1.0)
    {
        ui.painter()
            .hline(row.x_range(), row.top(), Stroke::new(1.0, p.line));
    }
    let h = title_g.size().y;
    ui.painter()
        .galley(row.min + vec2(text_left, 14.0), title_g, p.ink);
    ui.painter()
        .galley(row.min + vec2(text_left, 18.0 + h), note_g, p.ink_muted);
    if response.has_focus() {
        widgets::focus_ring(ui, row, 0.0, p);
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}
fn confirmation(
    ui: &mut Ui,
    title: &str,
    note: &str,
    action: &str,
    p: &Palette,
    images: &Images,
) -> Option<bool> {
    let mut answer = None;
    let open = controls::modal(
        ui.ctx(),
        Id::new("project-confirm"),
        420.0,
        190.0,
        p,
        |ui, rect| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            text(
                &mut child,
                title,
                Type::sans(18.0, 27.0).weight(600.0),
                p.ink,
            );
            child.add_space(12.0);
            text(&mut child, note, scale::SMALL, p.ink_muted);
            child.add_space(20.0);
            child.horizontal(|ui| {
                if button(
                    ui,
                    "project-confirm-cancel",
                    if action == "Discard changes" {
                        "Keep editing"
                    } else {
                        "Cancel project action"
                    },
                    Kind::Subtle,
                    true,
                    p,
                    images,
                ) {
                    answer = Some(false);
                }
                if button(
                    ui,
                    "project-confirm-yes",
                    action,
                    Kind::Error,
                    true,
                    p,
                    images,
                ) {
                    answer = Some(true);
                }
            });
        },
    );
    if open { answer } else { Some(false) }
}

fn avatar(ui: &Ui, center: egui::Pos2, initial: char, p: &Palette) {
    ui.painter().circle_filled(center, 16.0, p.canvas);
    ui.painter()
        .circle_stroke(center, 16.0, Stroke::new(1.0, p.line));
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        initial.to_uppercase().to_string(),
        scale::SMALL.font_id(),
        p.ink_muted,
    );
}

fn make_button(label: &str, kind: Kind, enabled: bool) -> Button<'_> {
    let b = Button::new(kind, Size::Small, label).enabled(enabled);
    match label {
        "New project" => b.icon(Icon::Plus),
        "New chat here" => b.icon(Icon::NotePencil),
        "Settings" => b.icon(Icon::GearSix),
        "Instructions" => b.icon(Icon::Notebook),
        "Pin to sidebar" => Button::new(Kind::Subtle, Size::Small, "")
            .icon(Icon::PushPin)
            .name(label)
            .enabled(enabled),
        "Unpin from sidebar" => Button::new(Kind::Subtle, Size::Small, "")
            .icon(Icon::PushPinSlash)
            .name(label)
            .enabled(enabled),
        "Delete project" => Button::new(Kind::Error, Size::Small, "Delete")
            .name(label)
            .enabled(enabled),
        "Cancel project action" => Button::new(kind, Size::Small, "Cancel")
            .name(label)
            .enabled(enabled),
        _ => b,
    }
}

fn back_link(ui: &mut Ui, label: &str, enabled: bool, p: &Palette) -> bool {
    let response = ui.add_enabled(
        enabled,
        egui::Link::new(
            egui::RichText::new(label)
                .font(scale::MICRO.font_id())
                .color(p.ink_faint),
        ),
    );
    response.clicked()
}

fn header(
    ui: &mut Ui,
    kicker: &str,
    title: &str,
    lede: Option<&str>,
    actions: Vec<(Button<'_>, Event)>,
    p: &Palette,
    images: &Images,
) -> Option<Event> {
    let width = ui.available_width();
    let kicker_g = paint::layout(
        ui.painter(),
        paint::job(kicker, scale::MICRO, p.ink_faint, width),
    );
    let title_g = paint::layout(
        ui.painter(),
        paint::job(title, Type::sans(24.0, 32.0).weight(600.0), p.ink, width),
    );
    let lede_g = lede.filter(|s| !s.is_empty()).map(|s| {
        paint::layout(
            ui.painter(),
            paint::job(s, Type::sans(15.0, 22.5), p.ink_muted, width.min(608.0)),
        )
    });
    let heading_width = kicker_g
        .size()
        .x
        .max(title_g.size().x)
        .max(lede_g.as_ref().map_or(0.0, |g| g.size().x));
    let heading_height = kicker_g.size().y
        + 6.0
        + title_g.size().y
        + lede_g.as_ref().map_or(0.0, |g| g.size().y + 6.0);
    let mut lines: Vec<Vec<(Button<'_>, Event)>> = vec![vec![]];
    let mut line_width = 0.0;
    for action in actions {
        let w = action.0.width(ui);
        if line_width > 0.0 && line_width + w > width {
            lines.push(vec![]);
            line_width = 0.0;
        }
        line_width += w + 8.0;
        lines.last_mut().unwrap().push(action);
    }
    let actions_width = (line_width - 8.0).max(0.0);
    let inline = lines.len() == 1 && heading_width + 16.0 + actions_width <= width;
    let action_height = lines.len() as f32 * 40.0 - 8.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            width,
            if inline {
                heading_height.max(action_height)
            } else {
                heading_height + 16.0 + action_height
            },
        ),
        Sense::hover(),
    );
    let mut y = rect.top();
    for (g, label, color) in [
        (Some(kicker_g), kicker, p.ink_faint),
        (Some(title_g), title, p.ink),
        (lede_g, lede.unwrap_or_default(), p.ink_muted),
    ] {
        if let Some(g) = g {
            widgets::label(
                ui,
                Rect::from_min_size(pos2(rect.left(), y), g.size()),
                label,
            );
            let height = g.size().y;
            ui.painter().galley(pos2(rect.left(), y), g, color);
            y += height + 6.0;
        }
    }
    let mut event = None;
    let mut y = if inline {
        rect.bottom() - action_height
    } else {
        rect.top() + heading_height + 16.0
    };
    for line in lines {
        let mut x = if inline {
            rect.right() - actions_width
        } else {
            rect.left()
        };
        for (b, action) in line {
            if b.show_at(
                ui,
                Id::new(("project-header", b.name.unwrap_or(b.label))),
                pos2(x, y),
                p,
                images,
            )
            .clicked()
            {
                event = Some(action);
            }
            x += b.width(ui) + 8.0;
        }
        y += 40.0;
    }
    event
}

fn form_field(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    placeholder: &str,
    multiline: bool,
    enabled: bool,
    p: &Palette,
) {
    let width = ui.available_width();
    let wide = width >= 640.0;
    let control_height = if multiline { 80.0 } else { 40.0 };
    let (row, _) = ui.allocate_exact_size(
        vec2(width, 28.0 + control_height + if wide { 0.0 } else { 29.0 }),
        Sense::hover(),
    );
    let label_pos = pos2(
        row.left() + 16.0,
        row.top()
            + if wide {
                (row.height() - 21.0) / 2.0
            } else {
                14.0
            },
    );
    let label_g = paint::layout(
        ui.painter(),
        paint::job(label, scale::SMALL.weight(550.0), p.ink, 208.0),
    );
    widgets::label(ui, Rect::from_min_size(label_pos, label_g.size()), label);
    ui.painter().galley(label_pos, label_g, p.ink);
    let field = Rect::from_min_max(
        pos2(
            row.left() + if wide { 248.0 } else { 16.0 },
            row.top() + if wide { 14.0 } else { 43.0 },
        ),
        pos2(row.right() - 16.0, row.bottom() - 14.0),
    );
    if multiline {
        let bg = ui.painter().add(egui::Shape::Noop);
        let inner = field.shrink2(vec2(12.0, 10.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        let response = child.add_enabled(
            enabled,
            egui::TextEdit::multiline(value)
                .id(Id::new("project-description"))
                .font(scale::SMALL.font_id())
                .text_color(p.ink)
                .frame(egui::Frame::NONE)
                .desired_width(inner.width())
                .min_size(inner.size())
                .char_limit(1000)
                .hint_text(
                    egui::RichText::new(placeholder)
                        .font(scale::SMALL.font_id())
                        .color(p.ink_faint),
                ),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, enabled, "Project description")
        });
        ui.painter()
            .set(bg, egui::Shape::rect_filled(field, 10.0, p.surface));
        ui.painter().rect_stroke(
            field,
            10.0,
            Stroke::new(1.0, p.line_strong),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            widgets::focus_ring(ui, field, 0.0, p);
        }
    } else {
        ui.add_enabled_ui(enabled, |ui| {
            controls::text_field(
                ui,
                Id::new("project-name"),
                field,
                value,
                placeholder,
                scale::SMALL,
                12.0,
                controls::FieldLook::new(p),
                p,
                "Project name",
            );
        });
        *value = value.chars().take(100).collect();
        ui.painter().hline(
            (row.left() + 1.0)..=(row.right() - 1.0),
            row.bottom(),
            Stroke::new(1.0, p.line),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cww::projects::{Can, Person, Project};

    fn project() -> Project {
        Project {
            id: 81,
            number: 17,
            name: "Synthetic team".into(),
            member: true,
            can: Can {
                chat: true,
                edit: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn queued_page_cannot_restore_content_after_access_changes() {
        let mut screen = Screen::default();
        let request = Request::Show {
            project: 17,
            before: None,
        };
        screen.request(request.clone());
        screen.loaded(Ok(Reply::Page(Page {
            project: project(),
            ..Default::default()
        })));
        screen.request(Request::Show {
            project: 17,
            before: Some("older".into()),
        });
        assert!(
            screen.invalidate().is_empty(),
            "wait for the one in-flight request"
        );
        assert!(screen.view.is_none());
        let commands = screen.loaded(Ok(Reply::Page(Page {
            project: project(),
            ..Default::default()
        })));
        assert_eq!(
            commands,
            vec![Command::Projects(screen.generation, request)]
        );
        assert!(
            screen.view.is_none(),
            "discard the stale response before refreshing"
        );
        screen.loaded(Err(Failure::new("not_found", "No longer available")));
        assert!(screen.view.is_none() && !screen.busy);
    }

    #[test]
    fn lost_write_response_is_reconciled_without_repeating_the_write() {
        let mut screen = Screen::default();
        screen.request(Request::Create {
            fields: Fields {
                name: "Launch".into(),
                ..Default::default()
            },
        });
        screen.loaded(Err(Failure::new("connection", "Connection closed")));
        assert_eq!(
            screen.retry(),
            vec![Command::Projects(screen.generation, Request::List)]
        );
    }

    #[test]
    fn revalidation_preserves_unsaved_fields_only_while_editing_is_allowed() {
        let mut screen = Screen::default();
        let form = Form {
            project: Some(project()),
            people: vec![Person {
                id: 1,
                member: true,
                fixed: true,
                ..Default::default()
            }],
        };
        screen.request(Request::Edit { project: 17 });
        screen.loaded(Ok(Reply::Form(form.clone())));
        let Some(View::Edit(editor)) = &mut screen.view else {
            panic!()
        };
        editor.fields.name = "Unsaved name".into();
        editor.fields.user_ids.push(999);
        screen.invalidate();
        screen.loaded(Ok(Reply::Form(form)));
        let Some(View::Edit(editor)) = &screen.view else {
            panic!()
        };
        assert_eq!(editor.fields.name, "Unsaved name");
        assert_eq!(editor.fields.user_ids, vec![1]);
        assert!(editor.dirty());
        screen.invalidate();
        screen.loaded(Err(Failure::new("forbidden", "Editing removed")));
        assert!(screen.view.is_none() && screen.draft.is_none());
    }

    #[test]
    fn successful_project_changes_refresh_the_sidebar() {
        let mut screen = Screen::default();
        screen.request(Request::Membership {
            project: 17,
            join: true,
        });
        assert_eq!(
            screen.loaded(Ok(Reply::Page(Page {
                project: project(),
                ..Default::default()
            }))),
            vec![Command::List]
        );
        screen.request(Request::Delete { project: 17 });
        let commands = screen.loaded(Ok(Reply::Removed));
        assert!(commands.contains(&Command::List));
        assert!(commands.contains(&Command::Projects(screen.generation, Request::List)));
    }
}
