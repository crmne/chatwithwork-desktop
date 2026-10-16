//! The organization's existing notification preferences, edited natively.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use cww::notification_delivery::{Permission, Status as NativeStatus};

use cww::control::ControlRequest;
use egui::Ui;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{SettingsApp, open_url, widgets};
use crate::control::Client;
use crate::model::Status;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Account {
    id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ChatChoice {
    key: String,
    title: String,
    description: String,
    push: bool,
    email: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EmailChoice {
    key: String,
    title: String,
    description: String,
    enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Preferences {
    account: Account,
    chat_notifications: Vec<ChatChoice>,
    notification_content: bool,
    notification_emails: Vec<EmailChoice>,
}

impl Preferences {
    fn update(&self) -> Value {
        json!({
            "account": self.account.id,
            "chat_notifications": self.chat_notifications.iter().map(|choice|
                (choice.key.clone(), json!({"push": choice.push, "email": choice.email}))
            ).collect::<serde_json::Map<_, _>>(),
            "notification_content": self.notification_content,
            "notification_emails": self.notification_emails.iter().map(|choice|
                (choice.key.clone(), json!(choice.enabled))
            ).collect::<serde_json::Map<_, _>>()
        })
    }
}

enum Reply {
    Native(Result<NativeStatus, String>),
    Preferences(Result<Preferences, String>),
    BrowserLink(Result<String, String>),
}

pub(super) struct Settings {
    scope: (String, String),
    client: Client,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    saved: Option<Preferences>,
    draft: Option<Preferences>,
    loading: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    retry: Option<ControlRequest>,
    native: Option<NativeStatus>,
    native_busy: bool,
    native_error: Option<String>,
    native_checked: Option<Instant>,
}

impl Settings {
    fn new(client: Client, scope: (String, String)) -> Self {
        let (tx, rx) = channel();
        Self {
            scope,
            client,
            tx,
            rx,
            saved: None,
            draft: None,
            loading: true,
            busy: false,
            error: None,
            notice: None,
            retry: None,
            native: None,
            native_busy: false,
            native_error: None,
            native_checked: None,
        }
    }

    fn request(&mut self, ctx: &egui::Context, request: ControlRequest) {
        self.busy = true;
        self.retry = Some(request.clone());
        if matches!(request, ControlRequest::NotificationPreferences) && self.draft.is_none() {
            self.loading = true;
        }
        self.error = None;
        self.notice = None;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("cww-notification-settings".into())
            .spawn(move || {
                let link = matches!(request, ControlRequest::NotificationBrowserLink);
                let result = client.request(&request).map_err(|error| error.to_string());
                let reply = if link {
                    Reply::BrowserLink(result.and_then(|value| {
                        value["path"]
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "The server returned no browser link.".into())
                    }))
                } else {
                    Reply::Preferences(result.and_then(|value| {
                        serde_json::from_value(value).map_err(|_| {
                            "The server returned invalid notification settings.".into()
                        })
                    }))
                };
                let _ = tx.send(reply);
                ctx.request_repaint();
            })
        {
            self.busy = false;
            self.loading = false;
            self.error = Some(format!("Could not load notification settings: {error}"));
        }
    }

    fn native_request(&mut self, ctx: &egui::Context, permission: bool) {
        self.native_busy = true;
        self.native_checked = Some(Instant::now());
        self.native_error = None;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("cww-notification-permission".into())
            .spawn(move || {
                let result = (|| {
                    if permission {
                        client
                            .request(&ControlRequest::NotificationPermission { request: true })
                            .map_err(|error| error.to_string())?;
                    }
                    let value = client
                        .request(&ControlRequest::NotificationStatus)
                        .map_err(|error| error.to_string())?;
                    serde_json::from_value(value)
                        .map_err(|_| "The agent returned invalid notification status.".to_string())
                })();
                let _ = tx.send(Reply::Native(result));
                ctx.request_repaint();
            })
        {
            self.native_busy = false;
            self.native_error = Some(format!("Could not check notifications: {error}"));
        }
    }

    fn poll(&mut self) {
        while let Ok(reply) = self.rx.try_recv() {
            if !matches!(reply, Reply::Native(_)) {
                self.busy = false;
            }
            match reply {
                Reply::Native(result) => {
                    self.native_busy = false;
                    match result {
                        Ok(status) => self.native = Some(status),
                        Err(error) => self.native_error = Some(error),
                    }
                }
                Reply::Preferences(Ok(preferences)) => {
                    if !self.loading {
                        self.notice = Some("Preferences saved.".into());
                    }
                    self.saved = Some(preferences.clone());
                    self.draft = Some(preferences);
                    self.loading = false;
                }
                Reply::Preferences(Err(error)) | Reply::BrowserLink(Err(error)) => {
                    self.loading = false;
                    self.error = Some(error);
                }
                Reply::BrowserLink(Ok(path)) => {
                    let account = self.saved.as_ref().map(|saved| saved.account.id.as_str());
                    if let Some(url) =
                        account.and_then(|account| browser_link(&self.scope.0, account, &path))
                    {
                        open_url(&url);
                        self.notice = Some("Confirm that this browser is on this computer.".into());
                    } else {
                        self.error = Some("The server returned an invalid browser link.".into());
                    }
                }
            }
        }
    }
}

fn browser_link(server: &str, account: &str, path: &str) -> Option<String> {
    (!account.is_empty()
        && account.bytes().all(|b| b.is_ascii_digit())
        && path.starts_with(&format!("/{account}/native/notification_browser_link?"))
        && !path.contains(['\\', '\r', '\n']))
    .then(|| format!("{}{path}", server.trim_end_matches('/')))
}

impl SettingsApp {
    pub(super) fn notifications_page(&mut self, ui: &mut Ui, status: Option<&Status>) {
        let theme = self.theme;
        widgets::page_header(
            ui,
            &theme,
            "Notifications",
            "Choose when Chat with Work gets your attention.",
        );
        let scope = status
            .filter(|status| status.is_paired() && status.connection.connection != "revoked")
            .and_then(|status| Some((status.server.clone()?, status.device_id.clone()?)));
        let Some(scope) = scope else {
            self.notifications = None;
            ui.label(theme.weak("Pair this computer to manage your notification preferences."));
            return;
        };
        if self
            .notifications
            .as_ref()
            .is_none_or(|settings| settings.scope != scope)
        {
            let mut settings = Settings::new(
                Client::new(self.organizations.socket(&self.shared.paths)),
                scope,
            );
            settings.request(ui.ctx(), ControlRequest::NotificationPreferences);
            self.notifications = Some(settings);
        }
        let settings = self.notifications.as_mut().unwrap();
        settings.poll();
        if !settings.native_busy
            && settings
                .native_checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(3))
        {
            settings.native_request(ui.ctx(), false);
        }
        ui.ctx().request_repaint_after(Duration::from_secs(3));
        if let Some(error) = &settings.error {
            widgets::notice(ui, &theme, error, theme.palette.danger);
            if !settings.busy
                && ui.button("Try again").clicked()
                && let Some(request) = settings.retry.clone()
            {
                settings.request(ui.ctx(), request);
            }
        }
        if settings.loading {
            ui.label(theme.weak("Loading notification preferences…"));
        }
        if let Some(notice) = &settings.notice {
            ui.label(theme.weak(notice));
        }
        let Some(draft) = &mut settings.draft else {
            return;
        };
        ui.label(theme.weak(format!("Preferences for {}", draft.account.name)));
        widgets::section(ui, &theme, "Chat notifications");
        ui.add(egui::Label::new(theme.weak("Hear when a chat needs you or has finished, while you're not looking at it. Push goes to the browsers, computers and phones where you turned notifications on.")).wrap());
        ui.add_enabled_ui(!settings.busy, |ui| {
            widgets::group(ui, &theme, |ui| {
                for (index, choice) in draft.chat_notifications.iter_mut().enumerate() {
                    if index > 0 { widgets::row_separator(ui, &theme); }
                    widgets::setting_row(ui, &theme, &choice.title, &choice.description, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(82.0);
                            ui.horizontal(|ui| {
                                widgets::switch(ui, &theme, &mut choice.push, &format!("{} Push", choice.title));
                                ui.label("Push");
                            });
                            ui.horizontal(|ui| {
                                widgets::switch(ui, &theme, &mut choice.email, &format!("{} Email", choice.title));
                                ui.label("Email");
                            });
                        });
                    });
                }
                widgets::row_separator(ui, &theme);
                widgets::setting_row(ui, &theme, "Show the chat and its answer", "Browser notifications show the chat's title and the start of the answer, encrypted on the way. Native apps and emails only say that something happened.", |ui| {
                    widgets::switch(ui, &theme, &mut draft.notification_content, "Show the chat and its answer");
                });
            });
            widgets::section(ui, &theme, "Email notifications");
            widgets::group(ui, &theme, |ui| {
                for (index, choice) in draft.notification_emails.iter_mut().enumerate() {
                    if index > 0 { widgets::row_separator(ui, &theme); }
                    widgets::setting_row(ui, &theme, &choice.title, &choice.description, |ui| {
                        widgets::switch(ui, &theme, &mut choice.enabled, &choice.title);
                    });
                }
            });
        });
        let update = draft.update();
        let dirty = settings.saved.as_ref() != settings.draft.as_ref();
        if ui
            .add_enabled(
                !settings.busy && dirty,
                egui::Button::new("Save preferences"),
            )
            .clicked()
        {
            settings.request(
                ui.ctx(),
                ControlRequest::NotificationPreferencesUpdate {
                    preferences: update,
                },
            );
        }
        widgets::section(ui, &theme, "On this computer");
        let description = settings
            .native
            .as_ref()
            .map(native_description)
            .unwrap_or("Checking this computer's notification support…");
        ui.add(egui::Label::new(theme.weak(description)).wrap());
        if let Some(error) = &settings.native_error {
            widgets::notice(ui, &theme, error, theme.palette.danger);
        }
        if settings
            .native
            .as_ref()
            .is_some_and(|status| status.permission == Permission::NotDetermined)
            && ui
                .add_enabled(
                    !settings.native_busy,
                    egui::Button::new("Turn on native notifications…"),
                )
                .clicked()
        {
            settings.native_request(ui.ctx(), true);
        }
        ui.add_space(8.0);
        ui.add(egui::Label::new(theme.weak("Link this browser to avoid duplicate notifications here. Your other computers and phones keep their own notifications.")).wrap());
        if ui
            .add_enabled(!settings.busy, egui::Button::new("Link this browser…"))
            .clicked()
        {
            settings.request(ui.ctx(), ControlRequest::NotificationBrowserLink);
        }
    }
}

