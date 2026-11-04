//! The update banner in the Chat with Work window, as ZapFast, Spotifast
//! and TonePush show theirs: a card at the top right of the window, over
//! whatever page it shows, while a release waits. A copy that installs
//! updates itself offers Restart to Update; a copy a package manager updates
//! shows the command for it with Copy (update_notice.rs). Dismissing it
//! hides it until the next version (`dismissed_update` in app.json).
//!
//! It's a bundled page (`banner.html`) in an app-owned child web view, like
//! the settings pane, with commands of its own: the server's page under it
//! can't reach them, and the banner opens only the links the updater holds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Rect, State, Webview, WebviewUrl};

use crate::desktop::Desktop;

pub const LABEL: &str = "update-banner";
const PAGE: &str = "banner.html";
const WIDTH: f64 = 440.0;
const MARGIN: f64 = 16.0;

/// The card's height for what it shows: a line and Restart to Update, or
/// the notice with its command.
fn height(view: &Value) -> f64 {
    match (view["kind"].as_str(), view["command"].is_string()) {
        (Some("restart"), _) => 76.0,
        (_, true) => 156.0,
        _ => 112.0,
    }
}

/// Where the card goes in a window `size` wide: the top right corner, or
/// across a narrow window.
fn bounds(size: LogicalSize<f64>, height: f64) -> Rect {
    let width = WIDTH.min((size.width - 2.0 * MARGIN).max(200.0));
    Rect {
        position: LogicalPosition::new((size.width - width - MARGIN).max(0.0), MARGIN).into(),
        size: LogicalSize::new(width, height).into(),
    }
}

/// Whether the main window already moves the card when it's resized.
static FOLLOWS: AtomicBool = AtomicBool::new(false);

/// Shows the banner for `view` in the Chat with Work window, updates it, or
/// takes it away (`None`). Call off the main thread.
pub fn sync(app: &AppHandle, view: Option<Value>) {
    let Some(window) = app.get_window(crate::windows::MAIN) else {
        return;
    };
    let existing = app.get_webview(LABEL);
    let Some(view) = view else {
        if let Some(banner) = existing {
            let _ = banner.close();
        }
        *FITTED.lock().expect("banner height") = None;
        return;
    };
    let Ok(size) = window
        .inner_size()
        .and_then(|size| Ok(size.to_logical::<f64>(window.scale_factor()?)))
    else {
        return;
    };
    let fitted = *FITTED.lock().expect("banner height");
    let rect = bounds(size, fitted.unwrap_or_else(|| height(&view)));
    let banner = match existing {
        Some(banner) => banner,
        None => {
            let builder = WebviewBuilder::new(LABEL, WebviewUrl::App(PAGE.into()))
                .on_navigation(is_page)
                .on_new_window(|_, _| NewWindowResponse::Deny);
            match window.add_child(builder, LogicalPosition::new(0.0, 0.0), size) {
                Ok(banner) => banner,
                Err(e) => {
                    log::warn!("no update banner: {e}");
                    return;
                }
            }
        }
    };
    if let Err(e) = crate::preferences::place(&banner, rect) {
        log::warn!("couldn't place the update banner: {e}");
    }
    crate::windows::tell_window(app, LABEL, "update", &view);
    if !FOLLOWS.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        window.on_window_event(move |event| match event {
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                let app = app.clone();
                std::thread::spawn(move || {
                    if app.get_webview(LABEL).is_some() {
                        let desktop = app.state::<Arc<Desktop>>();
                        sync(&app, desktop.updates.banner());
                    }
                });
            }
            tauri::WindowEvent::Destroyed => FOLLOWS.store(false, Ordering::SeqCst),
            _ => {}
        });
    }
}

/// Puts the banner back on top after a pane went over it.
pub fn raise(app: &AppHandle) {
    if let Some(banner) = app.get_webview(LABEL) {
        let _ = banner.close();
        let app = app.clone();
        std::thread::spawn(move || {
            let desktop = app.state::<Arc<Desktop>>();
            sync(&app, desktop.updates.banner());
        });
    }
}

