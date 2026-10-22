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

/// Whether the server answers a TCP connection on its port.
pub fn reachable(server: &Server) -> bool {
    let Ok(url) = Url::parse(&server.origin()) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, TIMEOUT).is_ok())
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
    let server = desktop.server();
    let online = tauri::async_runtime::spawn_blocking(move || reachable(&server))
        .await
        .unwrap_or(false);
    log::info!("offline page: the server is {}", if online { "back" } else { "still unreachable" });
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
        assert!(reachable(&open));
        drop(listener);
        let closed = Server::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(!reachable(&closed));
    }
}
