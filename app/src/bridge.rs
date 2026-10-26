//! The bridge: the little the web app's pages may ask of the app. It is
//! the only IPC remote content gets, and only pages of the server the window
//! belongs to get it (a Tauri capability scoped to that origin, added in
//! desktop.rs, and each command checks the caller's page again).
//!
//! The page sees it as `window.ChatWithWorkDesktop` (see
//! docs/desktop-server-contract.md):
//!
//! - `openExternal(url)`: a link in the default browser.
//! - `openPreferences(page)`: the app's settings window, on one of its pages.
//! - `listen()`: a loopback callback for a sign-in or connection that runs
//!   in the default browser (loopback.rs).
//! - `computer`: this computer's shared folders and switches, which a page
//!   may narrow at once and widen only after the person says yes in the
//!   app's own dialog (computer.rs).
//! - `scan()`: the scan window, whose PDF comes back to the page as a file
//!   for its composer, once the person scans and attaches it (scan.rs).
//! - Dialogs: Turbo's `data-turbo-confirm`, `confirm()` and `alert()` show
//!   in the system's message box (`page_dialog`), since the dialog plugin's
//!   script points the last two at a command these pages may not call.
//!
//! Notifications are not part of it: the daemon already shows native ones,
//! open or closed, and the page tells the server what's on screen.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State, Url, Webview};

use crate::desktop::Desktop;
use crate::server::Server;

/// The script every page of the main window runs before its own. It does
/// nothing on pages from anywhere else.
pub fn script(server: &Server) -> String {
    const TEMPLATE: &str = include_str!("bridge.js");
    TEMPLATE
        .replace("__ORIGIN__", &json!(server.origin()).to_string())
        .replace(
            "__COMPONENTS__",
            &json!(crate::user_agent::COMPONENTS).to_string(),
        )
        .replace("__VERSION__", &json!(env!("CARGO_PKG_VERSION")).to_string())
}

/// The caller's page, if it is the server's and in one of the web app's
/// windows.
pub fn server_page(desktop: &Desktop, webview: &Webview) -> Result<Url, String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    if crate::windows::is_remote_label(webview.label()) && desktop.server().owns(&url) {
        Ok(url)
    } else {
        Err("Only Chat with Work's own pages can ask for this.".into())
    }
}

#[tauri::command]
pub fn open_external(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    url: String,
) -> Result<(), String> {
    server_page(&desktop, &webview)?;
    let url = Url::parse(&url).map_err(|_| "That isn't a link.".to_string())?;
    if !crate::policy::openable_externally(&url) {
        return Err("Only web and mail links open outside the app.".into());
    }
    open::that_detached(url.as_str()).map_err(|e| format!("Couldn't open the link: {e}"))
}

/// Async: a window made from a synchronous command, which runs on the main
/// thread, can deadlock on Windows. `page` is one of the settings window's
/// own pages; anything else opens it where it was.
#[tauri::command]
pub async fn open_preferences(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    page: Option<String>,
) -> Result<(), String> {
    server_page(&desktop, &webview)?;
    let page = page.as_deref().and_then(crate::args::page);
    crate::windows::show_settings(&app, page).map_err(|e| e.to_string())
}

/// What the page may see of this computer (computer.rs).
#[tauri::command]
pub fn computer(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    Ok(crate::computer::view(desktop.status().as_ref()))
}

