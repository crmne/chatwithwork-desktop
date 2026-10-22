//! The bridge: the little the web app's pages may ask of the app. It is
//! the only IPC remote content gets, and only pages of the server the window
//! belongs to get it (a Tauri capability scoped to that origin, added in
//! desktop.rs, and each command checks the caller's page again).
//!
//! The page sees it as `window.ChatWithWorkDesktop` (see
//! docs/desktop-server-contract.md):
//!
//! - `openExternal(url)`: a link in the default browser.
//! - `openPreferences()`: the app's settings window.
//! - `listen()`: a loopback callback for a sign-in or connection that runs
//!   in the default browser (loopback.rs).
//!
//! Notifications are not part of it: the daemon already shows native ones,
//! open or closed, and the page tells the server what's on screen.

use std::sync::Arc;

use serde_json::json;
use tauri::{AppHandle, State, Url, Webview};

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
/// thread, can deadlock on Windows.
#[tauri::command]
pub async fn open_preferences(
    app: AppHandle,
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<(), String> {
    server_page(&desktop, &webview)?;
    crate::windows::show_settings(&app, None).map_err(|e| e.to_string())
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
}
