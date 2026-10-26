//! Which Chat with Work the window shows, and what counts as staying on it.
//!
//! The window loads the server this computer is paired with (config.toml's
//! `server.url`, which `cww login` writes), so staging and self-hosted
//! installs open their own site. Before pairing it opens chatwithwork.com.
//! `--server` overrides both, for development.

use cww::auth::client::ServerUrl;
use cww::notifications::navigation::Target;
use url::Url;

/// The server the window belongs to: its origin is the only one whose pages
/// load in the window and may use the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    origin: Url,
}

impl Server {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let server = ServerUrl::parse(input)?;
        Ok(Self {
            origin: Url::parse(&server.origin())?,
        })
    }

    /// `--server`, else the paired server, else chatwithwork.com.
    pub fn resolve(flag: Option<&str>, paths: Option<&cww::paths::Paths>) -> anyhow::Result<Self> {
        if let Some(flag) = flag {
            return Self::parse(flag);
        }
        let paired = paths
            .and_then(|paths| cww::config::Config::load(paths).ok())
            .and_then(|config| config.server)
            .map(|server| server.url);
        match paired {
            Some(url) => Self::parse(&url),
            None => Self::parse(cww::config::DEFAULT_SERVER),
        }
    }

    /// `https://host[:port]`, without a trailing slash.
    pub fn origin(&self) -> String {
        self.origin.origin().ascii_serialization()
    }

    /// `host[:port]`: how the server is named where a page speaks for
    /// itself, as in its dialogs' titles.
    pub fn host(&self) -> String {
        let host = self.origin.host_str().unwrap_or_default();
        match self.origin.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }
    }

    /// What the window opens on: like the installed web app, `/chats/new`,
    /// which leads into the organization used last.
    pub fn start_url(&self) -> Url {
        self.origin.join("/chats/new").expect("a fixed path joins")
    }

    /// A chat a notification points at, on this server only.
    pub fn chat_url(&self, target: &Target) -> Option<Url> {
        if target.server().origin() != self.origin() {
            return None;
        }
        self.origin
            .join(&format!("/{}/chats/{}", target.account(), target.chat()))
            .ok()
    }

    /// One of this server's pages, from a path like `/settings?tab=x`.
    /// Nothing that could lead elsewhere: `//host`, backslashes, schemes.
    pub fn page(&self, path: &str) -> Option<Url> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '\r', '\n']) {
            return None;
        }
        let url = self.origin.join(path).ok()?;
        self.owns(&url).then_some(url)
    }

    /// Whether `url` is a page of this server.
    pub fn owns(&self, url: &Url) -> bool {
        url.origin() == self.origin.origin()
    }

    /// The pattern Tauri's capabilities match remote pages against.
    pub fn capability_pattern(&self) -> String {
        format!("{}/*", self.origin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::parse("https://work.example").unwrap()
    }

    #[test]
    fn opens_on_new_chat_like_the_installed_web_app() {
        assert_eq!(
            server().start_url().as_str(),
            "https://work.example/chats/new"
        );
        let local = Server::parse("http://127.0.0.1:3210").unwrap();
        assert_eq!(local.origin(), "http://127.0.0.1:3210");
        assert_eq!(local.capability_pattern(), "http://127.0.0.1:3210/*");
        assert_eq!(local.host(), "127.0.0.1:3210");
        assert_eq!(server().host(), "work.example");
    }

    #[test]
    fn pages_stay_on_the_server() {
        assert_eq!(
            server()
                .page("/settings?tab=notifications")
                .unwrap()
                .as_str(),
            "https://work.example/settings?tab=notifications"
        );
        for elsewhere in [
            "//evil.example/x",
            "https://evil.example/",
            "settings",
            "/\\evil.example",
            "/a\nb",
        ] {
            assert!(server().page(elsewhere).is_none(), "{elsewhere}");
        }
    }

    #[test]
    fn plain_http_is_for_localhost_only() {
        assert!(Server::parse("http://work.example").is_err());
        assert!(Server::parse("https://work.example/path").is_err());
        assert!(Server::parse("ftp://work.example").is_err());
    }

    #[test]
    fn only_its_own_origin_counts_as_the_server() {
        let server = server();
        for own in [
            "https://work.example/",
            "https://work.example/482139075/chats/3?x=1#y",
            "https://work.example:443/a",
        ] {
            assert!(server.owns(&Url::parse(own).unwrap()), "{own}");
        }
        for other in [
            "http://work.example/",
            "https://work.example.evil/",
            "https://evil.example/https://work.example/",
            "https://sub.work.example/",
            "https://work.example:8443/",
            "about:blank",
        ] {
            assert!(!server.owns(&Url::parse(other).unwrap()), "{other}");
        }
    }

    #[test]
    fn a_notification_opens_its_chat_only_on_this_server() {
        let here =
            Target::parse(r#"{"server":"https://work.example","account":"482139075","chat":16}"#)
                .unwrap();
        assert_eq!(
            server().chat_url(&here).unwrap().as_str(),
            "https://work.example/482139075/chats/16"
        );
        let elsewhere =
            Target::parse(r#"{"server":"https://other.example","account":"482139075","chat":16}"#)
                .unwrap();
        assert!(server().chat_url(&elsewhere).is_none());
    }

    #[test]
    fn the_paired_server_wins_over_the_default_and_the_flag_over_both() {
        let dir = std::env::temp_dir().join(format!("cww-desktop-test-{}", std::process::id()));
        let paths = cww::paths::Paths::under(&dir);
        assert_eq!(
            Server::resolve(None, Some(&paths)).unwrap().origin(),
            "https://chatwithwork.com"
        );
        std::fs::create_dir_all(paths.config_file().parent().unwrap()).unwrap();
        std::fs::write(
            paths.config_file(),
            "[server]\nurl = \"https://staging.chatwithwork.com\"\ndevice_id = \"42\"\n",
        )
        .unwrap();
        assert_eq!(
            Server::resolve(None, Some(&paths)).unwrap().origin(),
            "https://staging.chatwithwork.com"
        );
        assert_eq!(
            Server::resolve(Some("http://localhost:3000"), Some(&paths))
                .unwrap()
                .origin(),
            "http://localhost:3000"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
