//! Updating the app by itself, through fastframe-update.
//!
//! Once a day (and when the person asks), the app reads the latest release
//! on GitHub. A copy that may replace itself, the Mac app from the disk
//! image and the Windows installer's, downloads it in the background,
//! checks `checksums.txt` against its signature with the key compiled in
//! here (`assets/update-public-key.hex`, the one `release.yml` signs with)
//! and the package against `checksums.txt`, and offers Restart to Update in
//! the tray and in General settings. On restart a helper installs it,
//! starts the new version and puts the old one back if that doesn't start.
//!
//! A copy a package manager owns never replaces itself (Homebrew, the AUR,
//! .deb and .rpm, Nix, the Mac's .pkg); the settings say which tool updates
//! it. WinGet installs the Windows installer's own package, so that copy
//! updates itself like any other, and `winget upgrade` still works. The Linux archive carries `cww`
//! beside the app and the updater replaces one program, so a copy from the
//! one-line installer is told to run it again instead.
//!
//! After an update the background agent runs the old `cww` until it
//! restarts, so the new version restarts it (`cww daemon install`) when its
//! version differs. The Windows installer does that itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastframe_update::{
    CHECK_INTERVAL, MacConfig, PackageManager, Prepared, Receipt, Release, Request, Response,
    Source, Transport, Unsupported, UpdateConfig, Updater,
};
use serde_json::{Value, json};

/// The version this binary was built as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where a person downloads a release by hand.
pub const RELEASES: &str = "https://github.com/crmne/chatwithwork-desktop/releases/latest";

/// The release assets' name on this platform: `chat-with-work-v<version>-
/// macos-universal.dmg`, `cww-v<version>-<target>.msi`, and on Linux the
/// `cww-app-v<version>-<target>.tar.gz` archive (which never updates
/// itself; see the module's note).
const SLUG: &str = if cfg!(target_os = "macos") {
    "chat-with-work"
} else if cfg!(windows) {
    "cww"
} else {
    "cww-app"
};

/// The key `checksums.txt.sig` is checked with. A build for an end-to-end
/// test of the updater may name another with `CWW_UPDATE_PUBLIC_KEY` when it
/// is compiled; release builds never set it.
const PUBLIC_KEY: &str = match option_env!("CWW_UPDATE_PUBLIC_KEY") {
    Some(key) => key,
    None => include_str!("../assets/update-public-key.hex"),
};

pub const CONFIG: UpdateConfig = UpdateConfig {
    // On Windows the installer's package carries cww.exe, cww-agent.exe and
    // the scanner's helper beside the app, and only the app runs the
    // updater.
    portable_executable: if cfg!(windows) { Some("cww-app") } else { None },
    macos: MacConfig {
        // packaging/macos/Info.plist.
        bundle_ids: &["com.chatwithwork.desktop"],
        executable_names: &["cww-app"],
        legacy_bundle_names: &[],
    },
    publisher_key: Some(PUBLIC_KEY.trim_ascii()),
    ..UpdateConfig::new(
        "crmne/chatwithwork-desktop",
        "Chat with Work",
        SLUG,
        VERSION,
    )
};

/// What `--version` prints. Before installing a Mac update (and a Linux
/// archive's), the running version's helper runs the downloaded app with
/// `--version` and accepts only `<slug> <version>`, so on a Mac the app
/// answers `chat-with-work 1.2.3`. Windows keeps `cww-app`, the program's
/// name, since its installer's package isn't asked.
pub fn version_line() -> String {
    let name = if cfg!(windows) { "cww-app" } else { CONFIG.slug };
    format!("{name} {VERSION}")
}

