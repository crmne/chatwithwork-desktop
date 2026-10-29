//! The command line. Notifications, the applications menu and the login
//! item start the app with these, and a second launch hands them to the one
//! running.

use cww::notifications::navigation::Target;

pub const USAGE: &str = "\
Chat with Work

Usage: cww-app [options]

Options:
  --background         Start in the tray without opening a window (how the
                       app starts at login)
  --settings           Open the settings window instead of Chat with Work
  --scan               Open the scan window
  --page <name>        Open the settings window on a page: welcome, folders,
                       privacy, activity, account, notifications, general
  --open-chat <json>   Open an account-scoped chat from a native notification
  --server <url>       Show this Chat with Work server instead of the paired
                       one (for development)
  --version            Print the version
  --help               Print this help

The cww command runs the agent itself; see `cww --help`.";

/// The settings pages, as the bundled pages name them.
pub const PAGES: [&str; 7] = [
    "welcome",
    "folders",
    "privacy",
    "activity",
    "account",
    "notifications",
    "general",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// The web app, on its start page.
    Main,
    /// A chat a notification pointed at.
    Chat(Target),
    /// The settings window, on a page or the one it opens on.
    Settings(Option<&'static str>),
    /// The scan window.
    Scan,
    /// Nothing: the tray only.
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub open: Open,
    /// Started at login: a running app is left alone.
    pub background: bool,
    pub server: Option<String>,
    /// `--demo` (a paired sample) or `--demo-fresh` (a first run).
    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    pub demo: Option<bool>,
}

pub enum Parsed {
    Run(Options),
    /// `--version` or `--help`, already printed.
    Printed,
}

pub fn page(name: &str) -> Option<&'static str> {
    PAGES.iter().copied().find(|page| *page == name)
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut options = Options {
        open: Open::Main,
        background: false,
        server: None,
        demo: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open-chat" => {
                let value = args.next().ok_or("--open-chat needs a destination")?;
                options.open =
                    Open::Chat(Target::parse(&value).map_err(|_| "invalid chat destination")?);
            }
            "--background" => {
                options.background = true;
                options.open = Open::Nothing;
            }
            "--settings" => options.open = Open::Settings(None),
            "--scan" => options.open = Open::Scan,
            "--page" => {
                let name = args.next().ok_or("--page needs a name")?;
                let page = page(&name).ok_or_else(|| format!("unknown page {name:?}"))?;
                options.open = Open::Settings(Some(page));
            }
            "--server" => {
                let url = args.next().ok_or("--server needs a URL")?;
                crate::server::Server::parse(&url).map_err(|e| format!("{e:#}"))?;
                options.server = Some(url);
            }
            "--demo" if cfg!(feature = "demo") => options.demo = Some(false),
            "--demo-fresh" if cfg!(feature = "demo") => options.demo = Some(true),
            "--version" | "-V" => {
                println!("{}", crate::update::version_line());
                return Ok(Parsed::Printed);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(Parsed::Printed);
            }
            // macOS passes this when an app is opened from Finder.
            other if other.starts_with("-psn_") => {}
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Parsed::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Options, String> {
        match parse(list.iter().map(|s| s.to_string()))? {
            Parsed::Run(options) => Ok(options),
            Parsed::Printed => Err("printed".into()),
        }
    }

    #[test]
    fn parses_options() {
        assert_eq!(args(&[]).unwrap().open, Open::Main);
        let login = args(&["--background"]).unwrap();
        assert!(login.background);
        assert_eq!(login.open, Open::Nothing);
        assert_eq!(args(&["--settings"]).unwrap().open, Open::Settings(None));
        assert_eq!(args(&["--scan"]).unwrap().open, Open::Scan);
        assert_eq!(
            args(&["--page", "activity"]).unwrap().open,
            Open::Settings(Some("activity"))
        );
        assert!(args(&["--page", "chat"]).is_err());
        assert!(args(&["--frobnicate"]).is_err());
        assert!(args(&["-psn_0_12345"]).is_ok());
        assert!(args(&["--version"]).is_err());
        assert_eq!(
            args(&["--server", "http://localhost:3000"])
                .unwrap()
                .server
                .as_deref(),
            Some("http://localhost:3000")
        );
        assert!(args(&["--server", "http://evil.example"]).is_err());
    }

    #[test]
    fn notification_launches_require_a_validated_destination() {
        let options = args(&[
            "--open-chat",
            r#"{"server":"https://example.test","account":"482139075","chat":16}"#,
        ])
        .unwrap();
        assert!(matches!(options.open, Open::Chat(target) if target.chat() == 16));
        assert!(args(&["--open-chat", "https://example.test/482139075/chats/16"]).is_err());
        assert!(args(&["--open-chat"]).is_err());
    }
}