fn native_description(status: &NativeStatus) -> &'static str {
    match status.permission {
        Permission::Denied => {
            "Notifications are blocked in your system settings. Allow Chat with Work there to receive them on this computer."
        }
        Permission::NotDetermined => {
            "Allow notifications on this computer. Your system will ask for permission."
        }
        Permission::Unavailable => {
            "Native notifications are unavailable. Your browser and email preferences still apply."
        }
        Permission::Granted | Permission::Unknown if !status.connected => {
            "Waiting for the notification connection. Browser and email notifications remain available."
        }
        Permission::Granted => {
            "This computer can receive native notifications, even while the app window is closed."
        }
        Permission::Unknown => {
            "Chat with Work can send notifications on this computer. If the system cannot show one, your linked browser can notify you instead."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_handoff_stays_on_the_displayed_account_and_paired_server() {
        assert_eq!(
            browser_link(
                "https://work.example",
                "482139075",
                "/482139075/native/notification_browser_link?token=example"
            ),
            Some(
                "https://work.example/482139075/native/notification_browser_link?token=example"
                    .into()
            )
        );
        for path in [
            "https://elsewhere.example/",
            "//elsewhere.example/",
            "/123456789/native/notification_browser_link?token=example",
            "/482139075/native/notification_browser_link?token=example\\escape",
            "/482139075/chats/16",
        ] {
            assert!(browser_link("https://work.example", "482139075", path).is_none());
        }
    }
}
