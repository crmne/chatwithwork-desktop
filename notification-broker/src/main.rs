//! A headless helper for CLI-only Linux packages, with the same private pipe
//! protocol as cww-app. macOS uses cww-app inside the signed application bundle.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    if std::env::args_os().skip(1).eq(["--version"]) {
        println!("cww-notify {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if !std::env::args_os().skip(1).eq(["--notification-broker"]) {
        eprintln!("This helper is managed by Chat with Work Desktop.");
        std::process::exit(2);
    }
    if let Err(error) = cww_notification_broker::run_broker() {
        eprintln!("notification broker: {error:#}");
        std::process::exit(1);
    }
}
