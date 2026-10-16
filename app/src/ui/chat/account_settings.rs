//! Personal names and answer copying, scoped to the selected organization.
use super::{
    controls::{self, Button, FieldLook, Kind, Size},
    icons::Images,
    paint,
    sidebar::Event,
    state::Command,
    tokens::{Palette, Type, scale},
    workspace::request_id,
};
use cww::{
    account_settings::{Change, CopyFormat, Page, Profile, Request},
    tui::chat::{Failure, SettingsLink},
};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    account: String,
    pub page: Option<Page>,
    pub draft: Profile,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub confirm_close: bool,
    change: Option<Change>,
}

impl Screen {
    pub fn new(account: String) -> Self {
        Self {
            generation: request_id(),
            account,
            page: None,
            draft: Profile::default(),
            busy: false,
            error: None,
            notice: None,
            confirm_close: false,
            change: None,
        }
    }
    pub fn dirty(&self) -> bool {
        self.page
            .as_ref()
            .is_some_and(|page| page.profile != self.draft)
    }
    pub fn load(&mut self) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        self.change = None;
        vec![Command::AccountSettings(self.generation, Request::Read)]
    }
    pub fn save_profile(&mut self) -> Vec<Command> {
        if !self.dirty() || self.busy {
            return vec![];
        }
        if self.draft.full_name.trim().is_empty() || self.draft.display_name.trim().is_empty() {
            self.error = Some("Fill in your full name and what we should call you.".into());
            return vec![];
        }
        self.save(Change::Profile {
            full_name: self.draft.full_name.clone(),
            display_name: self.draft.display_name.clone(),
        })
    }
    pub fn save_format(&mut self, format: CopyFormat) -> Vec<Command> {
        if self
            .page
            .as_ref()
            .is_none_or(|page| page.copy_answers_as == format)
        {
            return vec![];
        }
        self.save(Change::CopyFormat {
            copy_answers_as: format,
        })
    }
    fn save(&mut self, change: Change) -> Vec<Command> {
        if self.busy || self.page.is_none() {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        self.notice = None;
        self.change = Some(change.clone());
        vec![Command::AccountSettings(
            self.generation,
            Request::Update {
                account: self.account.clone(),
                change,
            },
        )]
    }
    pub fn loaded(&mut self, result: Result<Page, Failure>) -> Option<Page> {
        self.busy = false;
        let change = self.change.take();
        match result {
            Ok(page) if page.account.id == self.account => {
                // Copying saves apart from the profile, as on the website.
                if !matches!(change, Some(Change::CopyFormat { .. })) {
                    self.draft = page.profile.clone();
                }
                self.notice = change.map(|change| match change {
                    Change::Profile { .. } => "Profile updated.".into(),
                    Change::CopyFormat { .. } => "Copying preference updated.".into(),
                });
                self.error = None;
                self.page = Some(page.clone());
                Some(page)
            }
            Ok(_) => {
                self.clear();
                self.error = Some("Your organization changed. Open account settings again.".into());
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
                    self.clear();
                }
                self.error = Some(error.message);
                None
            }
        }
    }
    fn clear(&mut self) {
        self.page = None;
        self.draft = Profile::default();
        self.notice = None;
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
    let header = screen
        .page
        .as_ref()
        .map(|page| format!("{} · {}", page.email, page.account.name));
    let event = super::settings::Shell {
        rect,
        generation: screen.generation,
        header: header.as_deref(),
        active: "account",
        tabs,
        busy: screen.busy,
        palette: p,
        images,
    }
    .show(ui, |ui| content(ui, screen, tabs, p, images, &mut outcome));
    if event.is_some() {
        outcome.event = event;
    }
    if screen.confirm_close {
        let mut keep = false;
        let open = controls::modal(
            ui.ctx(),
            Id::new("discard-profile"),
            420.0,
            204.0,
            p,
            |ui, rect| {
                let height = controls::dialog_header(
                    ui,
                    rect.min,
                    rect.width(),
                    "Discard profile changes?",
                    &[("Your profile has not been saved.", false)],
                    p,
                );
                let at = pos2(rect.left(), rect.top() + height + 28.0);
                let button = Button::new(Kind::Subtle, Size::Small, "Keep editing");
                keep = button
                    .show_at(ui, Id::new("keep-profile"), at, p, images)
                    .clicked();
                outcome.discarded = Button::new(Kind::Primary, Size::Small, "Discard changes")
                    .show_at(
                        ui,
                        Id::new("discard-profile-confirm"),
                        pos2(at.x + button.width(ui) + 8.0, at.y),
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

fn content(
    ui: &mut Ui,
    screen: &mut Screen,
    tabs: &[SettingsLink],
    p: &Palette,
    images: &Images,
    out: &mut Outcome,
) {
    ui.set_width(ui.available_width());
    if let Some(error) = &screen.error {
        text(ui, error, scale::SMALL, p.negative_ink());
        ui.add_space(12.0);
    }
    if let Some(notice) = &screen.notice {
        text(ui, notice, scale::SMALL, p.ink);
        ui.add_space(12.0);
    }
    let Some(page) = screen.page.clone() else {
        text(
            ui,
            if screen.busy {
                "Loading account settings…"
            } else {
                "Account settings could not be loaded."
            },
            scale::SMALL,
            p.ink_muted,
        );
        ui.add_space(16.0);
        if !screen.busy && button(ui, "Retry", false, true, p, images) {
            out.commands.extend(screen.load());
        }
        return;
    };
    heading(ui, "Profile", p);
    text(
        ui,
        &format!(
            "Shown to people in {}. Each of your organizations has its own.",
            page.account.name
        ),
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    group(ui, p, |ui| {
        ui.add_enabled_ui(!screen.busy, |ui| {
            field(
                ui,
                "Full name",
                "Alex Morgan",
                &mut screen.draft.full_name,
                screen.generation,
                p,
            );
            separator(ui, p);
            field(
                ui,
                "What should we call you?",
                "e.g. Alex",
                &mut screen.draft.display_name,
                screen.generation,
                p,
            );
        });
    });
    ui.add_space(16.0);
    if button(
        ui,
        "Save profile",
        true,
        !screen.busy && screen.dirty(),
        p,
        images,
    ) {
        out.commands.extend(screen.save_profile());
    }
    ui.add_space(48.0);
    heading(ui, "Preferences", p);
    ui.add_space(16.0);
    group(ui, p, |ui| {
        row(ui, "Copy answers as", p, |ui| {
            let selected = copy_format(ui, page.copy_answers_as, !screen.busy, p);
            if selected != page.copy_answers_as {
                out.commands.extend(screen.save_format(selected));
            }
            ui.add_space(8.0);
            text(
                ui,
                "Rich text pastes into Docs, Mail, and Word with its bold, lists, links, and tables, and as Markdown into plain editors. Markdown copies the Markdown alone.",
                Type::sans(12.0, 18.0),
                p.ink_muted,
            );
        });
    });
    // These actions still require the website's signed-in identity session.
    if let Some(tab) = tabs.iter().find(|tab| tab.tab == "account") {
        ui.add_space(48.0);
        heading(ui, "Sign-in and security", p);
        text(
            ui,
            "Manage your email, password, profile picture, and account deletion on the website.",
            scale::SMALL,
            p.ink_muted,
        );
        ui.add_space(16.0);
        if button(
            ui,
            "Open account on the website",
            false,
            !screen.busy,
            p,
            images,
        ) {
            out.event = Some(Event::OpenUrl(tab.url.clone()));
        }
    }
}
fn heading(ui: &mut Ui, label: &str, p: &Palette) {
    text(ui, label, Type::sans(15.0, 22.5).weight(600.0), p.ink);
}
fn text(ui: &mut Ui, label: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(label, ty, color, ui.available_width())).wrap());
}
fn group(ui: &mut Ui, p: &Palette, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(14.0)
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}
fn field(
    ui: &mut Ui,
    label: &str,
    placeholder: &str,
    value: &mut String,
    generation: u64,
    p: &Palette,
) {
    row(ui, label, p, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        controls::text_field(
            ui,
            Id::new(("profile", generation, label)),
            rect,
            value,
            placeholder,
            scale::SMALL,
            12.0,
            FieldLook::new(p),
            p,
            label,
        );
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
            let label_width = if width >= 520.0 {
                (width - 32.0) / 3.0
            } else {
                width
            };
            let galley = paint::layout(
                ui.painter(),
                paint::job(label, scale::SMALL.weight(550.0), p.ink, label_width),
            );
            if width >= 520.0 {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(label_width, galley.size().y), Sense::hover());
                    ui.painter().galley(rect.min, galley, p.ink);
                    ui.add_space(32.0);
                    ui.vertical(content);
                });
            } else {
                let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                ui.painter().galley(rect.min, galley, p.ink);
                ui.add_space(8.0);
                content(ui);
            }
        });
}
fn copy_format(ui: &mut Ui, selected: CopyFormat, enabled: bool, p: &Palette) -> CopyFormat {
    let width = if ui.available_width() < 350.0 {
        ui.available_width()
    } else {
        184.0
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
    for (index, (value, label)) in [
        (CopyFormat::RichText, "Rich text"),
        (CopyFormat::Markdown, "Markdown"),
    ]
    .into_iter()
    .enumerate()
    {
        let segment = Rect::from_min_size(
            rect.min + vec2(index as f32 * width / 2.0 + 2.0, 2.0),
            vec2(width / 2.0 - 4.0, 30.0),
        );
        let response = ui.interact(
            segment,
            Id::new(("copy-format", label)),
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
    chosen
}

fn button(
    ui: &mut Ui,
    label: &str,
    right: bool,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let b = Button::new(Kind::Subtle, Size::Small, label).enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            if right {
                ui.available_width()
            } else {
                b.width(ui)
            },
            b.height(),
        ),
        Sense::hover(),
    );
    b.show_at(
        ui,
        Id::new(("account-button", label)),
        if right {
            pos2(rect.right() - b.width(ui), rect.top())
        } else {
            rect.min
        },
        p,
        images,
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page(account: &str) -> Page {
        serde_json::from_value(serde_json::json!({"account":{"id":account,"name":"Northwind"},"email":"alex@example.com","profile":{"full_name":"Alex Rivera","display_name":"Alex"},"copy_answers_as":"rich_text"})).unwrap()
    }
    #[test]
    fn copying_saves_apart_from_names_and_failures_preserve_the_draft() {
        let mut screen = Screen::new("482139075".into());
        screen.load();
        screen.loaded(Ok(page("482139075")));
        screen.draft.full_name = "Alex Morgan".into();
        assert_eq!(
            screen.save_format(CopyFormat::Markdown),
            vec![Command::AccountSettings(
                screen.generation,
                Request::Update {
                    account: "482139075".into(),
                    change: Change::CopyFormat {
                        copy_answers_as: CopyFormat::Markdown
                    }
                }
            )]
        );
        assert!(screen.save_profile().is_empty());
        let mut saved = page("482139075");
        saved.copy_answers_as = CopyFormat::Markdown;
        screen.loaded(Ok(saved.clone()));
        assert_eq!(screen.draft.full_name, "Alex Morgan");
        assert!(screen.dirty());
        assert_eq!(
            screen.save_profile(),
            vec![Command::AccountSettings(
                screen.generation,
                Request::Update {
                    account: "482139075".into(),
                    change: Change::Profile {
                        full_name: "Alex Morgan".into(),
                        display_name: "Alex".into()
                    }
                }
            )]
        );
        screen.loaded(Err(Failure::new("unavailable", "Try again.")));
        assert!(screen.dirty());
        assert_eq!(screen.draft.full_name, "Alex Morgan");
        screen.save_profile();
        saved.profile = screen.draft.clone();
        screen.loaded(Ok(saved));
        assert!(!screen.dirty());
        assert_eq!(screen.notice.as_deref(), Some("Profile updated."));
    }
    #[test]
    fn missing_names_are_refused_and_wrong_accounts_or_revocation_clear_private_data() {
        let mut screen = Screen::new("482139075".into());
        screen.loaded(Ok(page("482139075")));
        screen.draft.display_name = " ".into();
        assert!(screen.save_profile().is_empty());
        assert!(screen.error.as_deref().unwrap().contains("Fill in"));
        assert!(screen.loaded(Ok(page("918273645"))).is_none());
        assert!(screen.page.is_none() && screen.draft.full_name.is_empty());
        screen.loaded(Ok(page("482139075")));
        screen.loaded(Err(Failure::new("forbidden", "Access removed.")));
        assert!(screen.page.is_none() && screen.draft.display_name.is_empty());
        assert!(screen.save_format(CopyFormat::Markdown).is_empty());
    }
}
