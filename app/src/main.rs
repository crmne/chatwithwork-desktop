//! Chat with Work: the menu bar app and settings window for the
//! `cww` daemon. It shows the daemon's state, pauses and resumes it, and
//! manages shared folders and pairing, all over the daemon's control socket
//! (CONTROL.md).
//!
//! While the window is closed the app is a tray item and three threads
//! blocked on I/O (the tray, the daemon's status, other launches), so it
//! uses no CPU until something happens.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod agent;
mod audio;
mod autostart;
mod clipboard;
mod control;
#[cfg(any(test, feature = "demo"))]
mod demo;
mod events;
mod icons;
mod instance;
mod model;
mod pairing;
mod paths;
mod platform;
mod screenshot;
mod shell;
mod speech;
mod time;
mod tray;
mod ui;

use std::path::PathBuf;

use cww_notification_broker as notifications;
use ui::Page;

const USAGE: &str = "\
Chat with Work

Usage: cww-app [options]

Options:
  --background         Start in the tray without opening the settings window
                       (how the app starts at login)
  --page <name>        Open the window on a page: chat, folders, privacy,
                       activity, account, general, welcome
  --screenshot <file>  Save a PNG of the settings window and quit
  --open-chat <json>   Open an account-scoped chat from a native notification
  --version            Print the version
  --help               Print this help

The cww command runs the agent itself; see `cww --help`.";

fn main() {
    // Crash reports, only where the person turned them on (config.toml's
    // crash_reports or CWW_CRASH_REPORTS). Starts no thread.
    if let Ok(paths) = cww::paths::Paths::from_env() {
        cww::crash_reports::install(cww::crash_reports::Component::App, &paths);
    }
    // The supervisor owns this pipe-only helper even while every window is
    // closed. macOS notification callbacks require the process's main loop.
    if std::env::args().skip(1).eq(["--notification-broker"]) {
        if let Err(error) = notifications::run_broker() {
            eprintln!("notification broker: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    // A pipe-only helper for the desktop and terminal composers. No tray,
    // window, single-instance handling, or log may write to its protocol.
    if std::env::args().skip(1).eq(["--record-audio"]) {
        std::process::exit(audio::record(false));
    }
    if std::env::args().skip(1).eq(["--record-audio-progress"]) {
        std::process::exit(audio::record(true));
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [flag, model] = args.as_slice()
        && flag == "--transcribe-local"
    {
        std::process::exit(speech::run(model));
    }
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("CWW_APP_LOG", "warn")).init();
    match parse(std::env::args().skip(1)) {
        Ok(Some(options)) => {
            if let Err(e) = run(options) {
                log::error!("{e:#}");
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        Ok(None) => {}
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

struct Args {
    shell: shell::Options,
    /// `--demo` (a paired sample) or `--demo-fresh` (a first run).
    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    demo: Option<bool>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut options = shell::Options {
        open_chat: None,
        open_settings: true,
        background: false,
        page: None,
        screenshot: None,
        account: None,
        instance: None,
    };
    let mut demo = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open-chat" => {
                let value = args.next().ok_or("--open-chat needs a destination")?;
                options.open_chat = Some(
                    cww::notifications::navigation::Target::parse(&value)
                        .map_err(|_| "invalid chat destination")?,
                );
            }
            "--background" => {
                options.background = true;
                options.open_settings = false;
            }
            "--page" => {
                let name = args.next().ok_or("--page needs a name")?;
                options.page = Some(page(&name).ok_or_else(|| format!("unknown page {name:?}"))?);
            }
            "--screenshot" => {
                options.screenshot = Some(PathBuf::from(
                    args.next().ok_or("--screenshot needs a file")?,
                ));
            }
            "--demo" if cfg!(feature = "demo") => demo = Some(false),
            "--demo-fresh" if cfg!(feature = "demo") => demo = Some(true),
            "--version" | "-V" => {
                println!("cww-app {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            // macOS passes this when an app is opened from Finder.
            other if other.starts_with("-psn_") => {}
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Some(Args {
        shell: options,
        demo,
    }))
}

fn page(name: &str) -> Option<Page> {
    Some(match name {
        "welcome" => Page::Welcome,
        "chat" => Page::Chat,
        "folders" => Page::Folders,
        "privacy" => Page::Privacy,
        "activity" => Page::Activity,
        "account" => Page::Account,
        "general" => Page::General,
        "notifications" => Page::Notifications,
        _ => return None,
    })
}

fn run(args: Args) -> anyhow::Result<()> {
    #[cfg(feature = "demo")]
    if let Some(fresh) = args.demo {
        let scenario = if fresh {
            demo::Scenario::Fresh
        } else {
            demo::Scenario::Sample
        };
        let server = demo::start(scenario)?;
        if scenario == demo::Scenario::Sample {
            server.script_live_events();
        }
        let mut options = args.shell;
        options.account = Some(server.account());
        if options.screenshot.is_none() {
            options.instance = Some(instance::demo_slot());
        }
        return shell::run(server.paths.clone(), options);
    }
    let paths = paths::Paths::from_env()?;
    let mut options = args.shell;
    if options.screenshot.is_none() {
        options.instance = Some(instance::slot());
    }
    shell::run(paths, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Option<Args>, String> {
        parse(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_options() {
        let plain = args(&[]).unwrap().unwrap();
        assert!(plain.shell.open_settings && !plain.shell.background);
        let login = args(&["--background"]).unwrap().unwrap();
        assert!(!login.shell.open_settings && login.shell.background);
        let page = args(&["--page", "activity"]).unwrap().unwrap();
        assert_eq!(page.shell.page, Some(Page::Activity));
        assert!(args(&["--page", "nope"]).is_err());
        assert!(args(&["--frobnicate"]).is_err());
        assert!(args(&["-psn_0_12345"]).unwrap().is_some());
        assert!(args(&["--version"]).unwrap().is_none());
    }

    #[test]
    fn notification_launches_require_a_validated_destination() {
        let options = args(&[
            "--open-chat",
            r#"{"server":"https://example.test","account":"482139075","chat":16}"#,
        ])
        .unwrap()
        .unwrap();
        assert_eq!(options.shell.open_chat.unwrap().chat(), 16);
        assert!(args(&["--open-chat", "https://example.test/482139075/chats/16"]).is_err());
        assert!(args(&["--open-chat"]).is_err());
    }
}
