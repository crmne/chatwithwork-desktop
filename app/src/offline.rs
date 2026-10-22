//! The offline page: when the server can't be reached, the Chat with Work
//! window shows a page bundled in the app (`ui/offline.html`) instead of the
//! web view's own error page, with Retry and a way to the settings. It
//! retries by itself too, and goes back where the window was headed once the
//! server answers.
//!
//! Before the window opens or moves to the server, the app checks the server
//! is reachable (a TCP connection to its host and port, with a short
//! timeout). On Linux, a page of the server that fails to load later (the
//! network went away) is checked the same way and replaced by the offline
//! page; WKWebView and WebView2 don't tell Tauri about failed loads.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, State, Url, Webview};

use crate::desktop::Desktop;
use crate::server::Server;

/// How long a check waits for the server.
const TIMEOUT: Duration = Duration::from_secs(4);

/// The page, relative to the bundled pages.
pub const PAGE: &str = "offline.html";

/// Whether the window should try the server: it answers a TCP connection
/// on its port, or a proxy stands between (the check can't see through one,
/// and the web view uses it). The check is advisory: the offline page also
/// offers to open Chat with Work anyway, for a system proxy this misses.
pub fn reachable(server: &Server, paths: &cww::paths::Paths) -> bool {
    if behind_proxy(server, paths) {
        return true;
    }
    answers(server, TIMEOUT)
}

/// A proxy from config.toml or the environment, as the daemon reads them.
fn behind_proxy(server: &Server, paths: &cww::paths::Paths) -> bool {
    let setting = cww::config::Config::load(paths)
        .ok()
        .and_then(|config| config.proxy);
    cww::auth::client::ServerUrl::parse(&server.origin())
        .ok()
        .and_then(|origin| {
            cww::proxy::for_server(setting.as_deref(), &origin)
                .ok()
                .flatten()
        })
        .is_some()
}

/// Whether the server answers within `budget`, name lookup included, across
/// all its addresses.
pub fn answers(server: &Server, budget: Duration) -> bool {
    let Ok(url) = Url::parse(&server.origin()) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let started = std::time::Instant::now();
    // The lookup has no timeout of its own, so it runs on a thread the
    // check stops waiting for.
    let spawned = std::thread::Builder::new()
        .name("cww-reachable".into())
        .spawn(move || {
            let Ok(addresses) = (host.as_str(), port).to_socket_addrs() else {
                let _ = tx.send(false);
                return;
            };
            for address in addresses {
                let left = budget.saturating_sub(started.elapsed());
                if left.is_zero() {
                    break;
                }
                if TcpStream::connect_timeout(&address, left).is_ok() {
                    let _ = tx.send(true);
                    return;
                }
            }
            let _ = tx.send(false);
        });
    spawned.is_ok() && rx.recv_timeout(budget).unwrap_or(false)
}

/// Whether `url` is the bundled offline page.
pub fn is_page(url: &Url) -> bool {
    crate::windows::is_local_url(url) && url.path() == format!("/{PAGE}")
}

/// The caller must be the offline page, in the Chat with Work window.
fn offline_page(webview: &Webview) -> Result<(), Value> {
    let ok =
        webview.label() == crate::windows::MAIN && webview.url().is_ok_and(|url| is_page(&url));
    if ok {
        Ok(())
    } else {
        Err(json!({ "code": "forbidden", "message": "Only the offline page can ask for this." }))
    }
}

/// What the page shows: the server's host.
#[tauri::command]
pub fn offline_info(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Result<Value, Value> {
    offline_page(&webview)?;
    let origin = desktop.server().origin();
    let host = Url::parse(&origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or(origin);
    Ok(json!({ "host": host }))
}

/// Checks the server again, and goes where the window was headed once it
/// answers. Answers whether it did.
#[tauri::command]
pub async fn offline_retry(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<Value, Value> {
    offline_page(&webview)?;
    let (server, paths) = (desktop.server(), desktop.paths.daemon.clone());
    let online = tauri::async_runtime::spawn_blocking(move || reachable(&server, &paths))
        .await
        .unwrap_or(false);
    log::info!(
        "offline page: the server is {}",
        if online { "back" } else { "still unreachable" }
    );
    if online {
        let target = desktop
            .take_wanted()
            .unwrap_or_else(|| desktop.server().start_url());
        if let Err(e) = crate::windows::navigate_main(&app, target) {
            return Err(json!({ "code": "failed", "message": e.to_string() }));
        }
    }
    Ok(json!({ "online": online }))
}

/// Goes where the window was headed without checking: the check can miss a
/// system proxy the web view uses.
#[tauri::command]
pub async fn offline_open_anyway(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
) -> Result<Value, Value> {
    offline_page(&webview)?;
    let target = desktop
        .take_wanted()
        .unwrap_or_else(|| desktop.server().start_url());
    webview
        .navigate(target)
        .map(|()| json!({}))
        .map_err(|e| json!({ "code": "failed", "message": e.to_string() }))
}

/// Opens the settings window from the offline page.
#[tauri::command]
pub async fn offline_settings(webview: Webview, app: AppHandle) -> Result<Value, Value> {
    offline_page(&webview)?;
    crate::windows::show_settings(&app, None)
        .map(|()| json!({}))
        .map_err(|e| json!({ "code": "failed", "message": e.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bundled_page_is_the_offline_page() {
        for page in [
            "tauri://localhost/offline.html",
            "http://tauri.localhost/offline.html?x=1",
        ] {
            assert!(is_page(&Url::parse(page).unwrap()), "{page}");
        }
        for other in [
            "tauri://localhost/index.html",
            "https://chatwithwork.com/offline.html",
            "tauri://localhost/offline.html/../index.html",
        ] {
            assert!(!is_page(&Url::parse(other).unwrap()), "{other}");
        }
    }

    #[test]
    fn a_listening_port_is_reachable_and_a_closed_one_isnt() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = Server::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(answers(&open, TIMEOUT));
        drop(listener);
        let closed = Server::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(!answers(&closed, TIMEOUT));
    }
}
