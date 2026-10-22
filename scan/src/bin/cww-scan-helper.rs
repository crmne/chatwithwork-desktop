//! The scanner's helper as a program of its own: the JSON-lines protocol in
//! helper.rs on stdin and stdout, over this system's backend. The desktop
//! app starts it on Windows, built for 32-bit x86 so 32-bit TWAIN drivers
//! load into it; elsewhere the app runs the same loop as
//! `cww-app --scan-helper`.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    std::process::exit(cww_scan::helper::run(cww_scan::system_backend));
}
