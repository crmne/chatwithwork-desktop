//! WebKitGTK workarounds, applied by the app itself only where they're
//! needed, and the check that a display is there before Tauri needs one.
//!
//! On NVIDIA's proprietary driver under Wayland, WebKitGTK's GPU process
//! dies at start with a Wayland protocol error (71) when the driver uses
//! explicit sync, as it does on Hyprland, and the window never draws.
//! `__NV_DISABLE_EXPLICIT_SYNC=1` turns explicit sync off for this process
//! only; everyone else renders as before. A value already in the
//! environment always wins.
//!
//! WebKitGTK 2.54's new Skia compositor shows one frame of an animation's
//! start value as the animation ends: a fade ends on a frame of its first
//! state, a slide on a frame of where it began, so a hovered icon blinks
//! and the sidebar drops out for a frame as it settles. Measured on every
//! composited frame of a headless WebKitGTK 2.54.1 (gamescope's output):
//! 24 of 30 slides and 16 of 20 fades did it with Skia, none with the
//! older compositor, at the same 60 fps (WebKit bugs 280804 and 317744 use
//! the same switch). `WEBKIT_USE_SKIA_FOR_COMPOSITION=0` keeps GPU
//! rendering and composites with the older one. Older WebKitGTK ignores
//! the variable, so it's set everywhere, and `=1` turns Skia back on.
//! Re-test on each new WebKitGTK minor (docs/desktop-app.md) and drop it
//! once Skia's frames are clean.

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
        if needs_older_compositor(std::env::var_os(SKIA_COMPOSITION).is_some()) {
            // SAFETY: as above.
            unsafe { std::env::set_var(SKIA_COMPOSITION, "0") };
        }
    }
}

/// Names the app for GLib: the tray item's title (the StatusNotifierItem's
/// `Title`, which panels show on hover, as libappindicator has no tooltip)
/// is GLib's application name, which is the binary's, "cww-app", until set.
/// Given the build's product name (tauri.conf.json's `productName`). Set
/// once; later calls leave it.
pub fn name_application(name: &str) {
    #[cfg(target_os = "linux")]
    if gtk::glib::application_name().is_none_or(|current| current != name) {
        static NAMED: std::sync::Once = std::sync::Once::new();
        NAMED.call_once(|| gtk::glib::set_application_name(name));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = name;
}

/// Opens GTK's display before Tauri does. Tauri's event loop (tao) panics
/// when GTK can't start, which happens whenever the app is started outside
/// a graphical session: from SSH, a service started before the desktop
/// exported its display, or a session whose compositor has gone. That
/// panic is a crash report with nothing to fix in it; this says why in a
/// sentence and exits instead, and the daemon opens a notification's chat
/// in the browser when the app exits with an error. On macOS and Windows
/// there is always a display, so this is Linux only.
pub fn open_display() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // GTK remembers it's initialized: tao's own gtk::init() then
        // returns at once on this same (main) thread.
        if gtk::init().is_err() {
            return Err(no_display(
                std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
                std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()),
            )
            .into());
        }
    }
    Ok(())
}

/// Why no window could open, from what the environment names.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn no_display(wayland: bool, x11: bool) -> &'static str {
    if wayland || x11 {
        "Chat with Work couldn't open a window: it couldn't connect to the \
         display this session names (WAYLAND_DISPLAY or DISPLAY)."
    } else {
        "Chat with Work needs a desktop to open its windows, and this session \
         has none (neither WAYLAND_DISPLAY nor DISPLAY is set). Start Chat with \
         Work from your desktop, or use `cww` in a terminal."
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPLICIT_SYNC: &str = "__NV_DISABLE_EXPLICIT_SYNC";

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SKIA_COMPOSITION: &str = "WEBKIT_USE_SKIA_FOR_COMPOSITION";

/// Always, unless a choice was already made.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn needs_older_compositor(already_set: bool) -> bool {
    !already_set
}

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

    #[test]
    fn the_older_compositor_is_chosen_unless_a_choice_was_made() {
        assert!(needs_older_compositor(false));
        assert!(!needs_older_compositor(true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_tray_is_named_once_by_the_product_name() {
        name_application("Chat with Work");
        name_application("cww-app");
        assert_eq!(
            gtk::glib::application_name().as_deref(),
            Some("Chat with Work")
        );
    }

    #[test]
    fn no_display_says_whether_one_was_named() {
        assert!(no_display(false, false).contains("WAYLAND_DISPLAY"));
        assert!(no_display(true, false).contains("couldn't connect"));
        assert!(no_display(false, true).contains("couldn't connect"));
    }
}
