//! What the bundled settings pages may ask of the app. Only pages served
//! from the app itself (`tauri://localhost`, `http://tauri.localhost` on
//! Windows) in an app-owned child webview get these: the capability in
//! capabilities/local.json names that webview and no remote URL, and every
//! command checks its caller's page again. The web app's pages never reach
//! the daemon.
//!
//! The daemon's control channel is behind an allow-list ([`ALLOWED`]): what
//! the settings need, never chats, files' contents or `shutdown`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cww::control::ControlRequest;
use serde_json::{Value, json};
use tauri::{AppHandle, State, Webview};

use crate::desktop::Desktop;

/// Control requests the settings pages may send (CONTROL.md).
pub const ALLOWED: [&str; 27] = [
    "hello",
    "status",
    "pause",
    "resume",
    "roots_add",
    "roots_remove",
    "roots_writable",
    "roots_commands",
    "roots_label",
    "roots_bot",
    "suggested_roots",
    "deny",
    "deny_extra",
    "audit_tail",
    "notification_status",
    "notification_permission",
    "notification_preferences",
    "notification_preferences_update",
    "notification_browser_link",
    // The browser (docs/browser.md). Turning it on registers the host first,
    // which only `cww` can do, so that goes through `browser_enable`.
    "browser",
    "browser_site",
    "browser_all_sites",
    "browser_ask_before_actions",
    "browser_blocked_site",
    "browser_changes",
    "browser_stopped",
    "browser_bot",
];

type Answer<T = Value> = Result<T, Value>;

fn refusal(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

/// The caller must be a bundled page in an app-owned child webview.
fn local_page(webview: &Webview) -> Result<(), Value> {
    let local = crate::preferences::is_child(webview.label())
        && webview
            .url()
            .is_ok_and(|url| crate::preferences::is_document(&url));
    if local {
        Ok(())
    } else {
        Err(refusal(
            "forbidden",
            "Only the app's own settings can ask for this.",
        ))
    }
}

/// Parses a request a page sent and checks it's one it may send.
pub fn allowed(request: Value) -> Result<ControlRequest, Value> {
    let cmd = request["cmd"].as_str().unwrap_or_default().to_string();
    if !ALLOWED.contains(&cmd.as_str()) {
        return Err(refusal(
            "forbidden",
            format!("The settings can't send {cmd:?}."),
        ));
    }
    serde_json::from_value(request).map_err(|e| refusal("invalid", format!("Invalid request: {e}")))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Value> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| refusal("failed", e.to_string()))
}

#[tauri::command]
pub async fn control(webview: Webview, desktop: State<'_, Arc<Desktop>>, request: Value) -> Answer {
    local_page(&webview)?;
    let request = allowed(request)?;
    let desktop = desktop.inner().clone();
    blocking(move || {
        let app = desktop.app();
        let context = crate::preferences::grant_context(&app, &desktop, &webview, &request)
            .map_err(|e| refusal("forbidden", e))?;
        let status = desktop
            .client
            .status()
            .map_err(|e| refusal("not_running", e.to_string()))?;
        if let Some(question) = crate::preferences::question(&request, &status, &context)
            .map_err(|e| refusal("refused", e))?
        {
            let Some(_asking) = desktop.asking.start() else {
                return Err(refusal("busy", "Answer the app’s current question first."));
            };
            if !crate::bridge::confirm(&app, webview.window().label(), &question) {
                return Err(refusal("declined", "No changes made."));
            }
            // A dialog can outlive navigation or bot removal.
            crate::preferences::grant_context(&app, &desktop, &webview, &request)
                .map_err(|e| refusal("forbidden", e))?;
        }
        desktop.client.request(&request).map_err(|e| match e {
            crate::control::ControlError::NotRunning => refusal("not_running", e.to_string()),
            other => refusal("refused", other.to_string()),
        })
    })
    .await?
}

/// The activity log, newest first, as sentences (activity.rs).
#[tauri::command]
pub async fn activity(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    lines: Option<usize>,
) -> Answer {
    local_page(&webview)?;
    let client = desktop.client.clone();
    let lines = lines.unwrap_or(300).clamp(1, 2000);
    let entries = blocking(move || client.log(lines))
        .await?
        .map_err(|e| match e {
            crate::control::ControlError::NotRunning => refusal("not_running", e.to_string()),
            other => refusal("refused", other.to_string()),
        })?;
    Ok(json!({ "rows": crate::activity::rows(&entries) }))
}

