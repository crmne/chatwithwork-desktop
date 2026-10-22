//! The app's windows: the web app (`main`, and `page-N` for pages it opens
//! in a new window) and the bundled settings (`settings`).

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::desktop::Desktop;
use crate::policy::{self, Decision};

pub const MAIN: &str = "main";
pub const SETTINGS: &str = "settings";
/// Windows the web app opens for itself (`target="_blank"` on its own pages).
pub const POPUP_PREFIX: &str = "page-";

static POPUPS: AtomicU64 = AtomicU64::new(0);

pub fn is_remote_label(label: &str) -> bool {
    label == MAIN || label.starts_with(POPUP_PREFIX)
}

/// Whether `url` is one of the bundled pages: `tauri://localhost` on Linux
/// and macOS, `http(s)://tauri.localhost` on Windows.
pub fn is_local_url(url: &Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    }
}

/// Opens `url` in the default browser or mail app, if it may go there.
pub fn open_externally(url: &Url) {
    if policy::openable_externally(url)
        && let Err(e) = open::that_detached(url.as_str())
    {
        log::warn!("couldn't open a link in the browser: {e}");
    }
}

fn bring_forward(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let _ = window
            .app_handle()
            .set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Shows the web app, at `url` or where it was (its start page when it
/// opens).
pub fn show_main(app: &AppHandle, url: Option<Url>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN) {
        if let Some(url) = url {
            navigate_main(app, url)?;
        }
        bring_forward(&window);
        return Ok(());
    }
    let desktop = app.state::<std::sync::Arc<Desktop>>();
    let url = url.unwrap_or_else(|| desktop.server().start_url());
    navigate_main(app, url)?;
    if let Some(window) = app.get_webview_window(MAIN) {
        bring_forward(&window);
    }
    Ok(())
}

/// Takes the Chat with Work window to `url`, or to the offline page while the
/// server can't be reached (it goes on to `url` once it can). Blocks for up
/// to a few seconds on the check, so never call it on the main thread.
pub fn navigate_main(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let desktop = app.state::<std::sync::Arc<Desktop>>().inner().clone();
    let target = if crate::offline::reachable(&desktop.server(), &desktop.paths.daemon) {
        url
    } else {
        log::info!("the server can't be reached; showing the offline page");
        desktop.set_wanted(url);
        local_url(crate::offline::PAGE)
    };
    match app.get_webview_window(MAIN) {
        Some(window) => window.navigate(target),
        None => remote_window(app, MAIN, target).map(drop),
    }
}

/// A bundled page's address in a web view on this platform.
pub fn local_url(page: &str) -> Url {
    let base = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };
    Url::parse(base)
        .and_then(|base| base.join(page))
        .expect("a bundled page has an address")
}

/// The web app in a window: its pages get the bridge script and nothing
/// else, and everything that leaves the server opens in the browser.
fn remote_window(app: &AppHandle, label: &str, url: Url) -> tauri::Result<WebviewWindow> {
    let desktop = app.state::<std::sync::Arc<Desktop>>().inner().clone();
    let server = desktop.server();
    let navigation = desktop.clone();
    let page_load = desktop.clone();
    // The last page sent to the browser: Started and Finished can both
    // report the same one (WebView2 reports a stopped load as finished).
    let handed_off: std::sync::Mutex<Option<Url>> = std::sync::Mutex::new(None);
    let new_window = desktop.clone();
    let webview_url = if crate::offline::is_page(&url) {
        WebviewUrl::App(crate::offline::PAGE.into())
    } else {
        WebviewUrl::External(url)
    };
    let window = WebviewWindowBuilder::new(app, label, webview_url)
        .title("Chat with Work")
        .inner_size(1200.0, 820.0)
        .min_inner_size(380.0, 480.0)
        .user_agent(&crate::user_agent::user_agent(env!("CARGO_PKG_VERSION")))
        .initialization_script(crate::bridge::script(&server))
        // Files dropped on the window go to the page as an HTML drop, so the
        // composer takes them as uploads, as in a browser.
        .disable_drag_drop_handler()
        .zoom_hotkeys_enabled(true)
        .on_navigation(move |url| {
            if crate::offline::is_page(url) {
                return true;
            }
            match policy::navigation(&navigation.server(), url) {
                Decision::Allow => true,
                Decision::External(url) => {
                    open_externally(&url);
                    false
                }
                Decision::Deny => false,
            }
        })
        .on_page_load(move |window, payload| {
            if crate::offline::is_page(payload.url()) {
                return;
            }
            let decision = policy::top_level(&page_load.server(), payload.url());
            if decision == Decision::Allow {
                *handed_off.lock().expect("handoff lock") = None;
                return;
            }
            // Stop a page from elsewhere as it starts, or leave it once it
            // has loaded: a redirect only shows its address when it finishes.
            let script = match payload.event() {
                PageLoadEvent::Started => "window.stop()",
                PageLoadEvent::Finished => "history.length > 1 ? history.back() : window.stop()",
            };
            let _ = window.eval(script);
            if let Decision::External(url) = decision {
                let mut last = handed_off.lock().expect("handoff lock");
                if last.as_ref() != Some(&url) {
                    open_externally(&url);
                    *last = Some(url);
                }
            }
        })
        .on_new_window(move |url, _features| {
            match policy::new_window(&new_window.server(), &url) {
                Decision::Allow => {
                    let app = new_window.app();
                    std::thread::spawn(move || {
                        let n = POPUPS.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Err(e) = remote_window(&app, &format!("{POPUP_PREFIX}{n}"), url) {
                            log::warn!("couldn't open a page in a new window: {e}");
                        }
                    });
                }
                Decision::External(url) => open_externally(&url),
                Decision::Deny => {}
            }
            NewWindowResponse::Deny
        })
        .on_download(crate::downloads::handle)
        .on_document_title_changed(|window, title| {
            let title = title.trim();
            let _ = window.set_title(if title.is_empty() {
                "Chat with Work"
            } else {
                title
            });
        })
        .build()?;
    #[cfg(target_os = "linux")]
    watch_failed_loads(&window, desktop);
    Ok(window)
}