fn is_page(url: &tauri::Url) -> bool {
    crate::windows::is_local_url(url) && url.path() == format!("/{PAGE}")
}

/// Only the banner's own page may use its commands.
fn banner_page(webview: &Webview) -> Result<(), Value> {
    if webview.label() == LABEL && webview.url().is_ok_and(|url| is_page(&url)) {
        Ok(())
    } else {
        Err(json!({ "code": "forbidden", "message": "Only the update banner can ask for this." }))
    }
}

type Answer = Result<Value, Value>;

/// The card's height, as its page measured it once drawn.
static FITTED: std::sync::Mutex<Option<f64>> = std::sync::Mutex::new(None);

/// Sizes the card to what its page drew.
#[tauri::command]
pub fn banner_fit(webview: Webview, app: AppHandle, height: f64) -> Answer {
    banner_page(&webview)?;
    if !height.is_finite() {
        return Err(json!({ "code": "invalid", "message": "Not a height." }));
    }
    *FITTED.lock().expect("banner height") = Some(height.clamp(56.0, 320.0));
    std::thread::spawn(move || {
        let desktop = app.state::<Arc<Desktop>>();
        sync(&app, desktop.updates.banner());
    });
    Ok(json!({}))
}

/// What the banner shows now.
#[tauri::command]
pub fn banner_state(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    banner_page(&webview)?;
    Ok(desktop.updates.banner().unwrap_or(Value::Null))
}

/// Restart to Update, as the tray's.
#[tauri::command]
pub fn banner_restart(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    banner_page(&webview)?;
    desktop
        .restart_to_update()
        .map(|()| json!({}))
        .map_err(|e| json!({ "code": "failed", "message": e }))
}

/// Hides the banner (and a package install's notice) until the next
/// version.
#[tauri::command]
pub fn banner_dismiss(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    banner_page(&webview)?;
    desktop.dismiss_update();
    Ok(json!({}))
}

/// Opens the release notes (`notes`) or the file to download
/// (`download`) in the browser: links the updater holds, never one the
/// page names.
#[tauri::command]
pub fn banner_open(webview: Webview, desktop: State<'_, Arc<Desktop>>, which: String) -> Answer {
    banner_page(&webview)?;
    let url = desktop
        .updates
        .banner_link(&which)
        .ok_or_else(|| json!({ "code": "invalid", "message": "There's no such link." }))?;
    open::that_detached(&url)
        .map(|()| json!({}))
        .map_err(|e| json!({ "code": "failed", "message": format!("Couldn't open the link: {e}") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_sits_top_right_and_fits_narrow_windows() {
        let wide = bounds(LogicalSize::new(1200.0, 820.0), 76.0);
        assert_eq!(wide.position, LogicalPosition::new(744.0, 16.0).into());
        assert_eq!(wide.size, LogicalSize::new(440.0, 76.0).into());
        let narrow = bounds(LogicalSize::new(380.0, 600.0), 148.0);
        assert_eq!(narrow.size, LogicalSize::new(348.0, 148.0).into());
        assert_eq!(narrow.position, LogicalPosition::new(16.0, 16.0).into());
    }

    #[test]
    fn its_height_follows_what_it_shows() {
        assert_eq!(height(&json!({ "kind": "restart" })), 76.0);
        assert_eq!(
            height(&json!({ "kind": "notice", "command": "brew upgrade cww" })),
            156.0
        );
        assert_eq!(height(&json!({ "kind": "notice", "command": null })), 112.0);
    }

    #[test]
    fn only_its_own_page_counts() {
        assert!(is_page(&crate::windows::local_url("banner.html")));
        assert!(!is_page(&crate::windows::local_url("index.html")));
        assert!(!is_page(
            &tauri::Url::parse("https://chatwithwork.com/banner.html").unwrap()
        ));
    }
}