/// What the pages need to know about the app and this computer.
#[tauri::command]
pub fn app_info(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    let platform = crate::model::Platform::current();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": crate::user_agent::platform(),
        "server": desktop.server().origin(),
        "onboarded": crate::state::AppState::load(&desktop.paths).onboarded,
        "tray_place": platform.tray_place(),
        "settings_item": platform.settings_item(),
        "quit_item": platform.quit_item(),
        "agent_installed": crate::agent::cww_binary().is_some(),
        "documents": crate::paths::documents_dir(),
        "home": crate::paths::home_dir(),
        "status": desktop.status_json(),
    }))
}

#[tauri::command]
pub fn set_onboarded(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    onboarded: bool,
) -> Answer {
    local_page(&webview)?;
    let mut state = crate::state::AppState::load(&desktop.paths);
    state.onboarded = onboarded;
    state
        .save(&desktop.paths)
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", format!("Couldn't save: {e}")))
}

#[tauri::command]
pub async fn pick_folder(webview: Webview, app: AppHandle) -> Answer {
    local_page(&webview)?;
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Share a Folder")
        .set_parent(&webview.window());
    if let Some(documents) = crate::paths::documents_dir() {
        dialog = dialog.set_directory(documents);
    }
    let picked = blocking(move || dialog.blocking_pick_folder()).await?;
    Ok(match picked.and_then(|path| path.into_path().ok()) {
        Some(path) => json!({ "path": path }),
        None => json!({ "path": null }),
    })
}

/// Starts pairing; the code and the outcome arrive as `pairing` events.
#[tauri::command]
pub fn pair(webview: Webview, desktop: State<'_, Arc<Desktop>>, server: Option<String>) -> Answer {
    local_page(&webview)?;
    if let Some(server) = &server {
        crate::server::Server::parse(server).map_err(|e| refusal("invalid", format!("{e:#}")))?;
    }
    let app = desktop.app();
    let cancel = desktop.account.pair(
        server,
        Box::new(move |event| {
            crate::windows::tell_settings(&app, "pairing", pair_event(event));
        }),
    );
    if let Some(previous) = desktop
        .pairing
        .lock()
        .expect("pairing lock")
        .replace(cancel)
    {
        previous();
    }
    Ok(json!({}))
}

fn pair_event(event: crate::pairing::PairEvent) -> Value {
    use crate::pairing::PairEvent;
    match event {
        PairEvent::Code(code) => json!({
            "kind": "code",
            "user_code": code.user_code,
            "verification_uri": code.verification_uri,
            "browser_url": code.browser_url,
            "device_name": code.device_name,
            "fingerprint": code.fingerprint,
        }),
        PairEvent::Paired { daemon_error } => {
            json!({ "kind": "paired", "daemon_error": daemon_error })
        }
        PairEvent::Failed(message) => json!({ "kind": "failed", "message": message }),
    }
}

#[tauri::command]
pub fn cancel_pairing(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    if let Some(cancel) = desktop.pairing.lock().expect("pairing lock").take() {
        cancel();
    }
    Ok(json!({}))
}

#[tauri::command]
pub async fn logout(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    let account = desktop.account.clone();
    blocking(move || account.logout())
        .await?
        .map(|()| json!({}))
        .map_err(|message| refusal("failed", message))
}

/// Registers and starts the daemon's service (`cww daemon install`).
/// Turn the browser on or off: `cww browser enable` registers `cww` as the
/// extension's native messaging host for this user's browsers and turns
/// the tools on; `disable` removes both. Answers what `cww` said.
#[tauri::command]
pub async fn browser_enable(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    enabled: bool,
) -> Answer {
    local_page(&webview)?;
    let desktop = desktop.inner().clone();
    blocking(move || {
        if enabled {
            let request = ControlRequest::BrowserEnabled { enabled:true };
            crate::preferences::grant_context(&desktop.app(), &desktop, &webview, &request).map_err(|e| refusal("forbidden", e))?;
            let Some(_asking) = desktop.asking.start() else { return Err(refusal("busy", "Answer the app’s current question first.")); };
            let question = crate::computer::Question { title: "Connect this browser".into(), message: "Let Chat with Work connect to its browser extension on this computer? It uses only its tab group and the sites you allow, with your sign-ins.".into(), yes: "Connect browser".into() };
            if !crate::bridge::confirm(&desktop.app(), webview.window().label(), &question) { return Err(refusal("declined", "No changes made.")); }
            crate::preferences::grant_context(&desktop.app(), &desktop, &webview, &request).map_err(|e| refusal("forbidden", e))?;
        }
        crate::agent::browser(enabled).map(|said| json!({ "said": said })).map_err(|e| refusal("failed", format!("{e:#}")))
    }).await?
}

