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
    "settings_present",
    "settings_hide",
    "listen",
    "computer",
    "computer_change",
    "computer_add_folder",
    "computer_notifications",
    "scan",
    "scan_file",
    "scan_pending",
    "page_dialog",
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
    "prepare_command_drives",
    "browser_enable",
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
    "update_status",
    "update_check",
    "update_restart",
    "set_update_checks",
    // The scan window (scan.rs).
    "scan_info",
    "scan_devices",
    "scan_capabilities",
    "scan_save_settings",
    "scan_start",
    "scan_cancel",
    "scan_page",
    "scan_edit",
    "scan_arrange",
    "scan_discard",
    "scan_destinations",
    "scan_send",
    "scan_attach",
    "scan_open",
    "scan_save",
    "scan_reveal",
    "scan_open_account",
    "scan_add_language",
    "scan_language_packs",
    "scan_download_language",
    "scan_cancel_language",
    // The offline page in the Chat with Work window (offline.rs).
    "offline_info",
    "offline_retry",
    "offline_open_anyway",
    "offline_settings",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("Tauri's build step");
}
