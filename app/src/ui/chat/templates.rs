//! The website's Office template rows, with native file pickers and downloads.
use super::{
    controls::{self, Button, Kind, Size},
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    state::Command,
    tokens::{Palette, Type, scale},
    workspace::request_id,
};
use cww::{
    templates::{File, Page, Request, Target},
    tui::chat::{Failure, SettingsLink},
};
use egui::{Id, Rect, Ui, pos2, vec2};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    account: String,
    pub project: Option<u64>,
    pub page: Option<Page>,
    pub busy: bool,
    pub error: Option<String>,
    pub removing: Option<(u64, String)>,
    picking: Option<Target>,
    refresh: bool,
}
impl Screen {
    pub fn new(account: String, project: Option<u64>) -> Self {
        Self {
            generation: request_id(),
            account,
            project,
            page: None,
            busy: false,
            error: None,
            removing: None,
            picking: None,
            refresh: false,
        }
    }
    pub fn load(&mut self) -> Vec<Command> {
        self.request(Request::List {
            project: self.project,
        })
    }
    fn request(&mut self, request: Request) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        vec![Command::Templates(self.generation, request)]
    }
    /// Hide cached rows on access changes. Finish the one request already in
    /// flight, then re-read; never replay a write or upload a delayed selection.
    pub fn invalidate(&mut self) -> Vec<Command> {
        self.page = None;
        self.removing = None;
        if self.busy {
            self.refresh = true;
            vec![]
        } else {
            self.load()
        }
    }
    pub fn loaded(&mut self, result: Result<Page, Failure>) -> Vec<Command> {
        self.busy = false;
        self.picking = None;
        if std::mem::take(&mut self.refresh) {
            return self.load();
        }
        match result {
            Ok(page)
                if page.account.id == self.account
                    && page.project.as_ref().map(|p| p.number) == self.project =>
            {
                self.page = Some(page);
                self.error = None;
            }
            Ok(_) => {
                self.page = None;
                self.error =
                    Some("Your organization or project changed. Open templates again.".into());
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
                        | "stale_account"
                ) {
                    self.page = None;
                }
                self.error = Some(error.message);
            }
        }
        vec![]
    }
    fn pick(&mut self, target: Target, extensions: Vec<String>) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        self.picking = Some(target);
        vec![Command::PickTemplate(self.generation, extensions)]
    }
    pub fn picked(&mut self, result: Result<Option<std::path::PathBuf>, Failure>) -> Vec<Command> {
        let Some(target) = self.picking.take() else {
            return vec![];
        };
        if std::mem::take(&mut self.refresh) {
            self.busy = false;
            return self.load();
        }
        match result {
            Ok(Some(path)) => vec![Command::UploadTemplate(self.generation, target, path)],
            other => {
                self.busy = false;
                if let Err(error) = other {
                    self.error = Some(error.message);
                }
                vec![]
            }
        }
    }
}
#[derive(Default)]
pub struct Outcome {
    pub commands: Vec<Command>,
    pub event: Option<Event>,
    pub download: Option<File>,
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
    let header = screen
        .page
        .as_ref()
        .map(|page| format!("{} · {}", page.email, page.account.name));
    let event = super::settings::Shell {
        rect,
        generation: screen.generation,
        header: header.as_deref(),
        active: "templates",
        tabs,
        busy: screen.busy,
        palette: p,
        images,
    }
    .show(ui, |ui| content(ui, screen, p, images, &mut outcome));
    if event.is_some() {
        outcome.event = event;
    }
    if let Some((id, label)) = screen.removing.clone() {
        let mut cancel = false;
        let mut remove = false;
        let open = controls::modal(
            ui.ctx(),
            Id::new("remove-template"),
            420.0,
            204.0,
            p,
            |ui, rect| {
                let height = controls::dialog_header(
                    ui,
                    rect.min,
                    rect.width(),
                    &format!("Remove the {} template?", label.to_lowercase()),
                    &[],
                    p,
                );
                let at = pos2(rect.left(), rect.top() + height + 24.0);
                let button = Button::new(Kind::Subtle, Size::Small, "Cancel");
                cancel = button
                    .show_at(ui, Id::new("cancel-template-removal"), at, p, images)
                    .clicked();
                remove = Button::new(Kind::SubtleNegative, Size::Small, "Remove template")
                    .show_at(
                        ui,
                        Id::new("confirm-template-removal"),
                        pos2(at.x + button.width(ui) + 8.0, at.y),
                        p,
                        images,
                    )
                    .clicked();
            },
        );
        if !open || cancel || remove {
            screen.removing = None;
        }
        if remove {
            outcome.commands.extend(screen.request(Request::Remove {
                account: screen.account.clone(),
                project: screen.project,
                template: id,
            }));
        }
    }
    outcome
}
fn content(ui: &mut Ui, screen: &mut Screen, p: &Palette, images: &Images, out: &mut Outcome) {
    ui.set_width(ui.available_width());
    let Some(page) = screen.page.clone() else {
        text(ui, "Templates", Type::sans(15.0, 22.5).weight(600.0), p.ink);
        ui.add_space(8.0);
        text(
            ui,
            screen.error.as_deref().unwrap_or(if screen.busy {
                "Loading templates…"
            } else {
                "Templates could not be loaded."
            }),
            scale::SMALL,
            p.ink_muted,
        );
        if !screen.busy
            && button(
                ui,
                "Try again",
                "Retry templates",
                Kind::Subtle,
                true,
                p,
                images,
            )
        {
            out.commands.extend(screen.load());
        }
        return;
    };
    if let Some(project) = &page.project {
        if button(
            ui,
            &project.name,
            "Back to project",
            Kind::Subtle,
            !screen.busy,
            p,
            images,
        ) {
            out.event = Some(Event::Projects(cww::projects::Request::Show {
                project: project.number,
                before: None,
            }));
        }
        ui.add_space(16.0);
    }
    text(ui, "Templates", Type::sans(15.0, 22.5).weight(600.0), p.ink);
    ui.add_space(4.0);
    let intro = if page.project.is_some() {
        format!(
            "The PowerPoint, Word, and Excel files this project's decks, documents, and spreadsheets are built on, like a client's brand. They win over each person's and {}'s.",
            page.account.name
        )
    } else {
        format!(
            "Your brand's PowerPoint, Word, and Excel files. When the assistant makes a deck, a document, or a spreadsheet, it builds on the template: its slide layouts, styles, fonts, colors, and logos. A project's template wins, then yours, then {}'s.",
            page.account.name
        )
    };
    text(ui, &intro, scale::SMALL, p.ink_muted);
    if let Some(error) = &screen.error {
        ui.add_space(12.0);
        text(ui, error, scale::SMALL, p.negative_ink());
    }
    if let Some(notice) = &page.notice {
        ui.add_space(12.0);
        text(ui, notice, scale::SMALL, p.ink);
    }
    if screen.busy {
        ui.add_space(12.0);
        text(ui, "Updating templates…", scale::SMALL, p.ink_muted);
    }
    for group in &page.groups {
        ui.add_space(48.0);
        text(ui, &group.name, Type::sans(15.0, 22.5).weight(600.0), p.ink);
        ui.add_space(4.0);
        let help = match group.layer {
            cww::workspace::Layer::Personal => "Used in every chat you start or reply to here, unless the chat's project has its own.".into(),
            cww::workspace::Layer::Organization => format!("Used across {} where neither the project nor the person has one. {}", page.account.name, if group.editable { "Owners and admins change them." } else { "Owners and admins change them; you can download them." }),
            cww::workspace::Layer::Project => if group.editable { "Everyone on the project but guests can change these templates." } else { "This project's templates are read-only." }.into(),
        };
        text(ui, &help, scale::SMALL, p.ink_muted);
        ui.add_space(16.0);
        egui::Frame::new()
            .fill(p.surface)
            .stroke(egui::Stroke::new(1.0, p.line))
            .corner_radius(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, template) in group.templates.iter().enumerate() {
                    if index > 0 {
                        let (r, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 1.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().line_segment(
                            [r.left_top(), r.right_top()],
                            egui::Stroke::new(1.0, p.line),
                        );
                    }
                    egui::Frame::new().inner_margin(16).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let wide = ui.available_width() >= 560.0;
                        let action_labels: Vec<&str> =
                            match (template.file.is_some(), group.editable) {
                                (true, true) => vec!["Download", "Replace", "Remove"],
                                (true, false) => vec!["Download"],
                                (false, true) => vec!["Upload"],
                                (false, false) => vec![],
                            };
                        let actions_width: f32 = action_labels
                            .iter()
                            .map(|label| Button::new(Kind::Subtle, Size::Small, label).width(ui))
                            .sum::<f32>()
                            + 8.0 * action_labels.len().saturating_sub(1) as f32;
                        let body = |ui: &mut Ui| {
                            ui.horizontal_top(|ui| {
                                let (icon, _) =
                                    ui.allocate_exact_size(vec2(36.0, 36.0), egui::Sense::hover());
                                ui.painter().rect_filled(icon, 10.0, p.canvas);
                                ui.painter().rect_stroke(
                                    icon,
                                    10.0,
                                    egui::Stroke::new(1.0, p.line),
                                    egui::StrokeKind::Inside,
                                );
                                images.icon_at(
                                    ui.painter(),
                                    icon.center(),
                                    20.0,
                                    match template.kind.as_str() {
                                        "presentation" => Icon::FilePpt,
                                        "spreadsheet" => Icon::FileXls,
                                        _ => Icon::FileDoc,
                                    },
                                    p.ink,
                                );
                                ui.add_space(16.0);
                                ui.vertical(|ui| {
                                    text(ui, &template.label, scale::SMALL.weight(550.0), p.ink);
                                    match &template.file {
                                        Some(file) => {
                                            text(ui, &file.name, scale::SMALL, p.ink_muted);
                                            text(ui, &file.detail, scale::MICRO, p.ink_faint);
                                        }
                                        None => text(
                                            ui,
                                            &format!(
                                                "None: {} files use the assistant's own designs.",
                                                template.app
                                            ),
                                            scale::SMALL,
                                            p.ink_muted,
                                        ),
                                    }
                                });
                            });
                        };
                        let mut actions = |ui: &mut Ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.horizontal_wrapped(|ui| {
                                if let Some(file) = &template.file
                                    && button(
                                        ui,
                                        "Download",
                                        &format!(
                                            "Download {} {} template",
                                            group.layer.path(),
                                            template.label.to_lowercase()
                                        ),
                                        Kind::Subtle,
                                        !screen.busy,
                                        p,
                                        images,
                                    )
                                {
                                    out.download = Some(file.clone());
                                }
                                if group.editable {
                                    let label = if template.file.is_some() {
                                        "Replace"
                                    } else {
                                        "Upload"
                                    };
                                    if button(
                                        ui,
                                        label,
                                        &format!(
                                            "{label} {} {} template",
                                            group.layer.path(),
                                            template.label.to_lowercase()
                                        ),
                                        Kind::Subtle,
                                        !screen.busy,
                                        p,
                                        images,
                                    ) {
                                        out.commands.extend(screen.pick(
                                            Target {
                                                account: page.account.id.clone(),
                                                project: screen.project,
                                                layer: group.layer,
                                                kind: template.kind.clone(),
                                            },
                                            template.extensions.clone(),
                                        ));
                                    }
                                    if let Some(file) = &template.file
                                        && button(
                                            ui,
                                            "Remove",
                                            &format!(
                                                "Remove {} {} template",
                                                group.layer.path(),
                                                template.label.to_lowercase()
                                            ),
                                            Kind::SubtleNegative,
                                            !screen.busy,
                                            p,
                                            images,
                                        )
                                    {
                                        screen.removing = Some((file.id, template.label.clone()));
                                    }
                                }
                            });
                        };
                        if wide {
                            ui.horizontal_top(|ui| {
                                let width = (ui.available_width() - actions_width - 16.0).max(1.0);
                                ui.allocate_ui_with_layout(
                                    vec2(width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(width);
                                        body(ui);
                                    },
                                );
                                ui.add_space(16.0);
                                ui.vertical(&mut actions);
                            });
                        } else {
                            body(ui);
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.add_space((ui.available_width() - actions_width).max(0.0));
                                actions(ui);
                            });
                        }
                    });
                }
            });
    }
}
fn text(ui: &mut Ui, label: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(label, ty, color, ui.available_width())).wrap());
}
#[allow(clippy::too_many_arguments)]
fn button(
    ui: &mut Ui,
    label: &str,
    name: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let button = Button::new(kind, Size::Small, label)
        .name(name)
        .enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(
        vec2(button.width(ui), button.height()),
        egui::Sense::hover(),
    );
    button
        .show(ui, ui.id().with(name), rect, p, images)
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page(account: &str, project: Option<u64>) -> Page {
        Page {
            account: cww::tui::chat::Named {
                id: account.into(),
                name: "Northwind".into(),
                ..Default::default()
            },
            email: "alex@example.com".into(),
            project: project.map(|number| cww::templates::Project {
                id: 999,
                number,
                name: "Launch".into(),
            }),
            groups: vec![],
            notice: None,
        }
    }
    #[test]
    fn failed_changes_keep_the_current_page_but_revoked_access_and_wrong_scopes_clear_it() {
        let mut screen = Screen::new("482139075".into(), Some(7));
        screen.loaded(Ok(page("482139075", Some(7))));
        screen.loaded(Err(Failure::new(
            "invalid",
            "Use a macro-free Office file.",
        )));
        assert!(screen.page.is_some());
        assert_eq!(
            screen.error.as_deref(),
            Some("Use a macro-free Office file.")
        );
        screen.loaded(Ok(page("482139075", Some(8))));
        assert!(screen.page.is_none());
        screen.loaded(Ok(page("123456789", Some(7))));
        assert!(screen.page.is_none());
        for code in ["forbidden", "not_found", "stale_account", "revoked"] {
            screen.loaded(Ok(page("482139075", Some(7))));
            screen.loaded(Err(Failure::new(code, "No access")));
            assert!(screen.page.is_none());
        }
    }
    #[test]
    fn access_changes_hide_cached_rows_and_recheck_after_pending_work_without_replaying_it() {
        let mut screen = Screen::new("482139075".into(), Some(7));
        screen.loaded(Ok(page("482139075", Some(7))));
        screen.pick(
            Target {
                account: "482139075".into(),
                project: Some(7),
                layer: cww::workspace::Layer::Project,
                kind: "document".into(),
            },
            vec!["docx".into()],
        );
        assert!(screen.invalidate().is_empty());
        assert!(screen.page.is_none());
        assert_eq!(
            screen.picked(Ok(Some("old.docx".into()))),
            vec![Command::Templates(
                screen.generation,
                Request::List { project: Some(7) }
            )]
        );
        assert!(screen.invalidate().is_empty());
        assert_eq!(
            screen.loaded(Ok(page("482139075", Some(7)))),
            vec![Command::Templates(
                screen.generation,
                Request::List { project: Some(7) }
            )]
        );
        assert!(screen.page.is_none());
        assert!(
            screen
                .loaded(Err(Failure::new("not_found", "No such project")))
                .is_empty()
        );
        assert!(screen.page.is_none());
        assert!(!screen.busy);
    }
    #[test]
    fn queued_picker_response_cannot_upload_into_a_new_template_page() {
        let mut state = super::super::state::ChatState::default();
        state.list.account.id = "482139075".into();
        state.show_templates(None);
        let old = state.templates.as_ref().unwrap().generation;
        state.show_templates(Some(7));
        let screen = state.templates.as_mut().unwrap();
        screen.loaded(Ok(page("482139075", Some(7))));
        screen.pick(
            Target {
                account: "482139075".into(),
                project: Some(7),
                layer: cww::workspace::Layer::Project,
                kind: "document".into(),
            },
            vec!["docx".into()],
        );
        assert!(
            state
                .update(super::super::state::Msg::TemplatePicked(
                    old,
                    Ok(Some("old.docx".into()))
                ))
                .is_empty()
        );
        let current = state.templates.as_ref().unwrap().generation;
        let commands = state.update(super::super::state::Msg::TemplatePicked(
            current,
            Ok(Some("new.docx".into())),
        ));
        assert!(
            matches!(&commands[..],[Command::UploadTemplate(_,target,path)] if target.project==Some(7) && path==std::path::Path::new("new.docx"))
        );
    }
    #[test]
    fn cancelled_file_picker_makes_no_upload_and_old_results_cannot_start_one() {
        let mut screen = Screen::new("482139075".into(), None);
        screen.loaded(Ok(page("482139075", None)));
        let target = Target {
            account: "482139075".into(),
            project: None,
            layer: cww::workspace::Layer::Personal,
            kind: "document".into(),
        };
        assert_eq!(screen.pick(target.clone(), vec!["docx".into()]).len(), 1);
        assert!(screen.busy);
        assert!(screen.picked(Ok(None)).is_empty());
        assert!(!screen.busy);
        assert!(screen.picked(Ok(Some("old.docx".into()))).is_empty());
        screen.pick(target.clone(), vec!["docx".into()]);
        assert_eq!(
            screen.picked(Ok(Some("current.docx".into()))),
            vec![Command::UploadTemplate(
                screen.generation,
                target,
                "current.docx".into()
            )]
        );
    }
}