/// Windows: let commands see the top of the drives they run on, which
/// Node.js, npm and cmd's `dir` look at and Windows keeps from sandboxes
/// (`cww::commands::windows::DRIVE_ROOT_MASK`). The drives are the ones the
/// daemon names in its status; Windows asks an administrator, over the
/// application window.
#[tauri::command]
pub async fn prepare_command_drives(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    #[cfg(windows)]
    {
        let Some(status) = desktop.status() else {
            return Err(refusal(
                "not_running",
                "The background agent isn't running.",
            ));
        };
        let roots: Vec<std::path::PathBuf> = status
            .commands
            .map(|c| c.drives)
            .unwrap_or_default()
            .into_iter()
            .filter(|drive| !drive.prepared)
            .map(|drive| drive.root)
            .collect();
        let owner = webview.window().hwnd().ok().map(|hwnd| hwnd.0 as isize);
        let prepared = blocking(move || cww::commands::windows::prepare_drives(&roots, owner))
            .await?
            .map_err(|e| refusal("failed", e))?;
        // The daemon reads the drives again for its next status.
        let client = desktop.client.clone();
        let _ = blocking(move || client.request(&ControlRequest::Reload)).await;
        Ok(json!({ "prepared": prepared }))
    }
    #[cfg(not(windows))]
    {
        let _ = desktop;
        Err(refusal(
            "unsupported",
            "Only Windows keeps the top of a drive from the commands' sandbox.",
        ))
    }
}

#[tauri::command]
pub async fn start_agent(webview: Webview) -> Answer {
    local_page(&webview)?;
    blocking(crate::agent::start)
        .await?
        .map(|printed| json!({ "printed": printed }))
        .map_err(|e| refusal("failed", format!("{e:#}")))
}

#[tauri::command]
pub async fn autostart(webview: Webview) -> Answer {
    local_page(&webview)?;
    let on = blocking(crate::autostart::is_enabled).await?;
    Ok(json!({ "enabled": on }))
}

#[tauri::command]
pub async fn set_autostart(webview: Webview, enabled: bool) -> Answer {
    local_page(&webview)?;
    blocking(move || crate::autostart::set_enabled(enabled))
        .await?
        .map(|()| json!({ "enabled": enabled }))
        .map_err(|e| refusal("failed", format!("{e:#}")))
}

#[tauri::command]
pub fn crash_reports(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    let consent = cww::crash_reports::Consent::load(&desktop.paths.daemon);
    Ok(json!({
        "enabled": consent.enabled,
        "from_env": consent.from_env,
        "available": cww::crash_reports::available(),
        "undecided": consent.undecided(),
        "about": cww::crash_reports::ABOUT,
        "env": cww::crash_reports::ENV,
    }))
}

#[tauri::command]
pub fn set_crash_reports(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    enabled: bool,
) -> Answer {
    local_page(&webview)?;
    cww::crash_reports::set(&desktop.paths.daemon, enabled)
        .map(|()| json!({ "enabled": enabled }))
        .map_err(|e| {
            refusal(
                "failed",
                format!("Couldn't save the crash report setting: {e:#}"),
            )
        })
}

/// Opens one of the background agent's own files in its default app: `config`
/// (config.toml), `audit` (the activity log) or `log` (the daemon's log).
#[tauri::command]
pub fn open_file(webview: Webview, desktop: State<'_, Arc<Desktop>>, which: String) -> Answer {
    local_page(&webview)?;
    let daemon = &desktop.paths.daemon;
    let path = match which.as_str() {
        "config" => daemon.config_file(),
        "audit" => daemon.audit_file(),
        "log" => daemon.daemon_log_file(),
        _ => return Err(refusal("invalid", "No such file.")),
    };
    if !path.exists() {
        return Err(refusal(
            "missing",
            format!("{} doesn't exist yet.", path.display()),
        ));
    }
    open::that_detached(&path)
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", format!("Couldn't open {}: {e}", path.display())))
}

