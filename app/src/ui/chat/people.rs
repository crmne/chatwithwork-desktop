//! Organization administration using the website's permissions and forms.
use super::{
    account_settings::{button, group, heading, separator, text},
    controls::{self, Button, FieldLook, Kind, Size},
    icons::{Icon, Images},
    paint,
    sidebar::Event,
    state::Command,
    tokens::{Palette, Type, scale},
    widgets::IconButton,
    workspace::request_id,
};
use cww::{
    people::{InvitationFields, OrganizationFields, Page, Reply, Request},
    tui::chat::{Failure, SettingsLink},
};
use egui::{Id, Rect, Sense, Ui, pos2, vec2};

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub generation: u64,
    account: String,
    pub page: Option<Page>,
    pub organization: OrganizationFields,
    pub invitation: InvitationFields,
    pub busy: bool,
    pub error: Option<String>,
    pub confirm_close: bool,
    confirmation: Option<(String, String, Request)>,
    pending: Option<Request>,
    copied: bool,
}
impl Screen {
    pub fn new(account: String) -> Self {
        Self {
            generation: request_id(),
            account,
            page: None,
            organization: OrganizationFields::default(),
            invitation: InvitationFields::default(),
            busy: false,
            error: None,
            confirm_close: false,
            confirmation: None,
            pending: None,
            copied: false,
        }
    }
    pub fn dirty(&self) -> bool {
        self.page.as_ref().is_some_and(|page| {
            page.admin
                && (self.organization != OrganizationFields::from(&page.organization)
                    || self.invitation != InvitationFields::default())
        })
    }
    pub fn request(&mut self, request: Request) -> Vec<Command> {
        if self.busy {
            return vec![];
        }
        self.busy = true;
        self.error = None;
        self.pending = Some(request.clone());
        vec![Command::People(self.generation, request)]
    }
    pub fn loaded(&mut self, result: Result<Reply, Failure>) -> Option<String> {
        self.busy = false;
        let pending = self.pending.take();
        match result {
            Ok(Reply::Page(page)) if page.account.id == self.account => {
                // Each website form saves separately. A role or invitation
                // response must not discard unsaved organization settings.
                if self.page.is_none()
                    || matches!(pending, Some(Request::Organization { .. }))
                    || !page.admin
                {
                    self.organization = OrganizationFields::from(&page.organization);
                }
                if matches!(pending, Some(Request::Invite { .. })) || !page.admin {
                    self.invitation = InvitationFields::default();
                }
                if matches!(pending, Some(Request::ResetJoinLink { .. })) {
                    self.copied = false;
                }
                self.page = Some(*page);
                self.error = None;
                self.confirmation = None;
            }
            Ok(Reply::Left {
                account,
                left: true,
                notice,
            }) if account.id == self.account
                && matches!(pending, Some(Request::Remove { id, .. }) if self.page.as_ref().is_some_and(|p|p.people.iter().any(|person|person.is_self && person.id==id))) =>
            {
                self.clear();
                return Some(notice);
            }
            Ok(_) => {
                self.clear();
                self.error = Some("Your organization changed. Open People again.".into());
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
            }
        }
        None
    }
    fn clear(&mut self) {
        self.page = None;
        self.organization = OrganizationFields::default();
        self.invitation = InvitationFields::default();
        self.confirmation = None;
        self.copied = false;
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
        .map(|p| format!("{} · {}", p.email, p.account.name));
    let event = super::settings::Shell {
        rect,
        generation: screen.generation,
        header: header.as_deref(),
        active: "people",
        tabs,
        busy: screen.busy,
        palette: p,
        images,
    }
    .show(ui, |ui| content(ui, screen, p, images, &mut out));
    if event.is_some() {
        out.event = event;
    }
    if screen.confirm_close {
        let (keep, discard) = confirmation(
            ui,
            "discard-people",
            "Discard People changes?",
            "Your organization settings or invitation have not been saved.",
            "Keep editing",
            "Discard changes",
            p,
            images,
        );
        if keep {
            screen.confirm_close = false;
        }
        out.discarded = discard;
    } else if let Some((title, action, request)) = screen.confirmation.clone() {
        let (cancel, confirmed) = confirmation(
            ui,
            "confirm-people",
            &title,
            "",
            "Cancel",
            &action,
            p,
            images,
        );
        if cancel || confirmed {
            screen.confirmation = None;
        }
        if confirmed {
            out.commands.extend(screen.request(request));
        }
    }
    out
}
fn content(ui: &mut Ui, screen: &mut Screen, p: &Palette, images: &Images, out: &mut Outcome) {
    ui.set_width(ui.available_width());
    if let Some(error) = &screen.error {
        text(ui, error, scale::SMALL, p.negative_ink());
        ui.add_space(12.0);
    }
    let Some(page) = screen.page.clone() else {
        text(
            ui,
            if screen.busy {
                "Loading people…"
            } else {
                "People could not be loaded."
            },
            scale::SMALL,
            p.ink_muted,
        );
        ui.add_space(16.0);
        if button(ui, "Retry", false, !screen.busy, p, images) {
            out.commands.extend(screen.request(Request::List));
        }
        return;
    };
    if let Some(notice) = &page.notice {
        text(ui, notice, scale::SMALL, p.ink);
        ui.add_space(12.0);
    }
    heading(ui, "Organization", p);
    text(
        ui,
        "Everyone here shares projects and team chats. Private chats stay private: no one else, admins included, can read them.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    group(ui, p, |ui| {
        if page.admin {
            ui.add_enabled_ui(!screen.busy, |ui| {
                field(
                    ui,
                    "Name",
                    "Organization name",
                    &mut screen.organization.name,
                    screen.generation,
                    p,
                );
                for policy in &page.organization.policies {
                    separator(ui, p);
                    padded(ui, |ui| {
                        let width = ui.available_width();
                        let start = ui.cursor().min;
                        let bounds =
                            Rect::from_min_size(start, vec2(width - 52.0, ui.available_height()));
                        let mut body = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                        body.set_width(bounds.width());
                        text(&mut body, &policy.label, scale::SMALL.weight(550.0), p.ink);
                        text(&mut body, &policy.description, scale::SMALL, p.ink_muted);
                        let height = body.min_rect().height().max(20.0);
                        let switch_rect = Rect::from_min_size(
                            start + vec2(width - 36.0, (height - 20.0) / 2.0),
                            vec2(36.0, 20.0),
                        );
                        let mut switch = ui.new_child(egui::UiBuilder::new().max_rect(switch_rect));
                        controls::switch(
                            &mut switch,
                            screen
                                .organization
                                .policies
                                .entry(policy.key.clone())
                                .or_insert(policy.value),
                            &policy.label,
                            !screen.busy,
                            p,
                        );
                        ui.allocate_exact_size(vec2(width, height), Sense::hover());
                    });
                }
            });
        } else {
            padded(ui, |ui| {
                text(ui, &page.account.name, scale::SMALL.weight(550.0), p.ink);
                text(
                    ui,
                    &format!("You're a {}", page.role),
                    scale::MICRO,
                    p.ink_faint,
                );
            });
        }
    });
    if page.admin {
        ui.add_space(16.0);
        if button(ui, "Save", true, !screen.busy, p, images) {
            if screen.organization.name.trim().is_empty() {
                screen.error = Some("Give the organization a name.".into());
            } else {
                out.commands.extend(screen.request(Request::Organization {
                    account: screen.account.clone(),
                    fields: screen.organization.clone(),
                }));
            }
        }
    }
    ui.add_space(48.0);
    heading(ui, "People", p);
    text(
        ui,
        &format!(
            "{} {}",
            page.people.len(),
            if page.people.len() == 1 {
                "person"
            } else {
                "people"
            }
        ),
        scale::MICRO,
        p.ink_faint,
    );
    text(
        ui,
        "Owners and admins invite and remove people. Removing someone disconnects their connectors and deletes their private chats after 30 days.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    group(ui, p, |ui| {
        for (i, person) in page.people.iter().enumerate() {
            if i > 0 {
                separator(ui, p);
            }
            let role_width = if person.can_change_role { 112.0 } else { 60.0 };
            let actions_width = role_width + if person.can_remove { 40.0 } else { 0.0 };
            record_row(
                ui,
                p,
                images,
                Some(&super::sidebar::initials(&person.name)),
                None,
                actions_width,
                |ui| {
                    text(
                        ui,
                        &format!(
                            "{}{}",
                            person.name,
                            if person.is_self { " · you" } else { "" }
                        ),
                        scale::SMALL.weight(550.0),
                        p.ink,
                    );
                    text(ui, &person.email, scale::MICRO, p.ink_faint);
                },
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if person.can_change_role {
                            let mut role = title_case(&person.role);
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(role_width, 32.0), Sense::hover());
                            let changed = ui
                                .add_enabled_ui(!screen.busy, |ui| {
                                    controls::select(
                                        ui,
                                        Id::new(("person-role", screen.generation, person.id)),
                                        rect,
                                        &mut role,
                                        &page
                                            .roles
                                            .iter()
                                            .map(|s| title_case(s))
                                            .collect::<Vec<_>>(),
                                        &format!("Role for {}", person.name),
                                        p,
                                        images,
                                    )
                                })
                                .inner;
                            if changed {
                                out.commands.extend(screen.request(Request::Role {
                                    account: screen.account.clone(),
                                    id: person.id,
                                    role: role.to_lowercase(),
                                }));
                            }
                        } else {
                            text(ui, &title_case(&person.role), scale::SMALL, p.ink_muted);
                        }
                        if person.can_remove
                            && icon_button(
                                ui,
                                if person.is_self {
                                    Icon::SignOut
                                } else {
                                    Icon::UserMinus
                                },
                                &person.remove_label,
                                !screen.busy,
                                p,
                                images,
                            )
                        {
                            screen.confirmation = Some((
                                person.remove_confirmation.clone(),
                                if person.is_self {
                                    "Leave".into()
                                } else {
                                    "Remove".into()
                                },
                                Request::Remove {
                                    account: screen.account.clone(),
                                    id: person.id,
                                },
                            ));
                        }
                    });
                },
            );
        }
    });
    if page.admin {
        invitation(ui, screen, &page, p, images, out);
    }
    ui.add_space(48.0);
    heading(ui, "Join link", p);
    text(
        ui,
        &format!(
            "Anyone with this link can join {} as a member. Share it in a place only your team can see.",
            page.account.name
        ),
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    group(ui, p, |ui| {
        record_row(
            ui,
            p,
            images,
            None,
            Some(Icon::Link),
            if page.admin { 132.0 } else { 92.0 },
            |ui| {
                ui.add(
                    egui::Label::new(paint::line_job(
                        &page.join_link,
                        scale::MICRO,
                        p.ink,
                        ui.available_width(),
                    ))
                    .truncate(),
                );
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let label = if screen.copied { "Copied" } else { "Copy" };
                    let b = Button::new(Kind::Subtle, Size::Small, label)
                        .icon(if screen.copied {
                            Icon::Check
                        } else {
                            Icon::Copy
                        })
                        .enabled(!screen.busy);
                    let (rect, _) = ui.allocate_exact_size(vec2(b.width(ui), 32.0), Sense::hover());
                    if b.show_at(ui, Id::new("copy-join-link"), rect.min, p, images)
                        .clicked()
                    {
                        ui.ctx().copy_text(page.join_link.clone());
                        screen.copied = true;
                    }
                    if page.admin
                        && icon_button(
                            ui,
                            Icon::ArrowsClockwise,
                            "Reset the join link",
                            !screen.busy,
                            p,
                            images,
                        )
                    {
                        screen.confirmation = Some((
                            "Reset the join link? The old one stops working.".into(),
                            "Reset link".into(),
                            Request::ResetJoinLink {
                                account: screen.account.clone(),
                            },
                        ));
                    }
                });
            },
        )
    });
    if page.admin {
        ui.add_space(8.0);
        text(
            ui,
            "After removing someone, reset the link so they can't use it to come back.",
            Type::sans(12.0, 18.0),
            p.ink_muted,
        );
    }
    if let Some(url) = &page.new_organization_url {
        ui.add_space(48.0);
        group(ui, p, |ui| {
            record_row(
                ui,
                p,
                images,
                None,
                Some(Icon::Plus),
                Button::new(Kind::Subtle, Size::Small, "New organization").width(ui),
                |ui| {
                    text(
                        ui,
                        "Start another organization",
                        scale::SMALL.weight(550.0),
                        p.ink,
                    );
                    text(
                        ui,
                        "For a different company or client. Your login stays the same; chats and connectors are kept separate.",
                        scale::SMALL,
                        p.ink_muted,
                    );
                },
                |ui| {
                    if button(ui, "New organization", false, !screen.busy, p, images) {
                        out.event = Some(Event::OpenUrl(url.clone()));
                    }
                },
            )
        });
    }
}
fn invitation(
    ui: &mut Ui,
    screen: &mut Screen,
    page: &Page,
    p: &Palette,
    images: &Images,
    out: &mut Outcome,
) {
    ui.add_space(48.0);
    heading(ui, "Invite someone", p);
    text(
        ui,
        "They'll get an email with a link that works once, for two weeks.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    group(ui, p, |ui| {
        ui.add_enabled_ui(!screen.busy,|ui|{
        field(ui,"Email","name@company.com",&mut screen.invitation.email,screen.generation,p);
        separator(ui,p);
        row(ui,"Role",p,|ui|{
            roles(ui,&mut screen.invitation.role,!screen.busy,p);
            ui.add_space(8.0);
            text(ui,"Admins also invite and remove people and manage projects. Guests are clients: they only read the chats you show them, in the projects you pick.",Type::sans(12.0,18.0),p.ink_muted);
        });
        if let Some(projects)=&page.projects && !projects.is_empty() {
            separator(ui,p);
            row(ui,"Projects",p,|ui|{
                for project in projects {
                    let mut selected=screen.invitation.project_ids.contains(&project.id);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x=8.0;
                        if controls::switch(ui,&mut selected,&project.name,!screen.busy,p).changed() {
                            screen.invitation.project_ids.retain(|id|*id!=project.id);
                            if selected {screen.invitation.project_ids.push(project.id);}
                        }
                        text(ui,&project.name,scale::SMALL,p.ink);
                    });
                    ui.add_space(6.0);
                }
                text(ui,"Guests need at least one. Members also join HQ on their own; guests never do.",Type::sans(12.0,18.0),p.ink_muted);
            });
        }
    }).inner
    });
    ui.add_space(16.0);
    if primary_button(ui, "Send invitation", !screen.busy, p, images) {
        if screen.invitation.email.trim().is_empty() {
            screen.error = Some("Enter an email address.".into());
        } else {
            out.commands.extend(screen.request(Request::Invite {
                account: screen.account.clone(),
                fields: screen.invitation.clone(),
            }));
        }
    }
    if let Some(invitations) = &page.invitations
        && !invitations.is_empty()
    {
        ui.add_space(16.0);
        group(ui, p, |ui| {
            for (i, invitation) in invitations.iter().enumerate() {
                if i > 0 {
                    separator(ui, p);
                }
                record_row(
                    ui,
                    p,
                    images,
                    None,
                    Some(Icon::EnvelopeSimple),
                    104.0,
                    |ui| {
                        text(ui, &invitation.email, scale::SMALL.weight(550.0), p.ink);
                        text(
                            ui,
                            &format!(
                                "{} · invited by {} · expires {}",
                                title_case(&invitation.role),
                                invitation.inviter,
                                invitation.expires_on
                            ),
                            scale::MICRO,
                            p.ink_faint,
                        );
                    },
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            text(ui, "Pending", scale::MICRO, p.attention);
                            if icon_button(
                                ui,
                                Icon::X,
                                &format!("Withdraw invitation to {}", invitation.email),
                                !screen.busy,
                                p,
                                images,
                            ) {
                                out.commands.extend(screen.request(Request::Withdraw {
                                    account: screen.account.clone(),
                                    id: invitation.id,
                                }));
                            }
                        });
                    },
                );
            }
        });
    }
}
fn padded(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, content);
}
fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        format!("{}{}", f.to_uppercase(), c.as_str())
    })
}
fn roles(ui: &mut Ui, selected: &mut String, enabled: bool, p: &Palette) {
    let width = ui.available_width().min(240.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, p.canvas);
    ui.painter().rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    for (i, value) in ["member", "admin", "guest"].iter().enumerate() {
        let segment = Rect::from_min_size(
            rect.min + vec2(i as f32 * width / 3.0 + 2.0, 2.0),
            vec2(width / 3.0 - 4.0, 30.0),
        );
        let response = ui.interact(
            segment,
            Id::new(("invitation-role", value)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let label = title_case(value);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                enabled,
                selected == value,
                &label,
            )
        });
        if selected == value {
            ui.painter().rect_filled(segment, 6.0, p.surface);
            ui.painter().rect_stroke(
                segment,
                6.0,
                egui::Stroke::new(1.0, p.line),
                egui::StrokeKind::Inside,
            );
        }
        let galley = paint::layout(
            ui.painter(),
            paint::line_job(&label, scale::SMALL, p.ink, segment.width()),
        );
        ui.painter()
            .galley(segment.center() - galley.size() / 2.0, galley, p.ink);
        if enabled {
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            *selected = (*value).into();
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn confirmation(
    ui: &mut Ui,
    id: &str,
    title: &str,
    description: &str,
    cancel: &str,
    action: &str,
    p: &Palette,
    images: &Images,
) -> (bool, bool) {
    let mut keep = false;
    let mut confirmed = false;
    let open = controls::modal(ui.ctx(), Id::new(id), 440.0, 240.0, p, |ui, rect| {
        let height = controls::dialog_header(
            ui,
            rect.min,
            rect.width(),
            title,
            &[(description, false)],
            p,
        );
        let at = pos2(rect.left(), rect.top() + height + 24.0);
        let b = Button::new(Kind::Subtle, Size::Small, cancel);
        keep = b
            .show_at(ui, Id::new((id, "cancel")), at, p, images)
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
            Id::new(("people-field", generation, label)),
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
fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
    IconButton {
        icon,
        icon_size: 16.0,
        label,
        tooltip: Some(label),
        color: p.ink_muted,
        hover_color: p.ink,
        wash: 5.0,
        radius: 8.0,
        enabled,
    }
    .show(ui, Id::new(("people-action", label)), rect, images, p)
    .clicked()
}
fn primary_button(ui: &mut Ui, label: &str, enabled: bool, p: &Palette, images: &Images) -> bool {
    let button = Button::new(Kind::Primary, Size::Small, label).enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    button
        .show_at(
            ui,
            Id::new(("people-button", label)),
            pos2(rect.right() - button.width(ui), rect.top()),
            p,
            images,
        )
        .clicked()
}
#[allow(clippy::too_many_arguments)]
fn record_row(
    ui: &mut Ui,
    p: &Palette,
    images: &Images,
    initials: Option<&str>,
    icon: Option<Icon>,
    actions_width: f32,
    body: impl FnOnce(&mut Ui),
    actions: impl FnOnce(&mut Ui),
) {
    padded(ui, |ui| {
        let width = ui.available_width();
        let start = ui.cursor().min;
        let wide = width >= 520.0;
        let body_width = width - 52.0 - if wide { actions_width + 16.0 } else { 0.0 };
        let bounds = Rect::from_min_size(
            start + vec2(52.0, 0.0),
            vec2(body_width, ui.available_height()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
        child.set_width(body_width);
        body(&mut child);
        let body_height = child.min_rect().height().max(36.0);
        let center = start + vec2(18.0, body_height / 2.0);
        if let Some(initials) = initials {
            ui.painter().circle_filled(center, 18.0, p.canvas);
            ui.painter()
                .circle_stroke(center, 18.0, egui::Stroke::new(1.0, p.line));
            let galley = paint::layout(
                ui.painter(),
                paint::line_job(
                    initials,
                    Type::sans(12.0, 16.0).weight(550.0),
                    p.ink_muted,
                    36.0,
                ),
            );
            ui.painter()
                .galley(center - galley.size() / 2.0, galley, p.ink_muted);
        } else if let Some(icon) = icon {
            ui.painter().rect_filled(
                Rect::from_center_size(center, vec2(36.0, 36.0)),
                8.0,
                p.canvas,
            );
            images.icon_at(ui.painter(), center, 16.0, icon, p.ink_muted);
        }
        let action_at = start
            + vec2(
                width - actions_width,
                if wide {
                    (body_height - 32.0) / 2.0
                } else {
                    body_height + 12.0
                },
            );
        let bounds = Rect::from_min_size(action_at, vec2(actions_width, 32.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
        child.set_width(actions_width);
        actions(&mut child);
        ui.allocate_exact_size(
            vec2(
                width,
                if wide {
                    body_height
                } else {
                    body_height + 44.0
                },
            ),
            Sense::hover(),
        );
    });
}
