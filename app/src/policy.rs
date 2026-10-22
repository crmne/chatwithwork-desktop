//! Where the window may go. Pages of the server load in it; everything else
//! a person follows opens in their default browser.
//!
//! The web views give the navigation handler only a URL, for the page and
//! for every frame in it alike, so it can't tell a link the person
//! followed from an embedded frame (the MCP Apps sandbox, the support
//! widget, payment forms). So the handler lets other sites' frames load,
//! and the window keeps itself on the server two other ways:
//!
//! - the bridge script intercepts links and forms that leave the server and
//!   opens them in the browser before the web view sees them;
//! - a top-level page from elsewhere that loads anyway (a redirect, a script
//!   changing `location`) is sent to the browser and the window goes back
//!   ([`Decision::Leave`] from [`top_level`]).

use url::Url;

use crate::server::Server;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Load it in the web view.
    Allow,
    /// Don't load it; open it in the default browser instead.
    External(Url),
    /// Don't load it at all.
    Deny,
}

/// Schemes a web view handles itself without reaching the network.
fn inline_scheme(url: &Url) -> bool {
    matches!(url.scheme(), "about" | "data" | "blob")
}

/// Whether the system may open `url` for the person: web pages and mail
/// only, never `file:`, scripts, or other apps' URL schemes.
pub fn openable_externally(url: &Url) -> bool {
    match url.scheme() {
        "https" | "http" => url.host().is_some() && url.username().is_empty(),
        "mailto" => true,
        _ => false,
    }
}

/// The navigation handler's answer, for the page or any frame in it.
pub fn navigation(server: &Server, url: &Url) -> Decision {
    if server.owns(url) || inline_scheme(url) {
        return Decision::Allow;
    }
    match url.scheme() {
        // Another site in a frame; see the module's note.
        "https" | "http" => Decision::Allow,
        "mailto" => Decision::External(url.clone()),
        _ => Decision::Deny,
    }
}

/// What to do once the window's own page has started loading `url`.
pub fn top_level(server: &Server, url: &Url) -> Decision {
    if server.owns(url) || url.scheme() == "about" {
        Decision::Allow
    } else if openable_externally(url) {
        Decision::External(url.clone())
    } else {
        Decision::Deny
    }
}

/// `window.open` and links with `target="_blank"`: the server's own pages
/// open in another window of the app (they need its session), anything else
/// in the browser.
pub fn new_window(server: &Server, url: &Url) -> Decision {
    if server.owns(url) {
        Decision::Allow
    } else if openable_externally(url) {
        Decision::External(url.clone())
    } else {
        Decision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::parse("https://work.example").unwrap()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn the_servers_pages_and_inline_documents_load() {
        for allowed in [
            "https://work.example/chats/new",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hi",
            "blob:https://work.example/2b1e",
        ] {
            assert_eq!(navigation(&server(), &url(allowed)), Decision::Allow);
        }
    }

    #[test]
    fn other_sites_may_be_frames_so_the_handler_lets_them_load() {
        let frame = url("https://chatwithwork-apps.com/");
        assert_eq!(navigation(&server(), &frame), Decision::Allow);
        // As the window's own page, the same site goes to the browser.
        assert_eq!(top_level(&server(), &frame), Decision::External(frame));
    }

    #[test]
    fn mail_goes_to_the_mail_app_and_other_schemes_nowhere() {
        let mail = url("mailto:help@work.example");
        assert_eq!(navigation(&server(), &mail), Decision::External(mail));
        for denied in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "slack://open",
            "ms-msdt:/id",
            "tauri://localhost/index.html",
        ] {
            assert_eq!(
                navigation(&server(), &url(denied)),
                Decision::Deny,
                "{denied}"
            );
            assert_eq!(
                top_level(&server(), &url(denied)),
                Decision::Deny,
                "{denied}"
            );
            assert_eq!(
                new_window(&server(), &url(denied)),
                Decision::Deny,
                "{denied}"
            );
        }
    }

    #[test]
    fn new_windows_stay_in_the_app_only_for_the_server() {
        assert_eq!(
            new_window(
                &server(),
                &url("https://work.example/rails/active_storage/x")
            ),
            Decision::Allow
        );
        let docs = url("https://docs.example/page");
        assert_eq!(new_window(&server(), &docs), Decision::External(docs));
    }

    #[test]
    fn nothing_with_credentials_or_without_a_host_is_handed_to_the_system() {
        assert!(!openable_externally(&url("https://user:pw@evil.example/")));
        assert!(!openable_externally(&url("file:///tmp/x")));
        assert!(openable_externally(&url("https://evil.example/?a=1&b=2")));
    }
}