/// Opens a web page (help, the server's approval page) in the browser.
#[tauri::command]
pub fn open_url(webview: Webview, url: String) -> Answer {
    local_page(&webview)?;
    let url = tauri::Url::parse(&url).map_err(|_| refusal("invalid", "That isn't a link."))?;
    if !matches!(url.scheme(), "https" | "http") || !crate::policy::openable_externally(&url) {
        return Err(refusal("invalid", "Only web links open from here."));
    }
    open::that_detached(url.as_str())
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", format!("Couldn't open the link: {e}")))
}

/// Opens the Chat with Work window, on one of the server's pages (`path`,
/// such as `/settings?tab=notifications`) or where it was.
#[tauri::command]
pub async fn show_main(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    path: Option<String>,
) -> Answer {
    local_page(&webview)?;
    let url = match path {
        Some(path) => Some(
            desktop
                .server()
                .page(&path)
                .ok_or_else(|| refusal("invalid", "That isn't a page of Chat with Work."))?,
        ),
        None => None,
    };
    // The window checks the server first, which can take seconds.
    blocking(move || crate::windows::show_main(&app, url))
        .await?
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", e.to_string()))
}

#[tauri::command]
pub fn quit(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    desktop.quit();
    Ok(json!({}))
}

// Dictation for `cww tui`: cloud or on this computer, with Whisper models
// downloaded as soon as local dictation is chosen (and by `cww tui` itself
// on first use). The preference lives in the daemon's config folder.

#[derive(Default)]
pub struct Download {
    cancelled: Arc<AtomicBool>,
    running: AtomicBool,
    bytes: Arc<AtomicU64>,
}

fn model_by_id(id: &str) -> Option<cww::dictation::Model> {
    cww::dictation::Model::ALL
        .into_iter()
        .find(|m| m.id() == id)
}

#[tauri::command]
pub fn dictation(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    let paths = &desktop.paths.daemon;
    let (preferences, error) = match cww::dictation::Preferences::load(paths) {
        Ok(p) => (p, None),
        Err(e) => (Default::default(), Some(format!("{e:#}"))),
    };
    let recommended = cww::dictation::Model::for_this_computer();
    let models: Vec<Value> = cww::dictation::Model::ALL
        .into_iter()
        .map(|m| {
            json!({ "id": m.id(), "name": m.name(), "label": m.label(), "bytes": m.bytes(), "downloaded": m.downloaded(paths) })
        })
        .collect();
    Ok(json!({
        "mode": preferences.mode,
        // What local dictation uses, and whether someone picked it.
        "model": preferences.model(),
        "chosen": preferences.model,
        "recommended": recommended,
        "models": models,
        "error": error,
        "downloading": desktop.download.running.load(Ordering::SeqCst),
    }))
}

/// Saves the choice. Choosing to dictate on this computer starts getting
/// its model at once, so it's there by the first dictation.
#[tauri::command]
pub fn set_dictation(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    mode: cww::dictation::Mode,
    model: Option<cww::dictation::Model>,
) -> Answer {
    local_page(&webview)?;
    let preferences = cww::dictation::Preferences { mode, model };
    preferences
        .save(&desktop.paths.daemon)
        .map_err(|e| refusal("failed", format!("{e:#}")))?;
    let downloading = mode == cww::dictation::Mode::Local
        && !preferences.model().downloaded(&desktop.paths.daemon)
        && start_download(&desktop, preferences.model()).is_ok();
    Ok(json!({ "downloading": downloading }))
}

/// Downloads a model again after a download failed or was cancelled.
#[tauri::command]
pub fn download_model(webview: Webview, desktop: State<'_, Arc<Desktop>>, model: String) -> Answer {
    local_page(&webview)?;
    let model = model_by_id(&model).ok_or_else(|| refusal("invalid", "No such model."))?;
    start_download(&desktop, model).map(|()| json!({}))
}