/// A change to this computer's sharing that a page asked for: at once when
/// it narrows, after the person's yes in the app's own dialog when it
/// widens. Answers what the page may see of the computer afterwards, or
/// `{ "declined": true }` when the person said no.
#[tauri::command]
pub async fn computer_change(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    change: Value,
) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    let change: crate::computer::Change = serde_json::from_value(change)
        .map_err(|_| "Chat with Work Desktop can't do that.".to_string())?;
    let desktop = desktop.inner().clone();
    let label = webview.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let status = desktop.client.status().map_err(|e| e.to_string())?;
        let (request, question) = change.plan(&status)?;
        if let Some(question) = question {
            let Some(_asking) = desktop.asking.start() else {
                return Err(
                    "Answer the question Chat with Work Desktop is asking first.".to_string(),
                );
            };
            if !confirm(&app, &label, &question) {
                return Ok(json!({ "declined": true }));
            }
        }
        desktop
            .client
            .request(&request)
            .map_err(|e| e.to_string())?;
        let status = desktop.client.status().ok();
        Ok(crate::computer::view(status.as_ref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shares a folder the person picks in the system's folder picker. The page
/// can't name one. Answers the folder's label, or `null` if they picked none.
#[tauri::command]
pub async fn computer_add_folder(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    let desktop = desktop.inner().clone();
    let parent = app.get_webview_window(webview.label());
    tauri::async_runtime::spawn_blocking(move || {
        let Some(_asking) = desktop.asking.start() else {
            return Err("Answer the question Chat with Work Desktop is asking first.".to_string());
        };
        let Some(path) = pick_folder(&app, parent.as_ref()) else {
            return Ok(json!({ "folder": null }));
        };
        let answer = desktop
            .client
            .add_root(&path, None)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "folder": answer["root"]["label"] }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Native notifications on this computer: whether they can show, and with
/// `ask`, the system's own question to allow them.
#[tauri::command]
pub async fn computer_notifications(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    ask: Option<bool>,
) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    let client = desktop.client.clone();
    tauri::async_runtime::spawn_blocking(move || {
        use cww::control::ControlRequest;
        if ask == Some(true) {
            client
                .request(&ControlRequest::NotificationPermission { request: true })
                .map_err(|e| e.to_string())?;
        }
        let answer = client
            .request(&ControlRequest::NotificationStatus)
            .map_err(|e| e.to_string())?;
        // Only what the page shows: never the broker's internals.
        Ok(json!({
            "permission": answer["permission"],
            "connected": answer["connected"],
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A dialog a page asked for: a confirmation (Turbo's `data-turbo-confirm`)
/// or an alert. Its words are the page's, so it is titled with the server's
/// name, as a browser's are, and never reads as one of the app's own
/// questions; answering it changes nothing on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDialog {
    kind: PageDialogKind,
    message: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    confirm: Option<String>,
    #[serde(default)]
    destructive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PageDialogKind {
    Confirm,
    Alert,
}

/// The longest message a page's dialog shows, and its button's label.
const DIALOG_TEXT: usize = 1000;
const DIALOG_LABEL: usize = 32;

impl PageDialog {
    pub fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|_| "Chat with Work Desktop can't show that.".into())
    }

    /// The message, with the detail under it, clipped.
    fn text(&self) -> String {
        let text = match self.detail.as_deref().map(str::trim) {
            Some(detail) if !detail.is_empty() => format!("{}\n\n{detail}", self.message.trim()),
            _ => self.message.trim().to_string(),
        };
        clip(&text, DIALOG_TEXT)
    }

    /// The button that says yes: the page's label on one line, else "OK".
    fn yes(&self) -> String {
        let label: String = self
            .confirm
            .as_deref()
            .unwrap_or_default()
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        match clip(label.trim(), DIALOG_LABEL) {
            label if label.is_empty() => "OK".into(),
            label => label,
        }
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(limit - 1).collect();
    clipped.push('…');
    clipped
}

/// Shows a page's dialog over its window, and answers whether the person
/// confirmed it (an alert answers `true` once dismissed). One question shows
/// at a time, the app's own included: while another is open, a confirmation
/// answers no and an alert is dropped.
#[tauri::command]
pub async fn page_dialog(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    dialog: Value,
) -> Result<bool, String> {
    server_page(&desktop, &webview)?;
    let dialog = PageDialog::parse(dialog)?;
    let title = desktop.server().host();
    let desktop = desktop.inner().clone();
    let label = webview.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(_asking) = desktop.asking.start() else {
            return false;
        };
        show_page_dialog(&app, &label, &title, &dialog)
    })
    .await
    .map_err(|e| e.to_string())
}

fn show_page_dialog(app: &AppHandle, label: &str, title: &str, dialog: &PageDialog) -> bool {
    #[cfg(feature = "demo")]
    if let Ok(answer) = std::env::var("CWW_DEMO_CONFIRM") {
        log::info!("page dialog ({:?}) answered {answer}", dialog.kind);
        return answer == "yes" || dialog.kind == PageDialogKind::Alert;
    }
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let (kind, buttons) = match dialog.kind {
        PageDialogKind::Alert => (MessageDialogKind::Info, MessageDialogButtons::Ok),
        PageDialogKind::Confirm => (
            if dialog.destructive {
                MessageDialogKind::Warning
            } else {
                MessageDialogKind::Info
            },
            MessageDialogButtons::OkCancelCustom(dialog.yes(), "Cancel".into()),
        ),
    };
    let mut message = app
        .dialog()
        .message(dialog.text())
        .title(title)
        .kind(kind)
        .buttons(buttons);
    if let Some(window) = app.get_webview_window(label) {
        message = message.parent(&window);
    }
    message.blocking_show() || dialog.kind == PageDialogKind::Alert
}

/// Asks the person in a dialog the app draws, over the page's window.
/// Demo builds can answer it for pictures and checks
/// (`CWW_DEMO_CONFIRM=yes|no`); real builds always ask.
fn confirm(app: &AppHandle, label: &str, question: &crate::computer::Question) -> bool {
    #[cfg(feature = "demo")]
    if let Ok(answer) = std::env::var("CWW_DEMO_CONFIRM") {
        return answer == "yes";
    }
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let mut dialog = app
        .dialog()
        .message(&question.message)
        .title(&question.title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            question.yes.clone(),
            "Cancel".into(),
        ));
    if let Some(window) = app.get_webview_window(label) {
        dialog = dialog.parent(&window);
    }
    dialog.blocking_show()
}

fn pick_folder(
    app: &AppHandle,
    parent: Option<&tauri::WebviewWindow>,
) -> Option<std::path::PathBuf> {
    #[cfg(feature = "demo")]
    if let Some(path) = std::env::var_os("CWW_DEMO_PICK_FOLDER") {
        return Some(path.into());
    }
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app.dialog().file().set_title("Share a Folder");
    if let Some(documents) = crate::paths::documents_dir() {
        dialog = dialog.set_directory(documents);
    }
    if let Some(window) = parent {
        dialog = dialog.set_parent(window);
    }
    dialog
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
}

/// Opens the scan window for the page's composer, and answers once the
/// person attaches a scan (`{ id, name, pages, size }`, whose bytes
/// `scan_file` hands over once) or closes the window (`null`). Opening the
/// window is all a page can do: the person scans and chooses to attach.
#[tauri::command]
pub async fn scan(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<crate::scan::Scans>>,
) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    let (request, answer) = scans.attach_for(webview.label());
    let opener = app.clone();
    let waiting = scans.inner().clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::windows::show_scan(&opener) {
            log::warn!("couldn't open the scan window: {e}");
            waiting.cancel_attach(request);
            return;
        }
        crate::scan::tell_attaching(&opener, true);
    });
    Ok(answer.await.ok().flatten().unwrap_or(Value::Null))
}

/// A scan the person sent to a new chat from the scan window, for the new
/// chat's composer to take (`{ id, name, pages, size }`, or `null`).
#[tauri::command]
pub fn scan_pending(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<crate::scan::Scans>>,
) -> Result<Value, String> {
    server_page(&desktop, &webview)?;
    Ok(scans.pending_for(webview.label()).unwrap_or(Value::Null))
}

/// The PDF a `scan` answered, once, to the page that asked for it, as bytes.
#[tauri::command]
pub fn scan_file(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<crate::scan::Scans>>,
    id: String,
) -> Result<tauri::ipc::Response, String> {
    server_page(&desktop, &webview)?;
    scans
        .take_attached(webview.label(), &id)
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "That scan is gone. Scan it again.".to_string())
}

/// Starts a loopback callback and answers its URL. When the browser comes
/// back, its query reaches the page as a `chatwithwork-desktop:callback`
/// event, if the window still shows a page of the server.
#[tauri::command]
pub fn listen(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<serde_json::Value, String> {
    server_page(&desktop, &webview)?;
    let label = webview.label().to_string();
    let deliver_to = desktop.inner().clone();
    let started = desktop
        .callbacks
        .start(move |callback, params| {
            let detail = json!({ "callback": callback, "params": params });
            deliver_to.deliver_callback(&label, detail);
        })
        .map_err(|e| format!("Couldn't wait for the browser: {e}"))?;
    Ok(json!({ "callback": started.callback }))
}

/// The JavaScript that hands a callback to the page.
pub fn callback_event(detail: &serde_json::Value) -> String {
    format!(
        "window.dispatchEvent(new CustomEvent(\"chatwithwork-desktop:callback\", {{ detail: {detail} }}))"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_names_only_its_server_and_escapes_what_it_inlines() {
        let script = script(&Server::parse("https://work.example").unwrap());
        assert!(script.contains(r#"const ORIGIN = "https://work.example";"#));
        assert!(script.contains(r#""auth-session""#));
        assert!(!script.contains("__ORIGIN__") && !script.contains("__COMPONENTS__"));
    }

    #[test]
    fn callback_details_go_in_as_json_not_script() {
        let event = callback_event(&json!({ "params": { "message": "\");alert(1);//" } }));
        assert!(event.contains(r#"{"params":{"message":"\");alert(1);//"}}"#));
    }

    #[test]
    fn a_page_dialog_takes_the_pages_words_and_nothing_else() {
        let dialog = PageDialog::parse(json!({
            "kind": "confirm",
            "message": " Disconnect Google Drive? ",
            "detail": "Its cached files are deleted.",
            "confirm": "Disconnect",
            "destructive": true,
        }))
        .unwrap();
        assert_eq!(dialog.kind, PageDialogKind::Confirm);
        assert!(dialog.destructive);
        assert_eq!(
            dialog.text(),
            "Disconnect Google Drive?\n\nIts cached files are deleted."
        );
        assert_eq!(dialog.yes(), "Disconnect");

        let alert = PageDialog::parse(json!({ "kind": "alert", "message": "Saved." })).unwrap();
        assert_eq!(alert.kind, PageDialogKind::Alert);
        assert_eq!(alert.text(), "Saved.");
        assert_eq!(alert.yes(), "OK");
        assert!(!alert.destructive);

        for refused in [
            json!({ "kind": "prompt", "message": "Name?" }),
            json!({ "kind": "confirm" }),
            json!({ "kind": "confirm", "message": "Sure?", "title": "Chat with Work Desktop" }),
            json!("Sure?"),
        ] {
            assert!(PageDialog::parse(refused.clone()).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_page_dialog_is_clipped_and_its_button_stays_on_one_line() {
        let dialog = PageDialog::parse(json!({
            "kind": "confirm",
            "message": "x".repeat(5000),
            "confirm": format!("Yes\n{}", "y".repeat(100)),
        }))
        .unwrap();
        assert_eq!(dialog.text().chars().count(), DIALOG_TEXT);
        assert!(dialog.text().ends_with('…'));
        assert!(!dialog.yes().contains('\n'));
        assert_eq!(dialog.yes().chars().count(), DIALOG_LABEL);

        let blank =
            PageDialog::parse(json!({ "kind": "confirm", "message": "Sure?", "confirm": " \t " }))
                .unwrap();
        assert_eq!(blank.yes(), "OK");
    }
}
