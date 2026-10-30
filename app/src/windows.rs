//! The app's windows: the web app (`main`, and `page-N` for pages it opens
//! in a new window) and the bundled settings (`settings`).

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::desktop::Desktop;
use crate::policy::{self, Decision};

pub const MAIN: &str = "main";
/// The bundled scan page (scan.rs).
pub const SCAN: &str = "scan";
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

pub fn is_settings_shell(url: &Url) -> bool {
    is_local_url(url) && url.path() == format!("/{}", crate::preferences::SHELL)
}

/// Opens `url` in the default browser or mail app, if it may go there.
pub fn open_externally(url: &Url) {
    if policy::openable_externally(url)
        && let Err(e) = open::that_detached(url.as_str())
    {
        log::warn!("couldn't open a link in the browser: {e}");
    }
}

fn bring_forward(window: &tauri::Window) {
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
    if let Some(window) = app.get_webview(MAIN) {
        if let Some(url) = url {
            navigate_main(app, url)?;
        } else if window.url().is_ok_and(|url| is_settings_shell(&url)) {
            let desktop = app.state::<std::sync::Arc<Desktop>>();
            navigate_main(app, desktop.server().start_url())?;
        } else {
            crate::preferences::hide(app, MAIN);
        }
        bring_forward(&window.window());
        return Ok(());
    }
    let desktop = app.state::<std::sync::Arc<Desktop>>();
    let url = url.unwrap_or_else(|| desktop.server().start_url());
    navigate_main(app, url)?;
    if let Some(window) = app.get_webview(MAIN) {
        bring_forward(&window.window());
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
    go_main(app, target)
}

/// Takes the Chat with Work window to `url` without checking the server,
/// making the window if it isn't open.
pub fn go_main(app: &AppHandle, url: Url) -> tauri::Result<()> {
    crate::preferences::close(app, MAIN);
    match app.get_webview(MAIN) {
        Some(window) => window.navigate(url),
        None => remote_window(app, MAIN, url).map(drop),
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
    let webview_url = if crate::offline::is_page(&url) || is_settings_shell(&url) {
        WebviewUrl::App(url.path().trim_start_matches('/').into())
    } else {
        WebviewUrl::External(url)
    };
    let window = WebviewWindowBuilder::new(app, label, webview_url)
        .title("Chat with Work")
        .inner_size(1200.0, 820.0)
        .resizable(!snapshots())
        .min_inner_size(380.0, 480.0)
        .user_agent(&crate::user_agent::user_agent(env!("CARGO_PKG_VERSION")))
        .initialization_script(crate::bridge::script(
            &server,
            &desktop.preferences.register(label),
        ))
        // Files dropped on the window go to the page as an HTML drop, so the
        // composer takes them as uploads, as in a browser.
        .disable_drag_drop_handler()
        .zoom_hotkeys_enabled(true)
        .on_navigation(move |url| {
            if crate::offline::is_page(url) || is_settings_shell(url) {
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
            if payload.event() == PageLoadEvent::Started && !is_settings_shell(payload.url()) {
                crate::preferences::close(window.app_handle(), window.label());
            }
            if crate::offline::is_page(payload.url()) || is_settings_shell(payload.url()) {
                return;
            }
            let decision = policy::top_level(&page_load.server(), payload.url());
            // Each new load may hand its page off again; only a load's own
            // Finished repeats its Started.
            if decision == Decision::Allow || payload.event() == PageLoadEvent::Started {
                *handed_off.lock().expect("handoff lock") = None;
            }
            if decision == Decision::Allow {
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
                    if let Some(window) = app.get_webview(MAIN) {
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
    let payload = serde_json::to_value(payload).unwrap_or_default();
    for (label, _) in app.webviews() {
        if crate::preferences::is_child(&label) {
            tell_window(app, &label, event, &payload);
        }
    }
}

/// Hands an event to a bundled page in the window `label`, if it's open on
/// one.
pub fn tell_window(app: &AppHandle, label: &str, event: &str, payload: impl serde::Serialize) {
    let Some(window) = app.get_webview(label) else {
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

/// This computer's settings in Chat with Work's own Settings, beside
/// everything else (the server's "This computer" tab).
pub const COMPUTER_SETTINGS: &str = "/settings?tab=computer";

/// Settings, from the tray: once the computer is paired and the Chat with
/// Work window is signed in, Chat with Work's Settings on This computer
/// there; otherwise the app-owned settings in the same window, which shares folders,
/// pairs and disconnects without signing in. Pairing approves the computer
/// in the browser, which signs nobody in to the window, so This computer
/// would only have asked to sign in again. Call it off the main thread:
/// reading the window's cookies waits for it on Windows.
pub fn show_computer_settings(app: &AppHandle) -> tauri::Result<()> {
    let desktop = app.state::<std::sync::Arc<Desktop>>().inner().clone();
    let paired = desktop.status().is_some_and(|status| status.is_paired());
    let url = desktop.server().page(COMPUTER_SETTINGS);
    let signed_in = url.as_ref().is_some_and(|url| signed_in(app, url));
    match (computer_settings(paired, signed_in), url) {
        (ComputerSettings::Web, Some(url)) => show_main(app, Some(url)),
        _ => show_settings(app, None),
    }
}

/// Where Settings in the tray goes.
#[derive(Debug, PartialEq, Eq)]
enum ComputerSettings {
    /// Chat with Work's Settings, This computer, in the Chat with Work window.
    Web,
    /// The app's own settings window.
    App,
}

/// Whether the Chat with Work window is signed in to the server: the
/// server signs the desktop in with "remember me", whose cookie outlives
/// the window. No window, or cookies it can't read, count as signed out.
fn signed_in(app: &AppHandle, url: &tauri::Url) -> bool {
    app.get_webview(MAIN)
        .and_then(|window| window.cookies_for_url(url.clone()).ok())
        .is_some_and(|cookies| signs_in(cookies.iter().map(|cookie| cookie.name())))
}

/// The server's sign-in cookie, as Devise names it for identities.
fn signs_in<'a>(mut names: impl Iterator<Item = &'a str>) -> bool {
    names.any(|name| name == "remember_identity_token")
}

fn computer_settings(paired: bool, signed_in: bool) -> ComputerSettings {
    if paired && signed_in {
        ComputerSettings::Web
    } else {
        ComputerSettings::App
    }
}

/// Shows the local settings pane, on `page` or where it was.
pub fn show_settings(app: &AppHandle, page: Option<&str>) -> tauri::Result<()> {
    crate::preferences::standalone(app, page.unwrap_or("general"))
}

/// Shows the scan window. Closing it deletes the pages not sent and stops
/// the scanner's helper.
pub fn show_scan(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(SCAN).is_some() {
        bring_forward(&app.get_window(SCAN).expect("scan window"));
        return Ok(());
    }
    let fixed = snapshots();
    let window = WebviewWindowBuilder::new(app, SCAN, WebviewUrl::App("scan.html".into()))
        .title("Scan")
        .theme(snapshot_theme())
        .inner_size(1040.0, 720.0)
        .resizable(!fixed)
        .min_inner_size(600.0, 480.0)
        .zoom_hotkeys_enabled(true)
        .on_navigation(|url| is_local_url(url) || matches!(url.scheme(), "about" | "data" | "blob"))
        .on_new_window(|url, _| {
            open_externally(&url);
            NewWindowResponse::Deny
        })
        .build()?;
    let closing = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let desktop = closing.state::<std::sync::Arc<Desktop>>().inner().clone();
            let scans = closing
                .state::<std::sync::Arc<crate::scan::Scans>>()
                .inner()
                .clone();
            std::thread::spawn(move || scans.close(&desktop.paths));
        }
    });
    bring_forward(&app.get_window(SCAN).expect("scan window"));
    Ok(())
}

#[cfg(all(any(target_os = "linux", target_os = "macos"), feature = "demo"))]
fn snapshots() -> bool {
    crate::snapshot::requested().is_some()
}

#[cfg(not(all(any(target_os = "linux", target_os = "macos"), feature = "demo")))]
fn snapshots() -> bool {
    false
}

/// The theme pictures are taken in (`CWW_APP_SNAPSHOT_THEME`), else the
/// system's.
fn snapshot_theme() -> Option<tauri::Theme> {
    if !snapshots() {
        return None;
    }
    match std::env::var("CWW_APP_SNAPSHOT_THEME").as_deref() {
        Ok("dark") => Some(tauri::Theme::Dark),
        Ok("light") => Some(tauri::Theme::Light),
        _ => None,
    }
}

/// Whether any window is open: for macOS's Dock icon, and what an update
/// relaunches with.
pub fn any_open(app: &AppHandle) -> bool {
    !app.windows().is_empty()
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
        assert!(!is_remote_label("settings"));
    }

    #[test]
    fn the_trays_settings_open_the_web_only_where_it_signs_in() {
        use super::{ComputerSettings, computer_settings};
        assert_eq!(computer_settings(true, true), ComputerSettings::Web);
        // Pairing approves the computer in the browser, which signs nobody
        // in to the app's window: the web page would ask to sign in again.
        assert_eq!(computer_settings(true, false), ComputerSettings::App);
        assert_eq!(computer_settings(false, true), ComputerSettings::App);
        assert!(super::signs_in(
            ["_chatwithwork_session", "remember_identity_token"].into_iter()
        ));
        assert!(!super::signs_in(["_chatwithwork_session"].into_iter()));
    }
}