/// Progress arrives as `dictation-download` events (`model`, `bytes`,
/// `total`, then `done` with an `error` if it failed).
fn start_download(desktop: &Arc<Desktop>, model: cww::dictation::Model) -> Answer<()> {
    let download = &desktop.download;
    if download.running.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "A model is already downloading."));
    }
    download.cancelled.store(false, Ordering::SeqCst);
    download.bytes.store(0, Ordering::SeqCst);
    let desktop = desktop.clone();
    let spawning = desktop.clone();
    std::thread::Builder::new()
        .name("cww-model-download".into())
        .spawn(move || {
            // Clears `running` however this ends, a panic included, so a
            // later download isn't refused as already running.
            struct Running(Arc<Desktop>);
            impl Drop for Running {
                fn drop(&mut self) {
                    self.0.download.running.store(false, Ordering::SeqCst);
                }
            }
            let _running = Running(desktop.clone());
            let app = desktop.app();
            let total = model.bytes();
            crate::windows::tell_settings(
                &app,
                "dictation-download",
                json!({ "model": model.id(), "bytes": 0, "total": total }),
            );
            let last = AtomicU64::new(0);
            let result = model.ensure(
                &desktop.paths.daemon,
                &desktop.download.cancelled,
                |bytes| {
                    desktop.download.bytes.store(bytes, Ordering::Relaxed);
                    // At most every megabyte, so the page isn't flooded.
                    if bytes.saturating_sub(last.load(Ordering::Relaxed)) >= 1_000_000
                        || bytes == total
                    {
                        last.store(bytes, Ordering::Relaxed);
                        crate::windows::tell_settings(
                            &app,
                            "dictation-download",
                            json!({ "model": model.id(), "bytes": bytes, "total": total }),
                        );
                    }
                },
            );
            desktop.download.running.store(false, Ordering::SeqCst);
            let error = result.err().map(|e| format!("{e:#}"));
            crate::windows::tell_settings(
                &app,
                "dictation-download",
                json!({ "model": model.id(), "done": true, "error": error }),
            );
        })
        .map_err(|e| {
            spawning.download.running.store(false, Ordering::SeqCst);
            refusal("failed", e.to_string())
        })?;
    Ok(())
}

#[tauri::command]
pub fn cancel_download(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    desktop.download.cancelled.store(true, Ordering::SeqCst);
    Ok(json!({}))
}

#[tauri::command]
pub fn remove_model(webview: Webview, desktop: State<'_, Arc<Desktop>>, model: String) -> Answer {
    local_page(&webview)?;
    let model = model_by_id(&model).ok_or_else(|| refusal("invalid", "No such model."))?;
    model
        .remove(&desktop.paths.daemon)
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", format!("{e:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_reach_only_the_requests_they_need() {
        for ok in [
            json!({ "cmd": "status" }),
            json!({ "cmd": "deny_extra", "previous": null, "pattern": "*.secret" }),
            json!({ "cmd": "pause" }),
            json!({ "cmd": "roots_label", "root": "docs", "label": "Docs" }),
            json!({ "cmd": "roots_bot", "root": "app", "bot": "42", "name": "Issue fixer", "lent": true, "unattended": false }),
            json!({ "cmd": "audit_tail", "lines": 50 }),
            json!({ "cmd": "notification_permission", "request": true }),
        ] {
            assert!(allowed(ok.clone()).is_ok(), "{ok}");
        }
        for refused in [
            json!({ "cmd": "shutdown" }),
            json!({ "cmd": "chat_send", "text": "hi" }),
            json!({ "cmd": "chats" }),
            json!({ "cmd": "asset", "path": "/x" }),
            json!({ "cmd": "subscribe", "topics": ["status"] }),
            json!({ "cmd": "reload" }),
            json!({}),
        ] {
            let error = allowed(refused.clone()).unwrap_err();
            assert_eq!(error["code"], "forbidden", "{refused}");
        }
        // Allowed by name, but malformed.
        assert_eq!(
            allowed(json!({ "cmd": "roots_label" })).unwrap_err()["code"],
            "invalid"
        );
    }

    #[test]
    fn every_allowed_request_is_one_the_daemon_knows() {
        for cmd in ALLOWED {
            let error = serde_json::from_value::<ControlRequest>(json!({ "cmd": cmd }))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(!error.contains("unknown variant"), "{cmd}: {error}");
        }
    }

    #[test]
    fn models_are_found_by_id() {
        assert_eq!(model_by_id("tiny"), Some(cww::dictation::Model::Tiny));
        assert_eq!(model_by_id("../../x"), None);
    }
}

// Updating the app by itself (update.rs).

#[tauri::command]
pub fn update_status(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    let view = desktop.updates.view();
    // A rolled-back update's message is shown once.
    desktop.updates.dismiss_problem();
    Ok(view)
}

#[tauri::command]
pub fn update_check(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    desktop.updates.check_now();
    Ok(desktop.updates.view())
}

/// Hands the downloaded update to its helper and quits.
#[tauri::command]
pub fn update_restart(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    local_page(&webview)?;
    desktop
        .restart_to_update()
        .map(|()| json!({}))
        .map_err(|e| refusal("failed", e))
}
