//! The app: one Tauri event loop for the tray, the web app's window and the
//! settings window, asleep until something happens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, RunEvent};

use crate::args::{Open, Options};
use crate::control::Client;
use crate::loopback::Callbacks;
use crate::model::{Status, TrayView};
use crate::paths::Paths;
use crate::server::Server;
use crate::tray::{Action, Tray};
use crate::windows::{self, MAIN};

/// What every part of the app shares.
pub struct Desktop {
    app: OnceLock<AppHandle>,
    pub paths: Paths,
    pub client: Client,
    server: Mutex<Server>,
    /// `--server` was given: pairing doesn't move the window elsewhere.
    server_fixed: bool,
    pub callbacks: Callbacks,
    pub account: Arc<dyn crate::pairing::Account>,
    pub pairing: Mutex<Option<crate::pairing::Cancel>>,
    pub download: crate::local::Download,
    status: Mutex<Option<Status>>,
    /// The watcher has reported at least once.
    status_known: AtomicBool,
    tray: Mutex<Option<Tray>>,
    quitting: AtomicBool,
}

impl Desktop {
    pub fn app(&self) -> AppHandle {
        self.app.get().expect("the app is running").clone()
    }

    /// The server whose pages the web app's windows show.
    pub fn server(&self) -> Server {
        self.server.lock().expect("server lock").clone()
    }

    /// What the settings pages show of the daemon: its status, and its
    /// health as the tray words it.
    pub fn status_json(&self) -> serde_json::Value {
        let status = self.status.lock().expect("status lock");
        let health = crate::model::Health::of(status.as_ref());
        serde_json::json!({
            "known": self.status_known.load(Ordering::SeqCst),
            "status": &*status,
            "health": { "label": health.label(), "tone": health.tone(), "serving": health.is_serving() },
        })
    }

    /// Hands a loopback callback to the page that asked for it, if that
    /// window still shows a page of the server.
    pub fn deliver_callback(&self, label: &str, detail: serde_json::Value) {
        let Some(window) = self.app().get_webview_window(label) else {
            return;
        };
        if window.url().is_ok_and(|url| self.server().owns(&url)) {
            let _ = window.eval(crate::bridge::callback_event(&detail));
            let _ = window.set_focus();
        }
    }

    pub fn quit(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.app().exit(0);
    }

    /// A new status from the daemon (`None` while it isn't running).
    fn status_changed(&self, status: Option<Status>) {
        let app = self.app();
        let view = TrayView::new(status.as_ref());
        if let Some(tray) = self.tray.lock().expect("tray lock").as_mut() {
            tray.update(&view);
        }
        let paired = status.as_ref().and_then(|s| s.server.clone());
        *self.status.lock().expect("status lock") = status;
        self.status_known.store(true, Ordering::SeqCst);
        crate::windows::tell_settings(&app, "status", self.status_json());
        // Paired with another server: the web app follows.
        if !self.server_fixed
            && let Some(paired) = paired
            && let Ok(server) = Server::parse(&paired)
            && server != self.server()
        {
            log::info!("now paired with {}", server.origin());
            *self.server.lock().expect("server lock") = server.clone();
            if let Err(e) = add_remote_capability(&app, &server) {
                log::warn!("couldn't allow the bridge on the new server: {e}");
            }
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.navigate(server.start_url());
            }
        }
    }
}

/// The bridge's commands, for pages of `server` in the web app's windows
/// only. Remote pages get nothing else: no core APIs, no events.
fn add_remote_capability(app: &AppHandle, server: &Server) -> tauri::Result<()> {
    let identifier = format!("remote-{}", server.origin());
    app.add_capability(
        CapabilityBuilder::new(identifier)
            .remote(server.capability_pattern())
            .windows([MAIN.to_string(), format!("{}*", windows::POPUP_PREFIX)])
            .permission("allow-open-external")
            .permission("allow-open-preferences")
            .permission("allow-listen"),
    )
}

/// What a tray entry asks of the app.
pub fn tray_action(app: &AppHandle, id: &str) {
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    match crate::tray::action(id) {
        Some(Action::Open) => {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = windows::show_main(&app, None);
            });
        }
        Some(Action::Settings) => {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = windows::show_settings(&app, None);
            });
        }
        Some(Action::TogglePause) => {
            let paused = desktop
                .status
                .lock()
                .expect("status lock")
                .as_ref()
                .is_some_and(|s| s.paused);
            let client = desktop.client.clone();
            std::thread::spawn(move || {
                if let Err(e) = client.set_paused(!paused) {
                    log::warn!("{e}");
                }
            });
        }
        Some(Action::Quit) => desktop.quit(),
        None => {}
    }
}

/// Opens what a launch asked for.
fn open(app: &AppHandle, open: &Open) {
    let desktop = app.state::<Arc<Desktop>>().inner().clone();
    let result = match open {
        Open::Main => windows::show_main(app, None),
        Open::Chat(target) => match desktop.server().chat_url(target) {
            Some(url) => windows::show_main(app, Some(url)),
            None => {
                log::info!("a notification for another server; ignored");
                Ok(())
            }
        },
        Open::Settings(page) => windows::show_settings(app, *page),
        Open::Nothing => Ok(()),
    };
    if let Err(e) = result {
        log::error!("couldn't open a window: {e}");
    }
}

