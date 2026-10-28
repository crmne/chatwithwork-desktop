//! Chat with Work Desktop: the Chat with Work web app in a native window,
//! the tray item for the `cww` daemon, and the background agent's settings.
//!
//! - The main window loads the real web app from the server this computer
//!   is paired with (or chatwithwork.com), the way the phone apps wrap the
//!   site, so it is always the web app as it is today. Its pages get no IPC
//!   but a small bridge (bridge.rs).
//! - The settings window shows pages bundled in the binary (ui/): pairing,
//!   shared folders, what is never shared, activity, notifications and
//!   general settings. They work offline and unpaired, and only they reach
//!   the daemon's control channel (CONTROL.md), through an allow-list
//!   (local.rs).
//! - While no window is open the app is its tray item and a thread watching
//!   the daemon; the web views and their processes go with their windows.
//!
//! The same binary is also pipe-only helpers: the notification broker, the
//! microphone recorder and local speech recognition, which the daemon and
//! `cww tui` start, and the scanner's driver, which the scan window starts.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod activity;
mod agent;
mod args;
mod audio;
mod autostart;
mod bridge;
mod computer;
mod control;
// The stand-in daemon for `--demo`: it answers the whole control protocol
// (CONTROL.md), chats included, so more of it serves the settings pages as
// they grow.
#[cfg(feature = "demo")]
#[allow(dead_code)]
mod demo;
mod desktop;
mod downloads;
mod first_launch;
mod linux;
mod local;
mod loopback;
mod model;
mod offline;
mod pairing;
mod paths;
mod policy;
mod scan;
mod server;
#[cfg(all(any(target_os = "linux", target_os = "macos"), feature = "demo"))]
mod snapshot;
mod speech;
mod state;
mod time;
mod tray;
mod user_agent;
mod windows;

use cww_notification_broker as notifications;

fn main() {
    // Crash reports, only where the person turned them on (config.toml's
    // crash_reports or CWW_CRASH_REPORTS). Starts no thread.
    if let Ok(paths) = cww::paths::Paths::from_env() {
        cww::crash_reports::install(cww::crash_reports::Component::App, &paths);
    }
    // Names the app in the system's volume mixer (PipeWire and PulseAudio),
    // with the icon the Linux packages install, before any stream opens.
    let _ = fastframe_audio::set_app_info(fastframe_audio::AppInfo {
        name: "Chat with Work".into(),
        icon: Some("cww-app".into()),
    });
    // The supervisor owns this pipe-only helper even while every window is
    // closed. macOS notification callbacks require the process's main loop.
    if std::env::args().skip(1).eq(["--notification-broker"]) {
        if let Err(error) = notifications::run_broker() {
            eprintln!("notification broker: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    // A pipe-only helper for `cww tui`'s composer. No tray, window,
    // single-instance handling, or log may write to its protocol.
    if std::env::args().skip(1).eq(["--record-audio"]) {
        std::process::exit(audio::record(false));
    }
    // The scanner's driver, in a process of its own (scan.rs): a driver
    // that crashes or hangs takes only this helper with it.
    if std::env::args().skip(1).eq(["--scan-helper"]) {
        std::process::exit(cww_scan::helper::run(cww_scan::system_backend));
    }
    if std::env::args().skip(1).eq(["--record-audio-progress"]) {
        std::process::exit(audio::record(true));
    }
    let raw: Vec<_> = std::env::args().skip(1).collect();
    if let [flag, model] = raw.as_slice()
        && flag == "--transcribe-local"
    {
        std::process::exit(speech::run(model));
    }
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("CWW_APP_LOG", "warn")).init();
    match args::parse(std::env::args().skip(1)) {
        Ok(args::Parsed::Run(options)) => {
            // Before GTK or any other thread starts.
            linux::prepare_web_view();
            // Tauri panics when GTK finds no display; say why instead.
            if let Err(message) = linux::open_display() {
                eprintln!("{message}");
                std::process::exit(1);
            }
            if let Err(e) = desktop::run(options) {
                log::error!("{e:#}");
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        Ok(args::Parsed::Printed) => {}
        Err(message) => {
            eprintln!("{message}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    }
}
