//! The web's Connect dialog: native rows and choices, browser-only vendor sign-in.

use cww::tui::chat::Failure;
use cww::workspace::{Catalog, Connector, Service, ServiceKind};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::controls::{self, Button, Kind, Size};
use super::icons::{Icon, Images};
use super::paint;
use super::state::Command;
use super::tokens::{Palette, scale};

use super::workspace::request_id;

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectDialog {
    pub query: String,
    pub category: Option<String>,
    pub catalog: Option<Catalog>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub handoff: Option<u64>,
    pub waiting_for_browser: bool,
    generation: u64,
    loading: bool,
    due: Option<f64>,
    next_refresh: f64,
    choosing: Option<String>,
}

impl Default for ConnectDialog {
    fn default() -> Self {
        Self {
            query: String::new(),
            category: None,
            catalog: None,
            error: None,
            notice: None,
            handoff: None,
            waiting_for_browser: false,
            generation: 0,
            loading: false,
            due: Some(0.0),
            next_refresh: 0.0,
            choosing: None,
        }
    }
}

impl ConnectDialog {
    pub fn loaded(&mut self, generation: u64, result: Result<Catalog, Failure>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(catalog) => {
                self.catalog = Some(catalog);
                self.error = None;
            }
            Err(failure) => self.error = Some(failure.message),
        }
    }

    pub fn reload(&mut self) {
        self.due = Some(0.0);
    }

    fn request(&mut self) -> Command {
        self.generation = request_id();
        self.loading = true;
        self.due = None;
        Command::Connectors {
            generation: self.generation,
            query: self.query.clone(),
            category: self.category.clone(),
        }
    }

    fn connect(
        &mut self,
        connector: &Connector,
        access: &str,
        connection: Option<String>,
    ) -> Command {
        let generation = request_id();
        self.handoff = Some(generation);
        self.error = None;
        self.notice = None;
        self.choosing = None;
        Command::ConnectorHandoff {
            generation,
            key: connector.key.clone(),
            access: access.into(),
            connection,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        changes_ready: bool,
        p: &Palette,
        images: &Images,
    ) -> (bool, Vec<Command>) {
        let now = ui.input(|i| i.time);
        let mut commands = vec![];
        if self.due.is_some_and(|due| now >= due)
            || (self.waiting_for_browser && now >= self.next_refresh && !self.loading)
        {
            commands.push(self.request());
            self.next_refresh = now + 5.0;
        }
        if self.waiting_for_browser {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(5));
        }
        if self.due.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let height = (ui.ctx().content_rect().height() - 64.0).clamp(240.0, 700.0);
        let mut close = false;
        let open = controls::modal(
            ui.ctx(),
            Id::new("connect-services"),
            672.0,
            height,
            p,
            |ui, rect| {
                let header = controls::dialog_header(
                    ui,
                    rect.min,
                    rect.width(),
                    "Connect a service",
                    &[(
                        "Connect at least one service to start chatting with your work tools.",
                        false,
                    )],
                    p,
                );
                let field = Rect::from_min_size(
                    rect.min + vec2(0.0, header + 18.0),
                    vec2(rect.width(), 38.0),
                );
                let response = ui.put(
                    field,
                    egui::TextEdit::singleline(&mut self.query)
                        .id(Id::new("connector-search"))
                        .font(scale::SMALL.font_id())
                        .hint_text("Search services")
                        .desired_width(field.width()),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search services")
                });
                if response.changed() {
                    self.category = None;
                    self.due = Some(now + 0.25);
                    self.choosing = None;
                }
                let footer = Rect::from_min_size(
                    pos2(rect.left(), rect.bottom() - 32.0),
                    vec2(rect.width(), 32.0),
                );
                let list = Rect::from_min_max(
                    pos2(rect.left(), field.bottom() + 16.0),
                    pos2(rect.right(), footer.top() - 12.0),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
                child.set_clip_rect(list);
                egui::ScrollArea::vertical()
                    .id_salt("connect-service-list")
                    .auto_shrink([false, false])
                    .show(&mut child, |ui| {
                        ui.set_width(list.width());
                        if let Some(error) = &self.error {
                            label(ui, error, p.negative_ink());
                            ui.add_space(12.0);
                        }
                        if let Some(notice) = &self.notice {
                            label(ui, notice, p.ink_muted);
                            ui.add_space(12.0);
                        }
                        if let Some(category) = self.category.clone() {
                            if button(
                                ui,
                                "back-categories",
                                "All services",
                                Kind::Subtle,
                                true,
                                p,
                                images,
                            ) {
                                self.category = None;
                                self.due = Some(0.0);
                            }
                            let name = self
                                .catalog
                                .as_ref()
                                .and_then(|c| c.categories.iter().find(|c| c.key == category))
                                .map(|c| c.name.as_str())
                                .unwrap_or(&category);
                            label(ui, name, p.ink);
                            ui.add_space(12.0);
                        }
                        if let Some(catalog) = self.catalog.clone() {
                            let mut category = String::new();
                            for connector in &catalog.connectors {
                                if connector.category != category {
                                    category.clone_from(&connector.category);
                                    let name = catalog
                                        .categories
                                        .iter()
                                        .find(|c| c.key == category)
                                        .map(|c| c.name.as_str())
                                        .unwrap_or(&category);
                                    if self.category.is_none() {
                                        ui.add_space(14.0);
                                        label(ui, name, p.ink_faint);
                                        ui.add_space(8.0);
                                    }
                                }
                                if let Some(action) =
                                    self.row(ui, connector, changes_ready, p, images)
                                {
                                    commands.push(action);
                                }
                            }
                            if catalog.connectors.is_empty() && !self.loading {
                                label(ui, "No services found. Try another name.", p.ink_muted);
                            }
                            if self.query.trim().is_empty() && self.category.is_none() {
                                ui.add_space(24.0);
                                label(ui, "More services", p.ink);
                                ui.add_space(8.0);
                                for category in catalog.categories {
                                    if button(
                                        ui,
                                        ("directory", &category.key),
                                        &format!("{} ({})", category.name, category.count),
                                        Kind::Subtle,
                                        true,
                                        p,
                                        images,
                                    ) {
                                        self.category = Some(category.key);
                                        self.due = Some(0.0);
                                    }
                                    ui.add_space(4.0);
                                }
                            }
                        }
                        if self.loading {
                            ui.add_space(12.0);
                            label(ui, "Loading…", p.ink_faint);
                        }
                    });
                let button = Button::new(Kind::Subtle, Size::Small, "I’m done");
                close = button
                    .show_at(
                        ui,
                        Id::new("close-connect-services"),
                        pos2(footer.right() - button.width(ui), footer.top()),
                        p,
                        images,
                    )
                    .clicked();
            },
        );
        (open && !close, commands)
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        connector: &Connector,
        changes_ready: bool,
        p: &Palette,
        images: &Images,
    ) -> Option<Command> {
        let mut command = None;
        let id = Id::new(("connector", &connector.key));
        egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.line)).corner_radius(8.0).inner_margin(16).show(ui, |ui| {
            ui.set_width((ui.available_width() - 2.0).max(1.0));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (icon, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                if !images.logos.draw(ui.painter(), connector.icon.as_ref(), icon, 0.0, p, 1.0) { images.icon_at(ui.painter(), icon.center(), 22.0, Icon::PlugsConnected, p.ink_muted); }
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(paint::job(&connector.name, scale::SMALL.weight(600.0), p.ink, ui.available_width())).wrap());
                    label(ui, &connector.description, p.ink_muted);
                    let state = if connector.paused { "Paused" } else { match connector.status.as_str() {
                        "connected" => "Connected", "refreshing" => "Refreshing", "expired" => "Authorization expired",
                        "unconfigured" | "awaiting_organization_app" => "Needs setup", _ => "Not connected",
                    }};
                    ui.add_space(6.0);
                    label(ui, state, if connector.status == "expired" { p.orange } else { p.ink_faint });
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let ready = changes_ready && self.handoff.is_none() && !matches!(connector.status.as_str(), "unconfigured" | "awaiting_organization_app");
                        let connected = matches!(connector.status.as_str(), "connected" | "refreshing" | "expired");
                        if button(ui, id.with("connect"), if connected { "Reconnect" } else { "Connect" }, Kind::Subtle, ready, p, images) {
                            let connection = connector.connections.first();
                            let access = connection.map_or("read", |c| c.access.as_str());
                            command = Some(self.connect(connector, access, connection.map(|c| c.id.to_string())));
                        }
                        if connector.changeable && button(ui, id.with("choice"), "Choose access", Kind::Subtle, ready, p, images) {
                            self.choosing = if self.choosing.as_deref() == Some(&connector.key) { None } else { Some(connector.key.clone()) };
                        }
                        if connected && button(ui, id.with("pause"), if connector.paused { "Resume" } else { "Pause" }, Kind::Subtle, changes_ready, p, images) {
                            let service = Service { key: connector.key.clone(), kind: ServiceKind::Connector, name: connector.name.clone(), icon: connector.icon.clone(), live: !connector.paused,
                                paused: connector.paused, label: state.into(), tone: None, removable: false };
                            command = Some(Command::ServicePause(request_id(), service, !connector.paused));
                        }
                        if connected && connector.several_accounts && button(ui, id.with("another"), "Add another account", Kind::Subtle, ready, p, images) {
                            command = Some(self.connect(connector, "read", Some("new".into())));
                        }
                    });
                    if self.choosing.as_deref() == Some(&connector.key) {
                        ui.add_space(12.0);
                        if button(ui, id.with("read"), "Read", Kind::Subtle, true, p, images) { command = Some(self.connect(connector, "read", None)); }
                        label(ui, "Search and read. You can change this later.", p.ink_muted);
                        ui.add_space(8.0);
                        if button(ui, id.with("change"), "Read and change", Kind::Subtle, connector.changes_allowed, p, images) { command = Some(self.connect(connector, "change", None)); }
                        label(ui, "Also make changes, like posting or creating files, each after you approve it.", p.ink_muted);
                        if !connector.changes_allowed { label(ui, "Your organization lets connectors only read.", p.ink_muted); }
                    }
                });
            });
        });
        ui.add_space(6.0);
        command
    }
}

fn label(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.add(
        egui::Label::new(paint::job(
            text,
            scale::SMALLER,
            color,
            ui.available_width(),
        ))
        .wrap(),
    );
}

fn button(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let button = Button::new(kind, Size::Small, label).enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(vec2(button.width(ui), button.height()), Sense::hover());
    button.show(ui, Id::new(id), rect, p, images).clicked()
}
