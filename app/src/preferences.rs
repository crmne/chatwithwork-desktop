//! App-owned settings in a native child webview, never in the server DOM.
//! The server supplies only a fixed page name and a rectangle. Account/bot
//! scope comes independently from the paired daemon's DPoP-authenticated API.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Rect, State, Url, Webview, WebviewUrl,
};

use crate::desktop::Desktop;

pub const PREFIX: &str = "computer-pane-";
pub const SHELL: &str = "settings-shell.html";
const PAGES: [&str; 6] = [
    "folders",
    "browser",
    "general",
    "notifications",
    "account",
    "activity",
];

#[derive(Default)]
pub struct Panels(Mutex<BTreeMap<String, Entry>>, Mutex<()>);

struct Entry {
    proof: String,
    generation: u64,
    page: Option<String>,
    context: Option<(Value, Instant)>,
    bounds: Option<Rect>,
    preparation: Option<String>,
    confirmation: Option<InlineConfirmation>,
}

struct InlineConfirmation {
    token: String,
    request: Value,
    context: Value,
    page: Option<String>,
    started: Instant,
}

impl Entry {
    fn cancel_clipped_confirmation(&mut self, client: LogicalSize<f64>) -> bool {
        let Some(bounds) = self.bounds.filter(|_| self.confirmation.is_some()) else {
            return false;
        };
        let position = bounds.position.to_logical::<f64>(1.0);
        let size = bounds.size.to_logical::<f64>(1.0);
        if position.x + size.width <= client.width && position.y + size.height <= client.height {
            return false;
        }
        // Never move a still-valid Allow target to fit a resized window.
        self.preparation = None;
        self.confirmation = None;
        true
    }

    fn placement(&mut self, page: &str, context: &Value, requested: Rect) -> (Rect, bool) {
        if self.page.as_deref() != Some(page)
            || self
                .context
                .as_ref()
                .is_some_and(|(previous, _)| previous != context)
        {
            self.preparation = None;
        }
        if let Some(pending) = &self.confirmation {
            if pending.page.as_deref() == Some(page)
                && pending.context["grant"] == *context
                && pending.started.elapsed() < Duration::from_secs(120)
            {
                return (self.bounds.unwrap_or(requested), false);
            }
            self.confirmation = None;
            return (requested, true);
        }
        (requested, false)
    }
}

fn random_token() -> String {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("secure randomness");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl Panels {
    pub fn register(&self, owner: &str) -> String {
        let proof = random_token();
        self.0.lock().expect("settings panels").insert(
            owner.into(),
            Entry {
                proof: proof.clone(),
                generation: 0,
                page: None,
                context: None,
                bounds: None,
                preparation: None,
                confirmation: None,
            },
        );
        proof
    }

    fn begin(&self, owner: &str, proof: &str) -> Result<(u64, Option<Value>), String> {
        let mut entries = self.0.lock().expect("settings panels");
        let entry = entries
            .get_mut(owner)
            .filter(|entry| entry.proof == proof)
            .ok_or("Only this window's main page can show computer settings.")?;
        entry.generation += 1;
        let context = entry
            .context
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(30))
            .map(|(value, _)| value.clone());
        Ok((entry.generation, context))
    }

    fn current(&self, owner: &str, generation: u64) -> bool {
        self.0
            .lock()
            .expect("settings panels")
            .get(owner)
            .is_some_and(|entry| entry.generation == generation)
    }

    /// Reserve before blocking status/context work. A late response cannot
    /// replace a newer visible question, or recreate one after navigation.
    pub fn begin_confirmation(&self, owner: &str) -> Result<String, String> {
        let mut entries = self.0.lock().expect("settings panels");
        let entry = entries
            .get_mut(owner)
            .ok_or("Reopen this computer’s settings first.")?;
        let preparation = random_token();
        entry.preparation = Some(preparation.clone());
        entry.confirmation = None;
        Ok(preparation)
    }

    pub fn prepare(
        &self,
        owner: &str,
        preparation: &str,
        request: &cww::control::ControlRequest,
        context: Value,
    ) -> Result<String, String> {
        let mut entries = self.0.lock().expect("settings panels");
        let entry = entries
            .get_mut(owner)
            .ok_or("Reopen this computer’s settings first.")?;
        if entry.preparation.as_deref() != Some(preparation) {
            return Err("These settings changed. Review the change again.".into());
        }
        entry.preparation = None;
        let token = random_token();
        entry.confirmation = Some(InlineConfirmation {
            token: token.clone(),
            request: serde_json::to_value(request).map_err(|e| e.to_string())?,
            context,
            page: entry.page.clone(),
            started: Instant::now(),
        });
        Ok(token)
    }

    #[cfg(test)]
    fn prepare_for_test(
        &self,
        owner: &str,
        request: &cww::control::ControlRequest,
        context: Value,
    ) -> Result<String, String> {
        let preparation = self.begin_confirmation(owner)?;
        self.prepare(owner, &preparation, request, context)
    }

    pub fn cancel_confirmation(&self, owner: &str, token: &str) {
        if let Some(entry) = self.0.lock().expect("settings panels").get_mut(owner)
            && entry
                .confirmation
                .as_ref()
                .is_some_and(|pending| pending.token == token)
        {
            entry.confirmation = None;
        }
    }

    pub fn consume_confirmation(
        &self,
        owner: &str,
        token: Option<&str>,
        request: &cww::control::ControlRequest,
        context: &Value,
    ) -> Result<(), String> {
        let mut entries = self.0.lock().expect("settings panels");
        let entry = entries
            .get_mut(owner)
            .ok_or("Reopen this computer’s settings first.")?;
        let pending = entry
            .confirmation
            .take()
            .ok_or("Review this change before allowing it.")?;
        if token != Some(pending.token.as_str())
            || pending.started.elapsed() > Duration::from_secs(120)
            || pending.request != serde_json::to_value(request).map_err(|e| e.to_string())?
            || pending.context != *context
            || pending.page != entry.page
        {
            return Err("These settings changed. Review the change again.".into());
        }
        Ok(())
    }
}

