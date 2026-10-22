//! WebKitGTK workarounds, applied by the app itself only where they're
//! needed.
//!
//! On NVIDIA's proprietary driver under Wayland, WebKitGTK's GPU process
//! dies at start with a Wayland protocol error (71) when the driver uses
//! explicit sync, as it does on Hyprland, and the window never draws.
//! `__NV_DISABLE_EXPLICIT_SYNC=1` turns explicit sync off for this process
//! only; everyone else renders as before. A value already in the
//! environment always wins.

/// Sets the workaround for this process before any thread or GTK starts.
pub fn prepare_web_view() {
    #[cfg(target_os = "linux")]
    {
        let needed = needs_explicit_sync_workaround(
            std::env::var_os(EXPLICIT_SYNC).is_some(),
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
            std::path::Path::new("/proc/driver/nvidia/version").exists(),
        );
        if needed {
            // SAFETY: called first thing in main, before any other thread
            // exists, so nothing reads the environment concurrently.
            unsafe { std::env::set_var(EXPLICIT_SYNC, "1") };
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPLICIT_SYNC: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// Only on Wayland with NVIDIA's driver loaded, and never over a choice
/// already made.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn needs_explicit_sync_workaround(already_set: bool, wayland: bool, nvidia: bool) -> bool {
    !already_set && wayland && nvidia
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nvidia_on_wayland_without_a_choice_gets_it() {
        assert!(needs_explicit_sync_workaround(false, true, true));
        assert!(!needs_explicit_sync_workaround(true, true, true));
        assert!(!needs_explicit_sync_workaround(false, false, true));
        assert!(!needs_explicit_sync_workaround(false, true, false));
    }
}