/// Shows the offline page when one of the server's pages fails to load and
/// the server can't be reached, in place of WebKitGTK's error page.
#[cfg(target_os = "linux")]
fn watch_failed_loads(window: &WebviewWindow, desktop: std::sync::Arc<Desktop>) {
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    let _ = window.with_webview(move |webview| {
        use webkit2gtk::{NetworkError, PolicyError, WebViewExt};
        webview
            .inner()
            .connect_load_failed(move |_view, _event, uri, error| {
                let ours = Url::parse(uri).is_ok_and(|url| desktop.server().owns(&url));
                let ignored =
                    error.matches(NetworkError::Cancelled) || error.kind::<PolicyError>().is_some();
                if !ours || ignored || label != MAIN {
                    return false;
                }
                let (app, desktop, url) = (
                    app.clone(),
                    desktop.clone(),
                    Url::parse(uri).expect("checked above"),
                );
                std::thread::spawn(move || {
                    if crate::offline::reachable(&desktop.server(), &desktop.paths.daemon) {
                        return;
                    }
                    desktop.set_wanted(url);
                    if let Some(window) = app.get_webview_window(MAIN) {
                        let _ = window.navigate(local_url(crate::offline::PAGE));
                    }
                });
                // WebKitGTK's page shows until the check decides; while the
                // server answers, that page is the right one.
                false
            });
    });
}

/// Hands an event to the settings pages (`status`, `pairing`, `navigate`,
/// `dictation-download`), if the window is open on a bundled page. Through
/// the page's own `window.cwwEvent` rather than Tauri's event system, so the
/// pages need no core permissions at all.
pub fn tell_settings(app: &AppHandle, event: &str, payload: impl serde::Serialize) {
    let Some(window) = app.get_webview_window(SETTINGS) else {
        return;
    };
    if !window.url().is_ok_and(|url| is_local_url(&url)) {
        return;
    }
    let call = settings_event(event, &serde_json::to_value(payload).unwrap_or_default());
    let _ = window.eval(call);
}

/// The JavaScript that delivers one event: its name and payload go in as
/// JSON, never as script.
pub fn settings_event(event: &str, payload: &serde_json::Value) -> String {
    format!(
        "window.cwwEvent && window.cwwEvent({}, {payload})",
        serde_json::Value::from(event)
    )
}

/// Shows the settings window, on `page` or where it was.
pub fn show_settings(app: &AppHandle, page: Option<&str>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        if let Some(page) = page {
            tell_settings(app, "navigate", page);
        }
        bring_forward(&window);
        return Ok(());
    }
    let path = match page {
        Some(page) => format!("index.html#{page}"),
        None => "index.html".to_string(),
    };
    // A fixed size for pictures, which tiling compositors leave floating.
    let fixed = snapshots();
    let window = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App(path.into()))
        .title("Chat with Work Desktop Settings")
        .inner_size(920.0, 660.0)
        .resizable(!fixed)
        .min_inner_size(560.0, 420.0)
        .zoom_hotkeys_enabled(true)
        .on_navigation(|url| {
            if is_local_url(url) || matches!(url.scheme(), "about" | "data" | "blob") {
                return true;
            }
            open_externally(url);
            false
        })
        .on_new_window(|url, _| {
            open_externally(&url);
            NewWindowResponse::Deny
        })
        .build()?;
    bring_forward(&window);
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "demo"))]
fn snapshots() -> bool {
    crate::snapshot::requested().is_some()
}

#[cfg(not(all(target_os = "linux", feature = "demo")))]
fn snapshots() -> bool {
    false
}

/// Whether any window is open, for macOS's Dock icon.
#[cfg(target_os = "macos")]
pub fn any_open(app: &AppHandle) -> bool {
    !app.webview_windows().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_pages_are_recognized_on_every_platform() {
        for local in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/index.html#folders",
            "https://tauri.localhost/",
        ] {
            assert!(is_local_url(&Url::parse(local).unwrap()), "{local}");
        }
        for remote in [
            "https://chatwithwork.com/",
            "tauri://evil/",
            "http://tauri.localhost.evil.example/",
            "http://localhost/",
        ] {
            assert!(!is_local_url(&Url::parse(remote).unwrap()), "{remote}");
        }
    }

    #[test]
    fn events_reach_the_pages_as_data() {
        let call = settings_event(
            "pairing",
            &serde_json::json!({ "message": "\");alert(1);//" }),
        );
        assert_eq!(
            call,
            r#"window.cwwEvent && window.cwwEvent("pairing", {"message":"\");alert(1);//"})"#
        );
    }

    #[test]
    fn only_the_web_apps_windows_are_remote() {
        assert!(is_remote_label(MAIN));
        assert!(is_remote_label("page-3"));
        assert!(!is_remote_label(SETTINGS));
    }
}