/// These two existing inline questions are the only flows with duplicate
/// confirmation. Network access, browser policy, exclusions and bot grants
/// keep their one native dialog. Remote computer_change never uses this path.
pub fn has_inline_confirmation(request: &cww::control::ControlRequest) -> bool {
    matches!(
        request,
        cww::control::ControlRequest::RootsWritable { writable: true, .. }
            | cww::control::ControlRequest::RootsCommands {
                commands: true,
                network: false,
                ..
            }
    )
}

pub fn confirmation_context(
    request: &cww::control::ControlRequest,
    status: &crate::model::Status,
    context: &Value,
) -> Result<Value, String> {
    use cww::control::ControlRequest as R;
    let root = match request {
        R::RootsWritable { root, .. } | R::RootsCommands { root, .. } => root,
        _ => return Err("This change uses its existing confirmation.".into()),
    };
    let folder = status
        .roots
        .iter()
        .find(|folder| &folder.id == root)
        .ok_or("That folder is no longer shared.")?;
    // Index progress may change while the person reads; the folder's identity
    // and access must not. None of this local state is sent to the parent page.
    Ok(
        json!({"grant":context,"server":status.server,"device_id":status.device_id,"folder":{
            "id":folder.id,"label":folder.label,"path":folder.local_path,
            "available":folder.available,"writable":folder.writable,
            "commands":folder.allows_commands,"network":folder.network
        }}),
    )
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Present {
    page: String,
    #[serde(default)]
    bot: Option<String>,
    bounds: Bounds,
    viewport: Viewport,
    theme: Theme,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Viewport {
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn color(self) -> tauri::window::Color {
        crate::windows::canvas_color(matches!(self, Self::Dark))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

fn page_matches(url: &Url, page: &str, bot: Option<&str>, context: &Value) -> bool {
    let Some(account) = context["account"]
        .as_str()
        .filter(|id| (7..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()))
    else {
        return false;
    };
    if page == "bot-sharing" {
        let Some(bot) = bot.filter(|id| {
            !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
        }) else {
            return false;
        };
        let confirmed = context["bot"]["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| context["bot"]["id"].as_u64().map(|id| id.to_string()));
        url.path() == format!("/{account}/bots/{bot}") && confirmed.as_deref() == Some(bot)
    } else {
        PAGES.contains(&page)
            && bot.is_none()
            && url.path() == format!("/{account}/settings")
            && url
                .query_pairs()
                .filter(|(key, _)| key == "tab")
                .map(|(_, value)| value.into_owned())
                .collect::<Vec<_>>()
                == [format!("computer_{page}")]
    }
}

fn rectangle(bounds: Bounds, viewport: Viewport, size: LogicalSize<f64>) -> Result<Rect, String> {
    if ![
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        viewport.width,
        viewport.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || viewport.width < 1.0
        || viewport.height < 1.0
        || bounds.width < 1.0
        || bounds.height < 1.0
    {
        return Err("The settings area isn't visible.".into());
    }
    let x = bounds.x.max(0.0).min(viewport.width);
    let y = bounds.y.max(0.0).min(viewport.height);
    let width = (bounds.x + bounds.width).min(viewport.width) - x;
    let height = (bounds.y + bounds.height).min(viewport.height) - y;
    if width < 120.0 || height < 80.0 {
        return Err("The settings area is too small.".into());
    }
    Ok(Rect {
        position: LogicalPosition::new(
            x * size.width / viewport.width,
            y * size.height / viewport.height,
        )
        .into(),
        size: LogicalSize::new(
            width * size.width / viewport.width,
            height * size.height / viewport.height,
        )
        .into(),
    })
}

/// Map CSS coordinates into the page's actual client area. On GTK, Tao's
/// cached window size includes client-side decorations and shadows; scaling
/// by that size makes every child both too large and too far right/down.
fn client_size(parent: &Webview) -> Result<LogicalSize<f64>, String> {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        parent
            .with_webview(move |handle| {
                use gtk::prelude::*;
                let allocation = handle.inner().allocation();
                let _ = tx.send(LogicalSize::new(
                    f64::from(allocation.width()),
                    f64::from(allocation.height()),
                ));
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "The native client size did not answer.".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let window = parent.window();
        Ok(window
            .inner_size()
            .map_err(|e| e.to_string())?
            .to_logical(window.scale_factor().map_err(|e| e.to_string())?))
    }
}

/// Tauri canonicalizes App("index.html") to the local root on some platforms.
pub fn is_document(url: &Url) -> bool {
    crate::windows::is_local_url(url) && matches!(url.path(), "/" | "/index.html")
}

pub fn is_child(label: &str) -> bool {
    label
        .strip_prefix(PREFIX)
        .is_some_and(crate::windows::is_remote_label)
}

pub fn child_label(owner: &str) -> String {
    format!("{PREFIX}{owner}")
}

/// The child is app-owned, but its placement is requested by a remote page.
/// Widening needs a native dialog or a one-use inline confirmation whose
/// placement is frozen. The local control protocol remains unchanged for the CLI.
pub fn question(
    request: &cww::control::ControlRequest,
    status: &crate::model::Status,
    context: &Value,
) -> Result<Option<crate::computer::Question>, String> {
    use crate::computer::{Change, Question};
    use cww::control::ControlRequest as R;
    let change = match request {
        R::Resume => Some(Change::Resume),
        R::RootsWritable { root, writable, .. } => Some(Change::Changes {
            folder: root.clone(),
            on: *writable,
        }),
        R::RootsCommands {
            root,
            commands,
            network,
        } => {
            let previous = status
                .roots
                .iter()
                .find(|folder| &folder.id == root)
                .ok_or("That folder is no longer shared.")?;
            if *commands && !previous.allows_commands {
                Some(Change::Commands {
                    folder: root.clone(),
                    on: true,
                })
            } else {
                Some(Change::Network {
                    folder: root.clone(),
                    on: *network,
                })
            }
        }
        _ => None,
    };
    if let Some(change) = change {
        let (_, mut question) = change.plan(status)?;
        if matches!(
            request,
            R::RootsCommands {
                network: true,
                commands: true,
                ..
            }
        ) && let Change::Commands { folder, .. } = &change
            && status
                .roots
                .iter()
                .any(|root| &root.id == folder && !root.network)
            && let Some(question) = question.as_mut()
        {
            question.message.push_str(" These commands may also use the network, including sending data from this folder.");
        }
        return Ok(question);
    }
    let ask = |title: &str, message: String| {
        Some(Question {
            title: title.into(),
            message,
            yes: "Allow".into(),
        })
    };
    Ok(match request {
        R::DenyExtra { previous, pattern } => {
            if let Some(pattern) = pattern {
                cww::exclusions::pattern(pattern).map_err(|e| e.to_string())?;
            }
            previous.as_ref().map(|previous| Question {
                title: if pattern.is_some() { "Change exclusion" } else { "Remove exclusion" }.into(),
                message: format!("{}? Files covered by the old pattern may become readable in shared folders, including by bots using them. Other exclusions still apply.",
                    pattern.as_ref().map(|new| format!("Replace “{previous}” with “{}”", new.trim())).unwrap_or_else(|| format!("Stop excluding “{previous}”"))),
                yes: if pattern.is_some() { "Change exclusion" } else { "Remove exclusion" }.into(),
            })
        }
        R::RootsAdd { path, .. } => ask("Share a folder", format!("Share {} with Chat with Work? It can read files here, except the files excluded by your private-file rules.", path.display())),
        R::RootsBot { root, bot, lent: true, unattended, .. } => {
            let confirmed = context["bot"]["id"].as_u64().map(|id| id.to_string()).or_else(|| context["bot"]["id"].as_str().map(str::to_owned));
            if confirmed.as_deref() != Some(bot) { return Err("Choose folder access from this bot’s settings.".into()); }
            let folder = status.roots.iter().find(|folder| &folder.id == root).ok_or("That folder is no longer shared.")?;
            let name = context["bot"]["name"].as_str().unwrap_or("this bot");
            let existing = folder.bots.iter().find(|grant| &grant.id == bot);
            if existing.is_none() || (*unattended && existing.is_some_and(|grant| !grant.unattended)) {
                ask("Share with bot", format!("Let {name} (bot {bot}) use “{}” on this computer? It can read here{}{}. You can stop sharing from the bot’s settings.", folder.label,
                    if folder.writable { " and change files with approval" } else { "" },
                    if *unattended { ". Its commands can start without approval, and can change or delete anything in this folder" } else if folder.allows_commands { " and run approved commands in this folder’s sandbox" } else { "" }))
            } else { None }
        }
        R::BrowserBot { bot, lent: true, .. } => {
            let confirmed = context["bot"]["id"].as_u64().map(|id| id.to_string()).or_else(|| context["bot"]["id"].as_str().map(str::to_owned));
            if confirmed.as_deref() != Some(bot) { return Err("Choose browser access from this bot’s settings.".into()); }
            ask("Share browser with bot", format!("Let {} (bot {bot}) use your browser’s Chat with Work tab group on the allowed sites, with your sign-ins? Changes still ask for approval.", context["bot"]["name"].as_str().unwrap_or("this bot")))
        }
        R::BrowserAskBeforeActions { ask: false } if status.browser.ask_before_actions => ask("Run browser actions without asking", "Let Chat with Work click, type and submit on permitted sites without asking again, including sending, buying or deleting? Your site rules, protected fields and Stop still apply. Bots keep their own approval rules. This does not change file or command approvals.".into()),
        R::BrowserAllSites { all_sites: true } if !status.browser.all_sites => ask("Allow all sites", "Let Chat with Work use any website in its own tab group, except your blocked sites, with your sign-ins? Protected fields and change approvals still apply.".into()),
        R::BrowserBlockedSite { site, blocked: false } => ask("Unblock a site", format!("Remove the block on {site}? Chat with Work can use it when your browser mode and allowed sites permit it.")),
        R::BrowserSite { site, allowed: true } => ask("Allow a site", format!("Let Chat with Work use {site} in its browser tab group, with your sign-in there?")),
        R::BrowserChanges { changes: true } if !status.browser.changes => ask("Allow browser changes", "Let Chat with Work type, click and submit on the allowed sites? Each change asks for approval in Chat with Work.".into()),
        R::BrowserStopped { stopped: false } if status.browser.stopped => ask("Resume browser sharing", "Let Chat with Work use the browser again, with the sites and actions you already allowed?".into()),
        _ => None,
    })
}

/// A bot grant belongs to the currently displayed, independently authenticated
/// bot. Recheck immediately before writing, even if the pane was mounted earlier.
pub fn grant_context(
    app: &AppHandle,
    desktop: &Desktop,
    child: &Webview,
    request: &cww::control::ControlRequest,
) -> Result<Value, String> {
    use cww::control::ControlRequest as R;
    let bot = match request {
        R::RootsBot { bot, .. } | R::BrowserBot { bot, .. } => Some(bot.as_str()),
        _ => None,
    };
    let widening = matches!(
        request,
        R::Resume
            | R::DenyExtra {
                previous: Some(_),
                ..
            }
            | R::RootsAdd { .. }
            | R::RootsWritable { writable: true, .. }
            | R::RootsCommands { commands: true, .. }
            | R::RootsCommands { network: true, .. }
            | R::BrowserSite { allowed: true, .. }
            | R::BrowserAllSites { all_sites: true }
            | R::BrowserAskBeforeActions { ask: false }
            | R::BrowserBlockedSite { blocked: false, .. }
            | R::BrowserChanges { changes: true }
            | R::BrowserStopped { stopped: false }
            | R::BrowserEnabled { enabled: true }
    );
    if bot.is_none() && !widening {
        return Ok(Value::Null);
    }
    let owner = child
        .label()
        .strip_prefix(PREFIX)
        .ok_or("Open this computer’s settings in Chat with Work.")?;
    let parent = app.get_webview(owner).ok_or("This window closed.")?;
    // Before pairing/offline the local shell is itself app-owned. Bot grants
    // always require the authenticated bot route, even in that local shell.
    if bot.is_none()
        && parent
            .url()
            .is_ok_and(|url| crate::windows::is_settings_shell(&url))
    {
        return Ok(Value::Null);
    }
    let page = desktop
        .preferences
        .0
        .lock()
        .expect("settings panels")
        .get(owner)
        .and_then(|entry| entry.page.clone())
        .ok_or("Reopen this computer’s settings first.")?;
    let context = desktop
        .client
        .request(&R::SettingsContext {
            bot: bot.map(str::to_owned),
        })
        .map_err(|e| e.to_string())?;
    let url = crate::bridge::server_page(desktop, &parent)?;
    let device = desktop
        .client
        .status()
        .map_err(|e| e.to_string())?
        .device_id;
    if device.as_deref() != context["device_id"].as_str()
        || !page_matches(&url, &page, bot, &context)
    {
        return Err("Open these settings in the account this computer is paired with.".into());
    }

    Ok(context)
}

pub fn hide(app: &AppHandle, owner: &str) {
    let desktop = app.state::<Arc<Desktop>>();
    if let Some(entry) = desktop
        .preferences
        .0
        .lock()
        .expect("settings panels")
        .get_mut(owner)
    {
        entry.generation += 1;
        entry.page = None;
        entry.context = None;
        entry.preparation = None;
        entry.confirmation = None;
    }
    if let Some(child) = app.get_webview(&child_label(owner)) {
        let _ = child.eval(crate::windows::settings_event(
            "confirmation_cancelled",
            &Value::Null,
        ));
        let _ = child.hide();
    }
}

pub fn close(app: &AppHandle, owner: &str) {
    hide(app, owner);
    if let Some(child) = app.get_webview(&child_label(owner)) {
        let _ = child.close();
    }
}

/// Tauri currently packs Linux child webviews into GtkBox, where Wry ignores
/// overlay bounds. Keep the managed webview/IPC isolation, but use GTK's own
/// overlay layout for this one view. Other platforms use native child bounds.
fn place_child(child: &Webview, bounds: Rect) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let scale = child.window().scale_factor().map_err(|e| e.to_string())?;
        let position = bounds.position.to_logical::<i32>(scale);
        let size = bounds.size.to_logical::<i32>(scale);
        let (tx, rx) = std::sync::mpsc::channel();
        child
            .with_webview(move |handle| {
                use gtk::prelude::*;
                let view = handle.inner();
                let result = (|| {
                    let parent = view.parent().ok_or("The native pane has no container.")?;
                    if let Some(container) = parent.downcast_ref::<gtk::Box>() {
                        let overlay = if let Some(overlay) = container
                            .children()
                            .into_iter()
                            .find_map(|widget| widget.downcast::<gtk::Overlay>().ok())
                        {
                            overlay
                        } else {
                            let main = container
                                .children()
                                .into_iter()
                                .find(|widget| {
                                    widget.is::<webkit2gtk::WebView>()
                                        && widget != view.upcast_ref::<gtk::Widget>()
                                })
                                .ok_or("The native window has no main page.")?;
                            container.remove(&main);
                            let overlay = gtk::Overlay::new();
                            overlay.add(&main);
                            container.pack_start(&overlay, true, true, 0);
                            overlay.show_all();
                            overlay
                        };
                        container.remove(&view);
                        overlay.add_overlay(&view);
                    }
                    view.set_halign(gtk::Align::Start);
                    view.set_valign(gtk::Align::Start);
                    view.set_hexpand(false);
                    view.set_vexpand(false);
                    view.set_margin_start(position.x);
                    view.set_margin_top(position.y);
                    view.set_size_request(size.width, size.height);
                    view.show();
                    Ok(())
                })();
                let _ = tx.send(result.map_err(str::to_string));
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "The native layout did not answer.".to_string())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        child.set_bounds(bounds).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn settings_present(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    request: Present,
    proof: String,
) -> Result<(), String> {
    crate::bridge::server_page(&desktop, &webview)?;
    let (generation, cached) = desktop.preferences.begin(webview.label(), &proof)?;
    let desktop = desktop.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mount = desktop.preferences.1.lock().expect("settings mount");
        if !desktop.preferences.current(webview.label(), generation) {
            return Ok(());
        }
        let status = desktop.client.status().map_err(|e| e.to_string())?;
        let device = status
            .device_id
            .as_deref()
            .ok_or("Pair this computer first.")?;
        let cached_at = desktop
            .preferences
            .0
            .lock()
            .expect("settings panels")
            .get(webview.label())
            .and_then(|entry| entry.context.as_ref().map(|(_, at)| *at));
        let mut refreshed = false;
        let context = match cached.filter(|context| {
            context["device_id"].as_str() == Some(device)
                && context["bot"]["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .or_else(|| context["bot"]["id"].as_str().map(str::to_owned))
                    == request.bot
        }) {
            Some(context) => context,
            None => {
                refreshed = true;
                match desktop
                    .client
                    .request(&cww::control::ControlRequest::SettingsContext {
                        bot: request.bot.clone(),
                    }) {
                    Ok(context) => context,
                    Err(error) => {
                        if desktop.preferences.current(webview.label(), generation) {
                            hide(&app, webview.label());
                        }
                        return Err(error.to_string());
                    }
                }
            }
        };
        let url = crate::bridge::server_page(&desktop, &webview)?;
        if context["device_id"].as_str() != Some(device)
            || !page_matches(&url, &request.page, request.bot.as_deref(), &context)
        {
            if desktop.preferences.current(webview.label(), generation) {
                hide(&app, webview.label());
            }
            return Err("Open these settings in the account this computer is paired with.".into());
        }
        if !desktop.preferences.current(webview.label(), generation) {
            return Ok(());
        }
        let window = webview.window();
        let size = client_size(&webview)?;
        let mut bounds = rectangle(request.bounds, request.viewport, size)?;
        let label = child_label(webview.label());
        let navigation =
            json!({ "page": request.page, "theme": request.theme.name(), "context": context });
        {
            let mut entries = desktop.preferences.0.lock().expect("settings panels");
            let entry = entries
                .get_mut(webview.label())
                .ok_or("This window closed.")?;
            if entry.generation != generation {
                return Ok(());
            }
            let clipped = entry.cancel_clipped_confirmation(size);
            let (placement, cancelled) = entry.placement(&request.page, &context, bounds);
            bounds = placement;
            if (clipped || cancelled)
                && let Some(child) = app.get_webview(&label)
            {
                let _ = child.eval(crate::windows::settings_event(
                    "confirmation_cancelled",
                    &Value::Null,
                ));
            }
            entry.context = Some((
                context.clone(),
                if refreshed {
                    Instant::now()
                } else {
                    cached_at.unwrap_or_else(Instant::now)
                },
            ));
            entry.page = Some(request.page.clone());
            entry.bounds = Some(bounds);
        }
        let child = if let Some(child) = app.get_webview(&label) {
            let _ = child.set_background_color(Some(request.theme.color()));
            place_child(&child, bounds)?;
            child.show().map_err(|e| e.to_string())?;
            let _ = child.eval(crate::windows::settings_event("presentation", &navigation));
            child
        } else {
            let initial = format!("window.CWW_SETTINGS_PANE = {navigation};");
            // GTK reparents the newly created view into its overlay. Loading
            // settings during that operation can interrupt the initial load;
            // start inert, then load the actual document exactly once below.
            let builder = WebviewBuilder::new(&label, WebviewUrl::App(SHELL.into()))
                .background_color(request.theme.color())
                .initialization_script(initial)
                .disable_drag_drop_handler()
                .on_navigation(|url| is_document(url) || crate::windows::is_settings_shell(url))
                .on_new_window(|_, _| NewWindowResponse::Deny);
            let child = window
                .add_child(builder, bounds.position, bounds.size)
                .map_err(|e| e.to_string())?;
            place_child(&child, bounds)?;
            child
                .navigate(crate::windows::local_url("index.html"))
                .map_err(|e| e.to_string())?;
            child
        };
        place_child(&child, bounds)?;
        if !desktop.preferences.current(webview.label(), generation) {
            let _ = child.close();
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn settings_hide(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    proof: String,
) -> Result<(), String> {
    crate::bridge::server_page(&desktop, &webview)?;
    desktop.preferences.begin(webview.label(), &proof)?;
    hide(&app, webview.label());
    Ok(())
}

/// The remote page cannot keep a revoked pane alive by stopping its timers.
/// Revalidate account/device/bot scope independently while any pane is open.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let desktop = app.state::<Arc<Desktop>>();
            let due: Vec<_> = desktop
                .preferences
                .0
                .lock()
                .expect("settings panels")
                .iter()
                .filter_map(|(owner, entry)| {
                    let (context, at) = entry.context.as_ref()?;
                    let page = entry.page.clone()?;
                    (at.elapsed() >= Duration::from_secs(25))
                        .then(|| (owner.clone(), entry.generation, page, context.clone()))
                })
                .collect();
            for (owner, generation, page, previous) in due {
                let bot = previous["bot"]["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .or_else(|| previous["bot"]["id"].as_str().map(str::to_owned));
                let context = desktop
                    .client
                    .request(&cww::control::ControlRequest::SettingsContext { bot: bot.clone() });
                let valid = context.as_ref().ok().is_some_and(|context| {
                    app.get_webview(&owner)
                        .and_then(|parent| crate::bridge::server_page(&desktop, &parent).ok())
                        .is_some_and(|url| page_matches(&url, &page, bot.as_deref(), context))
                        && desktop
                            .client
                            .status()
                            .ok()
                            .and_then(|status| status.device_id)
                            .as_deref()
                            == context["device_id"].as_str()
                });
                if !desktop.preferences.current(&owner, generation) {
                    continue;
                }
                if !valid {
                    hide(&app, &owner);
                    continue;
                }
                let context = context.expect("validated context");
                {
                    let mut entries = desktop.preferences.0.lock().expect("settings panels");
                    let Some(entry) = entries
                        .get_mut(&owner)
                        .filter(|entry| entry.generation == generation)
                    else {
                        continue;
                    };
                    entry.context = Some((context.clone(), Instant::now()));
                }
                if context != previous
                    && let Some(child) = app.get_webview(&child_label(&owner))
                {
                    let _ = child.eval(crate::windows::settings_event("context", &context));
                }
            }
        }
    });
}

/// The same native window before pairing, while offline, or with an old
/// server. The local navigation is app-owned too, so no remote page is
/// granted local commands just to show the welcome or account screen.
pub fn standalone(app: &AppHandle, page: &str) -> tauri::Result<()> {
    let app = app.clone();
    let page = page.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = standalone_now(&app, &page) {
            log::error!("computer settings: {error}");
        }
    });
    Ok(())
}

fn standalone_now(app: &AppHandle, page: &str) -> tauri::Result<()> {
    let desktop = app.state::<Arc<Desktop>>();
    let _mount = desktop.preferences.1.lock().expect("settings mount");
    let owner = crate::windows::MAIN;
    close(app, owner);
    crate::windows::go_main(app, crate::windows::local_url(SHELL))?;
    let window = app.get_window(owner).ok_or(tauri::Error::WindowNotFound)?;
    let size = window
        .inner_size()?
        .to_logical::<f64>(window.scale_factor()?);
    let path = format!(
        "index.html#{}",
        if page == "privacy" { "folders" } else { page }
    );
    let builder = WebviewBuilder::new(child_label(owner), WebviewUrl::App(path.into()))
        .background_color(crate::windows::canvas_color(
            window.theme().ok() == Some(tauri::Theme::Dark),
        ))
        .auto_resize()
        .on_navigation(is_document)
        .on_new_window(|_, _| NewWindowResponse::Deny);
    let child = window.add_child(builder, LogicalPosition::new(0.0, 0.0), size)?;
    place_child(
        &child,
        Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: size.into(),
        },
    )
    .map_err(|e| tauri::Error::Anyhow(anyhow::anyhow!(e)))?;
    #[cfg(target_os = "linux")]
    child.with_webview(|handle| {
        use gtk::prelude::*;
        let view = handle.inner();
        view.set_halign(gtk::Align::Fill);
        view.set_valign(gtk::Align::Fill);
        view.set_hexpand(true);
        view.set_vexpand(true);
        view.set_size_request(-1, -1);
    })?;
    window.show()?;
    window.set_focus()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_preparation_cannot_replace_a_newer_question_or_survive_navigation() {
        let panels = Panels::default();
        panels.register("main");
        let request = cww::control::ControlRequest::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        let context = json!({"grant":{"account":"1000001"}});
        let first = panels.begin_confirmation("main").unwrap();
        let second = panels.begin_confirmation("main").unwrap();
        let token = panels
            .prepare("main", &second, &request, context.clone())
            .unwrap();
        assert!(
            panels
                .prepare("main", &first, &request, context.clone())
                .is_err()
        );
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &request, &context)
                .is_ok()
        );

        let pending = panels.begin_confirmation("main").unwrap();
        {
            let mut entries = panels.0.lock().unwrap();
            let entry = entries.get_mut("main").unwrap();
            entry.page = Some("folders".into());
            entry.placement("general", &Value::Null, Rect::default());
        }
        assert!(
            panels
                .prepare("main", &pending, &request, context.clone())
                .is_err()
        );

        // Closing/recreating the same owner label cannot reuse a reservation.
        let pending = panels.begin_confirmation("main").unwrap();
        panels.register("main");
        assert!(panels.prepare("main", &pending, &request, context).is_err());
    }

    #[test]
    fn standalone_confirmation_is_bound_to_server_and_device_without_remote_context() {
        let request = cww::control::ControlRequest::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        let mut status = crate::model::Status {
            server: Some("https://chatwithwork.com".into()),
            device_id: Some("43".into()),
            roots: vec![crate::model::RootStatus {
                id: "work".into(),
                local_path: Some("/work".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let panels = Panels::default();
        panels.register("main");
        let original = confirmation_context(&request, &status, &Value::Null).unwrap();
        let token = panels
            .prepare_for_test("main", &request, original.clone())
            .unwrap();
        status.device_id = Some("44".into());
        assert!(
            panels
                .consume_confirmation(
                    "main",
                    Some(&token),
                    &request,
                    &confirmation_context(&request, &status, &Value::Null).unwrap()
                )
                .is_err()
        );
        status.device_id = Some("43".into());
        let token = panels.prepare_for_test("main", &request, original).unwrap();
        status.server = Some("https://another.example".into());
        assert!(
            panels
                .consume_confirmation(
                    "main",
                    Some(&token),
                    &request,
                    &confirmation_context(&request, &status, &Value::Null).unwrap()
                )
                .is_err()
        );
    }

    #[test]
    fn inline_confirmation_is_one_use_and_bound_to_window_request_folder_and_account() {
        use cww::control::ControlRequest as R;
        let panels = Panels::default();
        panels.register("main");
        panels.register("page-1");
        let request = R::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        let context =
            json!({"grant":{"account":"1000001","device_id":"43"},"folder":{"path":"/work"}});
        let token = panels
            .prepare_for_test("main", &request, context.clone())
            .unwrap();
        assert!(
            panels
                .consume_confirmation("page-1", Some(&token), &request, &context)
                .is_err()
        );
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &request, &context)
                .is_ok()
        );
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &request, &context)
                .is_err()
        );
        let token = panels
            .prepare_for_test("main", &request, context.clone())
            .unwrap();
        let other = R::RootsCommands {
            root: "work".into(),
            commands: true,
            network: false,
        };
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &other, &context)
                .is_err()
        );
        for changed in [
            json!({"grant":{"account":"2000002"},"folder":{"path":"/work"}}),
            json!({"grant":{"account":"1000001","device_id":"43"},"folder":{"path":"/another-folder"}}),
        ] {
            let token = panels
                .prepare_for_test("main", &request, context.clone())
                .unwrap();
            assert!(
                panels
                    .consume_confirmation("main", Some(&token), &request, &changed)
                    .is_err()
            );
        }
        let token = panels
            .prepare_for_test("main", &request, context.clone())
            .unwrap();
        panels.cancel_confirmation("main", &token);
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &request, &context)
                .is_err()
        );
    }

    #[test]
    fn inline_question_freezes_placement_but_navigation_and_expiry_invalidate_it() {
        use cww::control::ControlRequest as R;
        let panels = Panels::default();
        panels.register("main");
        let original = Rect {
            position: LogicalPosition::new(400.0, 80.0).into(),
            size: LogicalSize::new(600.0, 700.0).into(),
        };
        let moved = Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            ..original
        };
        {
            let mut entries = panels.0.lock().unwrap();
            let entry = entries.get_mut("main").unwrap();
            entry.page = Some("folders".into());
            entry.bounds = Some(original);
        }
        let request = R::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        let context = json!({"account":"1000001","device_id":"43"});
        panels
            .prepare_for_test("main", &request, json!({"grant":context}))
            .unwrap();
        let mut entries = panels.0.lock().unwrap();
        let entry = entries.get_mut("main").unwrap();
        let (placement, cancelled) = entry.placement("folders", &context, moved);
        assert_eq!(placement.position, original.position);
        assert_eq!(placement.size, original.size);
        assert!(!cancelled);
        let (placement, cancelled) = entry.placement("activity", &context, moved);
        assert_eq!(placement.position, moved.position);
        assert!(cancelled);
        assert!(entry.confirmation.is_none());
        drop(entries);
        panels
            .prepare_for_test("main", &request, json!({"grant":context}))
            .unwrap();
        let mut entries = panels.0.lock().unwrap();
        let entry = entries.get_mut("main").unwrap();
        entry.confirmation.as_mut().unwrap().started = Instant::now() - Duration::from_secs(121);
        let (placement, cancelled) = entry.placement("folders", &context, moved);
        assert_eq!(placement.position, moved.position);
        assert!(cancelled);
    }

    #[test]
    fn shrinking_the_client_cancels_an_inline_token_before_repositioning() {
        use cww::control::ControlRequest as R;
        let panels = Panels::default();
        panels.register("main");
        let request = R::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        let context = json!({"grant":{"account":"1000001"}});
        let original = Rect {
            position: LogicalPosition::new(544.0, 88.0).into(),
            size: LogicalSize::new(624.0, 708.0).into(),
        };
        panels.0.lock().unwrap().get_mut("main").unwrap().bounds = Some(original);
        let token = panels
            .prepare_for_test("main", &request, context.clone())
            .unwrap();
        {
            let mut entries = panels.0.lock().unwrap();
            let entry = entries.get_mut("main").unwrap();
            assert!(!entry.cancel_clipped_confirmation(LogicalSize::new(1200.0, 820.0)));
            assert!(entry.cancel_clipped_confirmation(LogicalSize::new(900.0, 700.0)));
            // Cancelling does not itself move the target. A subsequent normal
            // placement may fit the new client only after the token is dead.
            assert_eq!(entry.bounds.unwrap().position, original.position);
        }
        assert!(
            panels
                .consume_confirmation("main", Some(&token), &request, &context)
                .is_err()
        );
    }

    #[test]
    fn inline_confirmation_does_not_replace_remote_or_other_local_questions() {
        use cww::control::ControlRequest as R;
        let status = crate::model::Status {
            roots: vec![crate::model::RootStatus {
                id: "work".into(),
                label: "Work".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let request = R::RootsWritable {
            root: "work".into(),
            writable: true,
            i_know: false,
        };
        assert!(has_inline_confirmation(&request));
        assert!(question(&request, &status, &Value::Null).unwrap().is_some());
        assert!(
            crate::computer::Change::Changes {
                folder: "work".into(),
                on: true
            }
            .plan(&status)
            .unwrap()
            .1
            .is_some()
        );
        for request in [
            R::RootsCommands {
                root: "work".into(),
                commands: true,
                network: true,
            },
            R::BrowserAllSites { all_sites: true },
            R::DenyExtra {
                previous: Some("*.secret".into()),
                pattern: None,
            },
        ] {
            assert!(!has_inline_confirmation(&request));
        }
    }

    #[test]
    fn only_the_bundled_document_and_its_canonical_root_are_local_settings() {
        for (url, allowed) in [
            ("tauri://localhost/", true),
            ("tauri://localhost/index.html#folders", true),
            ("http://tauri.localhost/", true),
            ("tauri://localhost/scan.html", false),
            ("https://work.example/index.html", false),
            ("file:///index.html", false),
        ] {
            assert_eq!(is_document(&Url::parse(url).unwrap()), allowed, "{url}");
        }
    }

    #[test]
    fn only_the_paired_account_and_exact_settings_page_can_mount() {
        let context = json!({"account":"123456789", "device_id":"device"});
        for (path, allowed) in [
            ("/123456789/settings?tab=computer_folders", true),
            ("/987654321/settings?tab=computer_folders", false),
            ("/123456789/settings?tab=computer_activity", false),
            (
                "/123456789/settings?tab=computer_folders&tab=computer_activity",
                false,
            ),
            ("/123456789/chats/1?tab=computer_folders", false),
        ] {
            assert_eq!(
                page_matches(
                    &Url::parse(&format!("https://work.example{path}")).unwrap(),
                    "folders",
                    None,
                    &context
                ),
                allowed
            );
        }
    }

    #[test]
    fn a_bot_mount_needs_the_confirmed_bot_in_that_account() {
        let context = json!({"account":"123456789", "bot":{"id":42,"name":"A bot"}});
        let url = Url::parse("https://work.example/123456789/bots/42").unwrap();
        assert!(page_matches(&url, "bot-sharing", Some("42"), &context));
        assert!(!page_matches(&url, "bot-sharing", Some("43"), &context));
        assert!(!page_matches(&url, "activity", Some("42"), &context));
    }

    #[test]
    fn geometry_is_finite_clipped_and_zoomed_to_the_native_client() {
        let viewport = Viewport {
            width: 1000.0,
            height: 800.0,
        };
        let size = LogicalSize::new(1500.0, 1200.0);
        let rect = rectangle(
            Bounds {
                x: 600.0,
                y: 200.0,
                width: 800.0,
                height: 900.0,
            },
            viewport,
            size,
        )
        .unwrap();
        assert_eq!(rect.position, LogicalPosition::new(900.0, 300.0).into());
        assert_eq!(rect.size, LogicalSize::new(600.0, 900.0).into());
        for x in [f64::NAN, f64::INFINITY, 2000.0] {
            assert!(
                rectangle(
                    Bounds {
                        x,
                        y: 0.0,
                        width: 800.0,
                        height: 600.0
                    },
                    viewport,
                    size
                )
                .is_err()
            );
        }
    }

    #[test]
    fn widening_uses_native_confirmation_and_bot_scope_not_display_names() {
        use crate::model::{BotLoan, RootStatus, Status};
        use cww::control::ControlRequest as R;
        let mut status = Status {
            roots: vec![RootStatus {
                id: "work".into(),
                label: "Work".into(),
                writable: true,
                allows_commands: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let context = json!({"bot":{"id":42,"name":"Trusted bot"}});
        let grant = |bot: &str, unattended| R::RootsBot {
            root: "work".into(),
            bot: bot.into(),
            name: Some("untrusted label".into()),
            lent: true,
            unattended,
        };
        let ask = question(&grant("42", false), &status, &context)
            .unwrap()
            .unwrap();
        assert!(ask.message.contains("Trusted bot (bot 42)"));
        assert!(!ask.message.contains("untrusted label"));
        assert!(question(&grant("43", false), &status, &context).is_err());
        assert!(question(&grant("42", false), &status, &Value::Null).is_err());
        status.roots[0].bots.push(BotLoan {
            id: "42".into(),
            name: "Old name".into(),
            unattended: false,
        });
        assert!(
            question(&grant("42", false), &status, &context)
                .unwrap()
                .is_none()
        );
        let ask = question(&grant("42", true), &status, &context)
            .unwrap()
            .unwrap();
        assert!(ask.message.contains("without approval"));
        assert!(
            question(
                &R::RootsBot {
                    root: "work".into(),
                    bot: "42".into(),
                    name: None,
                    lent: false,
                    unattended: false
                },
                &status,
                &context
            )
            .unwrap()
            .is_none()
        );
        assert!(
            question(
                &R::RootsWritable {
                    root: "work".into(),
                    writable: false,
                    i_know: false,
                },
                &status,
                &context
            )
            .unwrap()
            .is_none()
        );
        assert!(
            question(
                &R::BrowserSite {
                    site: "example.com".into(),
                    allowed: true
                },
                &status,
                &context
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn allowing_all_sites_and_unblocking_require_native_confirmation() {
        use cww::control::ControlRequest as R;
        let status = crate::model::Status::default();
        for request in [
            R::BrowserAllSites { all_sites: true },
            R::BrowserAskBeforeActions { ask: false },
            R::BrowserBlockedSite {
                site: "example.com".into(),
                blocked: false,
            },
        ] {
            assert!(question(&request, &status, &Value::Null).unwrap().is_some());
        }
        for request in [
            R::BrowserAllSites { all_sites: false },
            R::BrowserAskBeforeActions { ask: true },
            R::BrowserBlockedSite {
                site: "example.com".into(),
                blocked: true,
            },
        ] {
            assert!(question(&request, &status, &Value::Null).unwrap().is_none());
        }
    }

    #[test]
    fn custom_exclusion_widening_always_asks_but_adding_only_narrows() {
        use cww::control::ControlRequest::DenyExtra;
        let status = crate::model::Status::default();
        for pattern in [None, Some("*.private".into())] {
            let request = DenyExtra {
                previous: Some("*.secret".into()),
                pattern,
            };
            let ask = question(&request, &status, &Value::Null).unwrap().unwrap();
            assert!(ask.message.contains("*.secret"));
            assert!(ask.message.contains("bots"));
        }
        assert!(
            question(
                &DenyExtra {
                    previous: None,
                    pattern: Some("*.secret".into())
                },
                &status,
                &Value::Null
            )
            .unwrap()
            .is_none()
        );
        assert!(
            question(
                &DenyExtra {
                    previous: None,
                    pattern: Some("[".into())
                },
                &status,
                &Value::Null
            )
            .is_err()
        );
    }

    #[test]
    fn route_validation_accepts_legacy_account_ids_and_refuses_unscoped_pages() {
        let url = Url::parse("https://work.example/1000001/settings?tab=computer_folders").unwrap();
        assert!(page_matches(
            &url,
            "folders",
            None,
            &json!({"account":"1000001"})
        ));
        assert!(!page_matches(
            &url,
            "folders",
            None,
            &json!({"account":"100000"})
        ));
        assert!(!page_matches(
            &url,
            "privacy",
            None,
            &json!({"account":"1000001"})
        ));
    }

    #[test]
    fn proofs_are_per_window_and_hiding_invalidates_pending_mounts() {
        let panels = Panels::default();
        let proof = panels.register("main");
        let other = panels.register("page-1");
        assert_ne!(proof, other);
        assert!(panels.begin("main", &other).is_err());
        let (first, _) = panels.begin("main", &proof).unwrap();
        let (second, _) = panels.begin("main", &proof).unwrap();
        assert!(!panels.current("main", first));
        assert!(panels.current("main", second));
    }
}