pub fn run(options: Options) -> anyhow::Result<()> {
    #[cfg(feature = "demo")]
    let demo = match options.demo {
        Some(fresh) => {
            let scenario = if fresh {
                crate::demo::Scenario::Fresh
            } else {
                crate::demo::Scenario::Sample
            };
            let server = crate::demo::start(scenario)?;
            if scenario == crate::demo::Scenario::Sample {
                server.script_live_events();
            }
            Some(server)
        }
        None => None,
    };
    #[cfg(feature = "demo")]
    let (paths, account): (Paths, Arc<dyn crate::pairing::Account>) = match &demo {
        Some(server) => (server.paths.clone(), server.account()),
        None => (Paths::from_env()?, Arc::new(crate::pairing::Cli)),
    };
    #[cfg(not(feature = "demo"))]
    let (paths, account): (Paths, Arc<dyn crate::pairing::Account>) =
        (Paths::from_env()?, Arc::new(crate::pairing::Cli));

    let server = Server::resolve(options.server.as_deref(), Some(&paths.daemon))?;
    // Moves an earlier version's login agent onto the bundled one. It asks
    // SMAppService, so not on the thread that runs the windows.
    #[cfg(target_os = "macos")]
    if let Err(e) = std::thread::Builder::new()
        .name("cww-autostart".into())
        .spawn(crate::autostart::migrate)
    {
        log::warn!("can't move start at login: {e}");
    }
    // Until the welcome page is done, the app opens on it.
    let first_open = match &options.open {
        Open::Main if !crate::state::AppState::load(&paths).onboarded => {
            Open::Settings(Some("welcome"))
        }
        other => other.clone(),
    };
    let desktop = Arc::new(Desktop {
        app: OnceLock::new(),
        client: Client::new(paths.socket_path()),
        paths,
        server: Mutex::new(server),
        server_fixed: options.server.is_some(),
        callbacks: Callbacks::default(),
        account,
        pairing: Mutex::new(None),
        download: Default::default(),
        status: Mutex::new(None),
        status_known: AtomicBool::new(false),
        tray: Mutex::new(None),
        quitting: AtomicBool::new(false),
    });

    let mut builder = tauri::Builder::default();
    #[cfg(feature = "demo")]
    let single = demo.is_none();
    #[cfg(not(feature = "demo"))]
    let single = true;
    if single {
        // First, so a second launch hands over before anything else starts.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            match crate::args::parse(argv.into_iter().skip(1)) {
                // A start at login while the app runs changes nothing.
                Ok(crate::args::Parsed::Run(options)) if options.background => {}
                Ok(crate::args::Parsed::Run(options)) => {
                    let app = app.clone();
                    std::thread::spawn(move || open(&app, &options.open));
                }
                _ => {}
            }
        }));
    }
    let setup_desktop = desktop.clone();
    let app = builder
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .manage(desktop.clone())
        // The microphone, for the composer's dictation, only for the
        // server's pages; the bundled pages and other sites ask nothing.
        .on_permission_request(|webview, kind| {
            let desktop = webview.state::<Arc<Desktop>>();
            let ours = crate::windows::is_remote_label(webview.label())
                && webview.url().is_ok_and(|url| desktop.server().owns(&url));
            match kind {
                PermissionKind::Microphone if ours => PermissionResponse::Allow,
                _ => PermissionResponse::Deny,
            }
        })
        .invoke_handler(tauri::generate_handler![
            crate::bridge::open_external,
            crate::bridge::open_preferences,
            crate::bridge::listen,
            crate::local::control,
            crate::local::activity,
            crate::local::app_info,
            crate::local::set_onboarded,
            crate::local::pick_folder,
            crate::local::pair,
            crate::local::cancel_pairing,
            crate::local::logout,
            crate::local::start_agent,
            crate::local::autostart,
            crate::local::set_autostart,
            crate::local::crash_reports,
            crate::local::set_crash_reports,
            crate::local::open_file,
            crate::local::open_url,
            crate::local::show_main,
            crate::local::quit,
            crate::local::dictation,
            crate::local::set_dictation,
            crate::local::download_model,
            crate::local::cancel_download,
            crate::local::remove_model,
        ])
        .setup(move |app| {
            let desktop = setup_desktop;
            let handle = app.handle().clone();
            let _ = desktop.app.set(handle.clone());
            add_remote_capability(&handle, &desktop.server())?;
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            match Tray::new(&handle, &TrayView::new(None)) {
                Ok(tray) => *desktop.tray.lock().expect("tray lock") = Some(tray),
                Err(e) => log::warn!("no tray item: {e}"),
            }
            let watcher = desktop.clone();
            desktop
                .client
                .spawn_watch(move |status| watcher.status_changed(status));
            let opener = handle.clone();
            std::thread::spawn(move || open(&opener, &first_open));
            #[cfg(all(target_os = "linux", feature = "demo"))]
            if let Some(dir) = crate::snapshot::requested() {
                std::thread::spawn(move || crate::snapshot::run(handle, dir));
            }
            Ok(())
        })
        .build(tauri::generate_context!())?;

    app.run(move |app, event| match event {
        // Closing the last window leaves the tray item running, and the
        // web views' processes with the windows. Quit (or a tray-less
        // desktop) ends the app.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            let desktop = app.state::<Arc<Desktop>>();
            let has_tray = desktop.tray.lock().expect("tray lock").is_some();
            if has_tray && !desktop.quitting.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
        RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { .. },
            ..
        } => {
            use tauri_plugin_window_state::AppHandleExt;
            let _ = app.save_window_state(tauri_plugin_window_state::StateFlags::all());
        }
        RunEvent::WindowEvent {
            event: tauri::WindowEvent::Destroyed,
            ..
        } => {
            // A menu bar app again: no Dock icon while no window is open.
            #[cfg(target_os = "macos")]
            if !windows::any_open(app) {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let _ = app;
        }
        _ => {}
    });
    #[cfg(feature = "demo")]
    drop(demo);
    Ok(())
}