/// Releases come from GitHub, or for an end-to-end test from a feed on
/// this computer (`CWW_UPDATE_FEED=http://127.0.0.1:<port>`, loopback HTTP
/// only). The signature is required either way.
fn updater() -> Updater {
    let updater = Updater::new(CONFIG, UreqTransport);
    match std::env::var("CWW_UPDATE_FEED") {
        Ok(feed) => match Source::local(&feed) {
            Ok(source) => updater.with_source(source),
            Err(e) => {
                log::warn!("ignoring CWW_UPDATE_FEED: {e:#}");
                updater
            }
        },
        Err(_) => updater,
    }
}

/// fastframe-update's transport over ureq, with the TLS roots and proxy the
/// rest of Chat with Work Desktop uses (config.toml's `proxy`, else the
/// environment). It follows no redirects and treats every status as an
/// answer: the updater follows redirects itself, only to GitHub's hosts.
struct UreqTransport;

impl Transport for UreqTransport {
    fn get(&self, request: &Request<'_>) -> anyhow::Result<Response> {
        let url = url::Url::parse(request.url)?;
        let proxy = match cww::paths::Paths::from_env()
            .ok()
            .and_then(|paths| cww::config::Config::load(&paths).ok())
        {
            Some(config) if url.scheme() == "https" => {
                let origin =
                    cww::auth::client::ServerUrl::parse(&url.origin().ascii_serialization())?;
                cww::proxy::for_server(config.proxy.as_deref(), &origin)?
                    .map(|proxy| proxy.to_ureq())
                    .transpose()?
            }
            _ => None,
        };
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(proxy)
            .timeout_connect(Some(Duration::from_secs(15)))
            // Long enough for the whole disk image on a slow line.
            .timeout_global(Some(Duration::from_secs(20 * 60)))
            .tls_config(cww::tls::ureq_config())
            .build()
            .new_agent();
        let response = agent
            .get(request.url)
            .header("Accept", request.accept)
            .header("User-Agent", request.user_agent)
            .call()?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Response {
            status,
            location,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

/// How a copy that can't update itself is updated, in the app's words.
pub fn advice(reason: &Unsupported) -> String {
    match reason {
        Unsupported::Homebrew => "Installed with Homebrew: update it with brew upgrade.".into(),
        Unsupported::SystemPackage(PackageManager::Pacman) => {
            "Installed from the AUR: update it with your AUR helper or pacman.".into()
        }
        Unsupported::SystemPackage(PackageManager::Apt) => {
            "Installed from a .deb package: update it with apt or your software center.".into()
        }
        Unsupported::SystemPackage(PackageManager::Dnf) => {
            "Installed from an .rpm package: update it with dnf or your software center.".into()
        }
        Unsupported::Nix => "Installed with Nix: update it through Nix.".into(),
        Unsupported::Flatpak => "Installed with Flatpak: update it with flatpak update.".into(),
        Unsupported::Snap => "Installed with Snap: update it with snap refresh.".into(),
        Unsupported::Cargo => "Built with cargo: build the new version the same way.".into(),
        Unsupported::MoveToApplications => {
            "Move Chat with Work to Applications, then open it again to update it here.".into()
        }
        Unsupported::SystemDirectory => {
            "Installed system-wide: update it through the package manager that installed it."
                .into()
        }
        // The one-line installer's copy on Linux, or one built from source.
        Unsupported::NotPortable if cfg!(target_os = "linux") => {
            "Update it the way you installed it: run the one-line installer again, or use your package manager.".into()
        }
        _ => "This copy can't replace itself. Download the new version from the release page."
            .into(),
    }
}

/// The Mac's .pkg installs `cww` into /usr/local/bin as root, beside the
/// app; replacing only the app would leave that one behind, so a Mac with
/// the package's receipt updates with the next package instead.
fn installed_by_pkg() -> Option<String> {
    (cfg!(target_os = "macos")
        && std::path::Path::new("/var/db/receipts/com.chatwithwork.cww.plist").exists())
    .then(|| {
        "Installed with the .pkg installer: install the new version's .pkg from the release page."
            .into()
    })
}

/// Where updating stands, for the settings page and the tray.
#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Checking,
    /// The latest release is this one.
    Current,
    /// A newer release this copy can't install itself.
    Unsupported {
        release: Release,
        advice: String,
    },
    Downloading {
        release: Release,
        received: u64,
        total: u64,
    },
    /// Downloaded and verified, waiting for Restart to Update.
    Ready {
        release: Release,
        prepared: Box<Prepared>,
    },
    Installing {
        release: Release,
    },
    Failed(String),
}

impl Phase {
    fn release(&self) -> Option<&Release> {
        match self {
            Self::Unsupported { release, .. }
            | Self::Downloading { release, .. }
            | Self::Ready { release, .. }
            | Self::Installing { release } => Some(release),
            _ => None,
        }
    }

    fn view(&self) -> Value {
        let release = self
            .release()
            .map(|r| json!({ "version": r.version, "url": r.url }));
        match self {
            Self::Idle => json!({ "phase": "idle" }),
            Self::Checking => json!({ "phase": "checking" }),
            Self::Current => json!({ "phase": "current" }),
            Self::Unsupported { advice, .. } => {
                json!({ "phase": "unsupported", "release": release, "advice": advice })
            }
            Self::Downloading {
                received, total, ..
            } => {
                json!({ "phase": "downloading", "release": release, "received": received, "total": total })
            }
            Self::Ready { .. } => json!({ "phase": "ready", "release": release }),
            Self::Installing { .. } => json!({ "phase": "installing", "release": release }),
            Self::Failed(error) => json!({ "phase": "failed", "error": error }),
        }
    }
}

/// The updater's state, shared by its thread, the settings and the tray.
pub struct Updates {
    phase: Mutex<Phase>,
    /// When the last check ended, as the settings show it.
    checked_at: Mutex<Option<jiff::Timestamp>>,
    /// The message of an update that was rolled back, until it's seen.
    problem: Mutex<Option<String>>,
    automatic: AtomicBool,
    /// This launch is the previous version, put back after an update
    /// didn't start.
    rolled_back: bool,
    wake: Mutex<Option<Sender<()>>>,
}

impl Updates {
    pub fn new(automatic: bool, problem: Option<String>) -> Self {
        Self {
            phase: Mutex::new(Phase::Idle),
            checked_at: Mutex::new(None),
            rolled_back: problem.is_some(),
            problem: Mutex::new(problem),
            automatic: AtomicBool::new(automatic),
            wake: Mutex::new(None),
        }
    }

    /// What the settings show.
    pub fn view(&self) -> Value {
        let mut view = self.phase.lock().expect("update lock").view();
        view["version"] = json!(VERSION);
        view["automatic"] = json!(self.automatic.load(Ordering::SeqCst));
        view["checked_at"] = json!(
            self.checked_at
                .lock()
                .expect("update lock")
                .map(|t| t.to_string())
        );
        view["problem"] = json!(*self.problem.lock().expect("update lock"));
        view["releases"] = json!(RELEASES);
        view
    }

    /// The version waiting for a restart, for the tray.
    pub fn ready(&self) -> Option<String> {
        match &*self.phase.lock().expect("update lock") {
            Phase::Ready { release, .. } => Some(release.version.clone()),
            _ => None,
        }
    }

    pub fn set_automatic(&self, on: bool) {
        self.automatic.store(on, Ordering::SeqCst);
        if on {
            self.check_now();
        }
    }

    /// Checks now, whatever the schedule.
    pub fn check_now(&self) {
        if let Some(wake) = &*self.wake.lock().expect("update lock") {
            let _ = wake.send(());
        }
    }

    pub fn dismiss_problem(&self) {
        *self.problem.lock().expect("update lock") = None;
    }

    fn set(&self, phase: Phase, tell: &dyn Fn()) {
        *self.phase.lock().expect("update lock") = phase;
        tell();
    }

    /// One check, and the download when this copy may install it.
    fn run_once(&self, updater: &Updater, tell: &dyn Fn()) {
        // A downloaded update stays ready; checking again would only fetch
        // it again.
        if matches!(
            &*self.phase.lock().expect("update lock"),
            Phase::Ready { .. } | Phase::Installing { .. } | Phase::Downloading { .. }
        ) {
            return;
        }
        self.set(Phase::Checking, tell);
        let checked = updater.check();
        *self.checked_at.lock().expect("update lock") = Some(jiff::Timestamp::now());
        let release = match checked {
            Ok(Some(release)) => release,
            Ok(None) => return self.set(Phase::Current, tell),
            Err(e) => {
                // Offline, rate limited, GitHub down: say so only here.
                log::info!("update check failed: {e:#}");
                return self.set(
                    Phase::Failed(
                        "Couldn't check for updates. Check your connection and try again.".into(),
                    ),
                    tell,
                );
            }
        };
        if let Some(advice) = installed_by_pkg() {
            return self.set(Phase::Unsupported { release, advice }, tell);
        }
        if let Err(reason) = updater.installation() {
            log::info!(
                "update {} available; this copy can't install it: {reason:?}",
                release.version
            );
            let advice = advice(&reason);
            return self.set(Phase::Unsupported { release, advice }, tell);
        }
        log::info!("downloading update {}", release.version);
        self.set(
            Phase::Downloading {
                release: release.clone(),
                received: 0,
                total: 0,
            },
            tell,
        );
        let mut told = 0u64;
        let downloaded = updater.download(&release, |received, total| {
            if let Phase::Downloading {
                received: r,
                total: t,
                ..
            } = &mut *self.phase.lock().expect("update lock")
            {
                *r = received;
                *t = total;
            }
            // At most once a percent.
            let percent = received.saturating_mul(100) / total.max(1);
            if percent != told {
                told = percent;
                tell();
            }
        });
        match downloaded {
            Ok(prepared) => self.set(
                Phase::Ready {
                    release,
                    prepared: Box::new(prepared),
                },
                tell,
            ),
            Err(e) => {
                log::warn!("update download failed: {e:#}");
                self.set(
                    Phase::Failed(format!(
                        "Couldn't download version {}: {e:#}",
                        release.version
                    )),
                    tell,
                );
            }
        }
    }

    /// Checks at start and once a day while checks are on, and whenever
    /// asked. `tell` runs on every change.
    pub fn spawn(self: &Arc<Self>, tell: impl Fn() + Send + 'static) {
        let (wake, woken): (Sender<()>, Receiver<()>) = channel();
        *self.wake.lock().expect("update lock") = Some(wake);
        let updates = self.clone();
        let spawned = std::thread::Builder::new()
            .name("cww-updates".into())
            .spawn(move || {
                let updater = updater();
                // Let the windows and the agent start first.
                let mut asked = woken.recv_timeout(Duration::from_secs(20)).is_ok();
                // Right after an update was rolled back, the same release
                // would download again at once; it waits a day, unless asked.
                let mut rolled_back = updates.rolled_back;
                loop {
                    if asked || (updates.automatic.load(Ordering::SeqCst) && !rolled_back) {
                        updates.run_once(&updater, &tell);
                    }
                    rolled_back = false;
                    asked = match woken.recv_timeout(CHECK_INTERVAL) {
                        Ok(()) => true,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                }
            });
        if let Err(e) = spawned {
            log::warn!("no update checks: {e}");
        }
    }

    /// Hands the downloaded update to its helper, which waits for this app
    /// to quit. Returns once the helper has it; the caller quits.
    pub fn restart(&self, relaunch: Vec<String>) -> Result<(), String> {
        let (release, prepared) = {
            let mut phase = self.phase.lock().expect("update lock");
            match std::mem::take(&mut *phase) {
                Phase::Ready { release, prepared } => {
                    *phase = Phase::Installing {
                        release: release.clone(),
                    };
                    (release, prepared)
                }
                other => {
                    *phase = other;
                    return Err("No update is ready to install.".into());
                }
            }
        };
        log::info!("installing update {}", release.version);
        updater().handoff(*prepared, relaunch).map_err(|e| {
            let message = format!("Couldn't start the update: {e:#}");
            *self.phase.lock().expect("update lock") = Phase::Failed(message.clone());
            message
        })
    }
}

/// What the app relaunches with after an update: its window if one was
/// open, else just the tray.
pub fn relaunch_arguments(window_open: bool) -> Vec<String> {
    if window_open {
        Vec::new()
    } else {
        vec!["--background".into()]
    }
}

/// After an update this launch installed: tells the helper the new version
/// started, then restarts the background agent if it still runs the old
/// `cww`. `agent_version` waits for the agent's status (None while it
/// isn't running, when there's nothing to restart).
pub fn finish(receipt: Receipt, agent_version: impl FnOnce() -> Option<String>) {
    if let Err(e) = receipt.acknowledge() {
        log::warn!("couldn't acknowledge the update: {e:#}");
        return;
    }
    log::info!("updated to {VERSION}");
    if cfg!(windows) {
        // The installer stopped the agent and started the new one.
        return;
    }
    match agent_version() {
        Some(version) if version != VERSION => match crate::agent::start() {
            Ok(_) => log::info!("restarted the background agent ({version} to {VERSION})"),
            Err(e) => log::warn!("couldn't restart the background agent: {e:#}"),
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_is_valid_and_names_this_platforms_assets() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, VERSION);
        if cfg!(target_os = "macos") {
            assert_eq!(CONFIG.slug, "chat-with-work");
        } else if cfg!(windows) {
            assert_eq!(CONFIG.slug, "cww");
            assert_eq!(CONFIG.portable_executable, Some("cww-app"));
        } else {
            assert_eq!(CONFIG.slug, "cww-app");
        }
        assert_eq!(CONFIG.publisher_key.unwrap().len(), 64);
    }

    #[test]
    fn the_version_answers_the_helpers_probe() {
        if !cfg!(windows) {
            assert_eq!(version_line(), format!("{} {VERSION}", CONFIG.slug));
        }
        assert!(version_line().ends_with(VERSION));
    }

    #[test]
    fn package_managers_are_named_and_the_linux_installer_run_again() {
        assert!(advice(&Unsupported::Homebrew).contains("brew upgrade"));
        assert!(advice(&Unsupported::SystemPackage(PackageManager::Pacman)).contains("AUR"));
        assert!(advice(&Unsupported::Nix).contains("Nix"));
        let manual = advice(&Unsupported::NotPortable);
        if cfg!(target_os = "linux") {
            assert!(manual.contains("one-line installer"));
        } else {
            assert!(manual.contains("release page"));
        }
    }

    #[test]
    fn a_relaunch_brings_back_the_window_only_when_one_was_open() {
        assert!(relaunch_arguments(true).is_empty());
        assert_eq!(relaunch_arguments(false), ["--background"]);
        assert!(matches!(
            crate::args::parse(relaunch_arguments(false).into_iter()),
            Ok(crate::args::Parsed::Run(options)) if options.background
        ));
    }

    #[test]
    fn the_view_says_where_updating_stands() {
        let updates = Updates::new(true, Some("restored".into()));
        let view = updates.view();
        assert_eq!(view["phase"], "idle");
        assert_eq!(view["version"], VERSION);
        assert_eq!(view["automatic"], true);
        assert_eq!(view["problem"], "restored");
        updates.dismiss_problem();
        assert!(updates.view()["problem"].is_null());
        assert!(updates.ready().is_none());
        assert!(updates.restart(Vec::new()).is_err());
        *updates.phase.lock().unwrap() = Phase::Failed("offline".into());
        assert_eq!(updates.view()["error"], "offline");
    }
}
