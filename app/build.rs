//! Tauri's build step: embeds tauri.conf.json, the bundled pages (ui/) and
//! the capabilities, and makes a permission for each command, so a
//! capability can name exactly the commands a window's pages may call
//! (capabilities/local.json; the web app's is added at run time, scoped to
//! the paired server). On Windows it also embeds the executable's icon and
//! version information.

const COMMANDS: &[&str] = &[
    // The bridge, for the web app's pages (bridge.rs).
    "open_external",
    "open_preferences",
    "listen",
    // The bundled settings pages (local.rs).
    "control",
    "activity",
    "app_info",
    "set_onboarded",
    "pick_folder",
    "pair",
    "cancel_pairing",
    "logout",
    "start_agent",
    "autostart",
    "set_autostart",
    "crash_reports",
    "set_crash_reports",
    "open_file",
    "open_url",
    "show_main",
    "quit",
    "dictation",
    "set_dictation",
    "download_model",
    "cancel_download",
    "remove_model",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("Tauri's build step");
}
