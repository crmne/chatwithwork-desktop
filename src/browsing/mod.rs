//! Chat with Work in the person's own browser, through the Chat with Work
//! extension (docs/browser.md): the native messaging host the browser
//! starts, the daemon's bridge to it, the tools it offers, and the rules
//! every call follows here as well as in the extension.

pub mod bridge;
pub mod host;
pub mod install;
pub mod local;
pub mod settings;
pub mod sites;
pub mod tools;

/// Run as the native messaging host when the browser started this process
/// as one. Returns only when it wasn't.
pub fn host_if_invoked() {
    let args: Vec<_> = std::env::args_os().collect();
    let Some(origin) = host::invoked_as_host(&args) else {
        return;
    };
    let code = match crate::paths::Paths::from_env().and_then(|paths| host::run(&paths, &origin)) {
        Ok(()) => 0,
        Err(_) => 1,
    };
    std::process::exit(code);
}
