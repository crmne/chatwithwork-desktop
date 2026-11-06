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
//! .deb and .rpm, Nix, Flatpak, the Mac's .pkg), nor does an AppImage whose
//! release doesn't sign it. It still hears of each release once its signed
//! `checksums.txt` verifies (`Updater::verify`): General and the tray show
//! one notice per version, with the command for how it was installed
//! (update_notice.rs). WinGet installs the Windows installer's own package, so that copy
//! updates itself like any other, and `winget upgrade` still works. The Linux archive carries `cww`
//! beside the app and the updater replaces one program, so a copy from the
//! one-line installer is told to run it again instead.
//!
//! After an update the background agent runs the old `cww` until it
//! restarts, so every app launch checks the running agent against the app
//! and its installed `cww`, including package-manager upgrades. A stopped
//! agent remains stopped. The Windows installer restarts its agent itself.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastframe_update::{
    CHECK_INTERVAL, MacConfig, PackageManager, Prepared, Receipt, Release, Request, Response,
    Source, Transport, Unsupported, UpdateConfig, Updater,
};
use serde_json::{Value, json};

use crate::update_notice::Notice;

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
    // native-packages names the AppImage after the package:
    // chatwithwork-desktop-X.Y.Z-<arch>.AppImage. AppImages update
    // themselves once a release lists it in the signed checksums.txt;
    // until then they get the notice (update_notice.rs).
    appimage_name: Some("chatwithwork-desktop"),
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
    let name = if cfg!(windows) {
        "cww-app"
    } else {
        CONFIG.slug
    };
    format!("{name} {VERSION}")
}

/// Releases come from GitHub, or for an end-to-end test from a feed on
/// this computer (`CWW_UPDATE_FEED=http://127.0.0.1:<port>`, loopback HTTP
/// only). The signature is required either way.
fn updater() -> Updater {
    let updater = Updater::new(CONFIG, UreqTransport);
    match local_feed() {
        Some(source) => updater.with_source(source),
        None => updater,
    }
}

fn local_feed() -> Option<Source> {
    let feed = std::env::var("CWW_UPDATE_FEED").ok()?;
    Source::local(&feed)
        .inspect_err(|e| log::warn!("ignoring CWW_UPDATE_FEED: {e:#}"))
        .ok()
}

/// The release's page, for its notes: the tag's page on GitHub, which the
/// app builds from the version rather than taking from the listing.
fn notes(release: &Release) -> String {
    if local_feed().is_some() {
        release.url.clone()
    } else {
        format!(
            "https://github.com/{}/releases/tag/v{}",
            CONFIG.repository, release.version
        )
    }
}

/// Only `X.Y.Z`: the notice puts the version into file names and commands.
fn plain_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit())
        })
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
        Unsupported::Flatpak => {
            "Installed with Flatpak: update it with flatpak update, or install the newer .flatpak from the latest release if you installed a downloaded one."
                .into()
        }
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
fn installed_by_pkg() -> bool {
    cfg!(target_os = "macos")
        && std::path::Path::new("/var/db/receipts/com.chatwithwork.cww.plist").exists()
}

/// Where updating stands, for the settings page and the tray.
#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Checking,
    /// The latest release is this one.
    Current,
    /// A newer, verified release this copy can't install itself.
    Unsupported {
        release: Release,
        notice: Notice,
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
            Self::Unsupported { notice, .. } => json!({
                "phase": "unsupported",
                "release": release,
                "advice": notice.how,
                "command": notice.command,
                "download": notice.download,
            }),
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
    /// The newest version that ran before this start.
    newest_run: Mutex<Option<String>>,
    /// When the last check ended, as the settings show it.
    checked_at: Mutex<Option<jiff::Timestamp>>,
    /// The message of an update that was rolled back, until it's seen.
    problem: Mutex<Option<String>>,
    /// This launch is the previous version, put back after an update
    /// didn't start.
    rolled_back: bool,
    /// The version whose notice was dismissed (app.json), so a copy that
    /// can't update itself is told once per release.
    dismissed: Mutex<Option<String>>,
    /// Install updates automatically (app.json's `automatic_updates`).
    automatic: std::sync::atomic::AtomicBool,
    wake: Mutex<Option<Sender<()>>>,
}

impl Updates {
    pub fn new(problem: Option<String>, dismissed: Option<String>, automatic: bool) -> Self {
        Self {
            automatic: std::sync::atomic::AtomicBool::new(automatic),
            phase: Mutex::new(Phase::Idle),
            newest_run: Mutex::new(None),
            checked_at: Mutex::new(None),
            rolled_back: problem.is_some(),
            problem: Mutex::new(problem),
            dismissed: Mutex::new(dismissed),
            wake: Mutex::new(None),
        }
    }

    /// Says what newest version ran before this start, as `remember_run`
    /// answers it.
    pub fn with_newest_run(self, newest_run: Option<String>) -> Self {
        *self.newest_run.lock().expect("update lock") = newest_run;
        self
    }

    /// The newest version that ran before this start.
    pub fn newest_run(&self) -> Option<String> {
        self.newest_run.lock().expect("update lock").clone()
    }

    /// Whether updates install themselves.
    pub fn installs_updates(&self) -> bool {
        self.automatic.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_installs_updates(&self, on: bool) {
        self.automatic
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// This launch is the version an update was rolled back to.
    pub fn rolled_back(&self) -> bool {
        self.rolled_back
    }

    /// What the settings show.
    pub fn view(&self) -> Value {
        let mut view = self.phase.lock().expect("update lock").view();
        if view["phase"] == "unsupported" {
            view["dismissed"] = json!(self.available().is_none());
        }
        view["version"] = json!(VERSION);
        view["automatic"] = json!(true);
        let installs = self.installs_updates();
        view["installs_updates"] = json!(installs);
        // A downloaded update that installs at the next start on its own.
        if let Some(version) = view["release"]["version"].as_str()
            && view["phase"] == "ready"
        {
            let newest_run = self.newest_run();
            view["installs_next_start"] = json!(installs_on_its_own(
                VERSION,
                version,
                installs,
                newest_run.as_deref()
            ));
        }
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

    /// A newer release this copy can't install, until its notice is
    /// dismissed: the tray offers it.
    pub fn available(&self) -> Option<String> {
        let dismissed = self.dismissed.lock().expect("update lock").clone();
        match &*self.phase.lock().expect("update lock") {
            Phase::Unsupported { release, .. } if dismissed.as_ref() != Some(&release.version) => {
                Some(release.version.clone())
            }
            _ => None,
        }
    }

    /// Dismisses the notice of the release it's showing (and its banner),
    /// and returns that version, for app.json. A downloaded update's banner
    /// goes too; the tray and General still offer Restart to Update.
    pub fn dismiss(&self, version: &str) -> Option<String> {
        (self.banner_version()? == version).then(|| {
            *self.dismissed.lock().expect("update lock") = Some(version.to_owned());
            version.to_owned()
        })
    }

    /// The release the banner shows: one waiting for Restart to Update or
    /// for a package manager, until it's dismissed.
    pub fn banner_version(&self) -> Option<String> {
        let dismissed = self.dismissed.lock().expect("update lock").clone();
        match &*self.phase.lock().expect("update lock") {
            Phase::Ready { release, .. } | Phase::Unsupported { release, .. }
                if dismissed.as_ref() != Some(&release.version) =>
            {
                Some(release.version.clone())
            }
            _ => None,
        }
    }

    /// What the banner in the Chat with Work window shows, or `None` to
    /// hide it.
    pub fn banner(&self) -> Option<Value> {
        self.banner_version()?;
        let mut view = self.view();
        view["kind"] = json!(if view["phase"] == "ready" {
            "restart"
        } else {
            "notice"
        });
        Some(view)
    }

    /// A link the banner may open: the release notes, or the file a
    /// package install downloads. Never a link the page names.
    pub fn banner_link(&self, which: &str) -> Option<String> {
        match (&*self.phase.lock().expect("update lock"), which) {
            (Phase::Unsupported { notice, .. }, "download") => notice.download.clone(),
            (phase, "notes") => phase.release().map(|release| release.url.clone()),
            _ => None,
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

    /// A verified release this copy can't install: the notice, with the
    /// command for how it was installed.
    fn announce(
        &self,
        updater: &Updater,
        release: Release,
        reason: &Unsupported,
        pkg: bool,
        tell: &dyn Fn(),
    ) {
        let installed = crate::update_notice::detect(reason, pkg, updater.managed_update());
        // A local test feed serves its own files; otherwise the release's.
        let files = std::env::var("CWW_UPDATE_FEED")
            .ok()
            .filter(|_| local_feed().is_some())
            .map(|feed| feed.trim_end_matches('/').to_owned())
            .unwrap_or_else(|| {
                format!(
                    "https://github.com/{}/releases/download/v{}",
                    CONFIG.repository, release.version
                )
            });
        let notice = crate::update_notice::notice_from(
            &installed,
            &release.version,
            std::env::consts::ARCH,
            dirs::download_dir().as_deref(),
            &files,
        );
        let release = Release {
            url: notes(&release),
            ..release
        };
        self.set(Phase::Unsupported { release, notice }, tell);
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
        // Pictures of a downloaded update in --demo builds, which can't
        // download a real one (`CWW_DEMO_UPDATE=ready`): the release the
        // feed names, as if downloaded.
        #[cfg(feature = "demo")]
        if std::env::var("CWW_DEMO_UPDATE").as_deref() == Ok("ready")
            && let Ok(Some(release)) = &checked
        {
            let prepared = Prepared::sample(
                fastframe_update::Installation {
                    executable: std::env::current_exe().unwrap_or_default(),
                    kind: fastframe_update::Kind::Portable,
                },
                &release.version,
            );
            return self.set(
                Phase::Ready {
                    release: release.clone(),
                    prepared: Box::new(prepared),
                },
                tell,
            );
        }
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
        // Only a release whose checksums.txt verifies with the key compiled
        // in, and lists a download of that version for this computer, is
        // announced or downloaded.
        let verified = match updater.verify(&release) {
            Ok(verified) if plain_version(&release.version) => verified,
            Ok(_) => {
                log::warn!("not announcing {}: not a plain version", release.version);
                return self.set(Phase::Current, tell);
            }
            Err(e) => {
                log::warn!("not announcing {}: {e:#}", release.version);
                return self.set(
                    Phase::Failed(format!("Couldn't verify version {}.", release.version)),
                    tell,
                );
            }
        };
        let pkg = installed_by_pkg();
        let installable = if pkg {
            Err(Unsupported::NotPortable)
        } else {
            updater.installable(&verified)
        };
        if let Err(reason) = installable {
            log::info!(
                "update {} available; this copy can't install it: {reason:?}",
                release.version
            );
            return self.announce(updater, release, &reason, pkg, tell);
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
            Err(e) if e.downcast_ref::<Unsupported>().is_some() => {
                let reason = e.downcast_ref::<Unsupported>().cloned().expect("checked");
                log::info!(
                    "update {} can't be installed here: {reason:?}",
                    release.version
                );
                self.announce(updater, release, &reason, false, tell);
            }
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

    /// Checks at start and once a day, and whenever
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
                    if asked || !rolled_back {
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

/// Whether the background agent is running a command: nothing installs or
/// restarts it while one runs. An agent that isn't running runs nothing; one
/// that doesn't say may be running one.
pub fn commands_running(client: &crate::control::Client) -> bool {
    match client.request(&cww::control::ControlRequest::Commands) {
        Ok(answer) => answer["commands"]
            .as_array()
            .is_none_or(|commands| commands.iter().any(|run| run["state"] == "running")),
        Err(crate::control::ControlError::NotRunning) => false,
        Err(_) => true,
    }
}

/// Remembers the newest version that ran on this computer, on every start, so
/// going back to an older one can be told from never having had a newer one.
/// Answers what ran before this start.
pub fn remember_run(paths: &crate::paths::Paths) -> Option<String> {
    let before = crate::state::AppState::load(paths).newest_run;
    let newest = fastframe_update::policy::newest_run(VERSION, before.as_deref());
    if before.as_deref() != Some(newest.as_str())
        && let Err(e) =
            crate::state::AppState::update(paths, |state| state.newest_run = Some(newest))
    {
        log::warn!("couldn't remember the newest version: {e}");
    }
    before
}

/// At start, before any window opens: installs the update an earlier run
/// downloaded, when it's newer, installs on its own and the background agent
/// runs no command. Hands
/// it to the helper, which waits for this app to quit, installs it and
/// starts the new version (rolled back if it doesn't start); the caller
/// quits when this answers true.
pub fn install_kept(
    paths: &crate::paths::Paths,
    client: &crate::control::Client,
    window: bool,
    newest_run: Option<&str>,
) -> bool {
    let state = crate::state::AppState::load(paths);
    let updater = updater();
    let kept = match updater.kept() {
        Ok(Some(kept)) => kept,
        Ok(None) => return false,
        Err(e) => {
            log::warn!("the kept update didn't check out: {e:#}");
            return false;
        }
    };
    let installs = fastframe_update::is_newer(kept.version(), VERSION)
        && installs_on_its_own(
            VERSION,
            kept.version(),
            state.installs_updates(),
            newest_run,
        )
        && !commands_running(client);
    if !installs {
        log::info!(
            "update {} waits: not installed at this start",
            kept.version()
        );
        return false;
    }
    log::info!("installing update {} at start", kept.version());
    match updater.handoff(kept, relaunch_arguments(window)) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("couldn't install the kept update: {e:#}");
            false
        }
    }
}

/// Whether `release` installs at the next start without the person pressing
/// anything: automatic updates are on, and it isn't one they went back from.
///
/// The rest of fastframe-update's `policy`, without its rule that a new major
/// version asks. That rule is for apps people pay for by version. Carmine, on
/// 2026-11-06: Chat with Work Desktop is tied to a website, not to anything a
/// person paid for by version, so it shouldn't wait for major versions.
fn installs_on_its_own(
    current: &str,
    release: &str,
    automatic: bool,
    newest_run: Option<&str>,
) -> bool {
    use fastframe_update::is_newer;
    let went_back =
        newest_run.is_some_and(|newest| is_newer(newest, current) && !is_newer(release, newest));
    automatic && !went_back
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
pub fn finish(
    receipt: Receipt,
    agent_version: impl FnOnce() -> Option<String>,
    client: &crate::control::Client,
) {
    if let Err(e) = receipt.acknowledge() {
        log::warn!("couldn't acknowledge the update: {e:#}");
        return;
    }
    log::info!("updated to {VERSION}");
    reconcile_agent(agent_version(), client);
}

/// Package-manager upgrades have no updater receipt. Reconcile those too,
/// without starting an agent the person stopped or restarting the same old
/// executable repeatedly when only one member of the app/CLI pair changed.
pub fn reconcile_agent(running: Option<String>, client: &crate::control::Client) {
    // Never mid-command: an agent running one is restarted once it's done.
    if running.as_deref().is_some_and(|version| version != VERSION) {
        let mut waited = Duration::ZERO;
        while commands_running(client) && waited < Duration::from_secs(24 * 60 * 60) {
            std::thread::sleep(Duration::from_secs(30));
            waited += Duration::from_secs(30);
        }
    }
    let _ = reconcile_agent_with(
        running.as_deref(),
        || {
            let cww = crate::agent::cww_binary()?;
            let output = std::process::Command::new(cww)
                .arg("--version")
                .output()
                .ok()?;
            output.status.success().then(|| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .strip_prefix("cww ")
                    .unwrap_or_default()
                    .to_owned()
            })
        },
        || match crate::agent::start() {
            Ok(_) => {
                log::info!("restarted the background agent on the installed version {VERSION}")
            }
            Err(e) => log::warn!("couldn't restart the background agent: {e:#}"),
        },
    );
}

fn reconcile_agent_with(
    running: Option<&str>,
    installed: impl FnOnce() -> Option<String>,
    restart: impl FnOnce(),
) -> bool {
    // Windows' installer owns its service/task lifecycle.
    if cfg!(windows) || running.is_none() || running == Some(VERSION) {
        return false;
    }
    if installed().as_deref() != Some(VERSION) {
        log::warn!(
            "the installed app and background agent versions differ; finish updating both before restarting the agent"
        );
        return false;
    }
    restart();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn package_updates_restart_only_a_running_outdated_agent_with_a_matching_installed_binary() {
        use std::cell::Cell;
        let restarts = Cell::new(0);
        let restart = || restarts.set(restarts.get() + 1);
        assert!(!reconcile_agent_with(
            None,
            || panic!("stopped agents stay stopped"),
            restart
        ));
        assert!(!reconcile_agent_with(
            Some(VERSION),
            || panic!("current agents need no process"),
            restart
        ));
        assert!(!reconcile_agent_with(Some("0.9.0"), || None, restart));
        assert!(!reconcile_agent_with(
            Some("0.9.0"),
            || Some("0.9.0".into()),
            restart
        ));
        assert!(!reconcile_agent_with(
            Some("0.9.0"),
            || Some("future-version".into()),
            restart
        ));
        assert!(reconcile_agent_with(
            Some("0.9.0"),
            || Some(VERSION.into()),
            restart
        ));
        assert_eq!(restarts.get(), 1);
    }

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
        let updates = Updates::new(Some("restored".into()), None, true);
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

    fn unsupported(version: &str) -> Phase {
        Phase::Unsupported {
            release: Release {
                version: version.into(),
                url: format!(
                    "https://github.com/crmne/chatwithwork-desktop/releases/tag/v{version}"
                ),
            },
            notice: Notice {
                how: "Installed from the AUR. Update it with your AUR helper.".into(),
                command: Some("yay -Syu chatwithwork-desktop-bin".into()),
                download: None,
            },
        }
    }

    #[test]
    fn a_package_managers_release_is_announced_until_dismissed_once_per_version() {
        let updates = Updates::new(None, Some("0.12.0".into()), true);
        *updates.phase.lock().unwrap() = unsupported("0.12.0");
        // Dismissed at an earlier launch.
        assert_eq!(updates.available(), None);
        assert_eq!(updates.view()["dismissed"], true);
        assert_eq!(updates.dismiss("0.12.0"), None);
        // The next release is announced again.
        *updates.phase.lock().unwrap() = unsupported("0.13.0");
        assert_eq!(updates.available().as_deref(), Some("0.13.0"));
        let view = updates.view();
        assert_eq!(view["phase"], "unsupported");
        assert_eq!(view["dismissed"], false);
        assert_eq!(view["release"]["version"], "0.13.0");
        assert_eq!(view["command"], "yay -Syu chatwithwork-desktop-bin");
        assert!(view["download"].is_null());
        assert!(view["advice"].as_str().unwrap().contains("AUR"));
        // Never Restart to Update for it.
        assert!(updates.ready().is_none());
        assert!(updates.restart(Vec::new()).is_err());
        assert_eq!(updates.dismiss("0.12.0"), None);
        assert_eq!(updates.dismiss("0.13.0").as_deref(), Some("0.13.0"));
        assert_eq!(updates.available(), None);
        assert_eq!(updates.view()["dismissed"], true);
    }

    #[test]
    fn the_banner_offers_restart_or_the_notice_until_dismissed() {
        let updates = Updates::new(None, None, true);
        assert!(updates.banner().is_none());
        *updates.phase.lock().unwrap() = unsupported("0.13.0");
        let banner = updates.banner().unwrap();
        assert_eq!(banner["kind"], "notice");
        assert_eq!(banner["command"], "yay -Syu chatwithwork-desktop-bin");
        assert_eq!(
            updates.banner_link("notes").as_deref(),
            Some("https://github.com/crmne/chatwithwork-desktop/releases/tag/v0.13.0")
        );
        assert_eq!(updates.banner_link("download"), None);
        assert_eq!(updates.banner_link("https://example.com"), None);
        assert_eq!(updates.dismiss("0.12.0"), None);
        assert_eq!(updates.dismiss("0.13.0").as_deref(), Some("0.13.0"));
        assert!(updates.banner().is_none());
        assert!(updates.available().is_none());
    }

    fn ready(version: &str) -> Phase {
        Phase::Ready {
            release: Release {
                version: version.into(),
                url: String::new(),
            },
            prepared: Box::new(Prepared::sample(
                fastframe_update::Installation {
                    executable: "/Applications/Chat with Work.app/Contents/MacOS/cww-app".into(),
                    kind: fastframe_update::Kind::MacBundle,
                },
                version,
            )),
        }
    }

    #[test]
    fn a_download_installs_at_the_next_start_unless_turned_off_whatever_its_major() {
        let major: u64 = VERSION.split('.').next().unwrap().parse().unwrap();
        let next = format!("{major}.999.0");
        let updates = Updates::new(None, None, true);
        *updates.phase.lock().unwrap() = ready(&next);
        let view = updates.view();
        assert_eq!(view["phase"], "ready");
        assert_eq!(view["installs_next_start"], true);
        assert_eq!(updates.ready().as_deref(), Some(next.as_str()));
        assert_eq!(updates.banner().unwrap()["kind"], "restart");
        updates.set_installs_updates(false);
        assert_eq!(updates.view()["installs_next_start"], false);
        assert_eq!(updates.view()["installs_updates"], false);
        // A new major version installs too, and waits only with the switch off.
        let updates = Updates::new(None, None, true);
        *updates.phase.lock().unwrap() = ready(&format!("{}.0.0", major + 1));
        assert_eq!(updates.view()["installs_next_start"], true);
        updates.set_installs_updates(false);
        assert_eq!(updates.view()["installs_next_start"], false);
    }

    #[test]
    fn general_does_not_promise_what_a_person_went_back_from() {
        let major: u64 = VERSION.split('.').next().unwrap().parse().unwrap();
        let left = format!("{}.0.0", major + 1);
        let updates = Updates::new(None, None, true).with_newest_run(Some(left.clone()));
        *updates.phase.lock().unwrap() = ready(&left);
        assert_eq!(updates.view()["installs_next_start"], false);
    }

    #[test]
    fn a_person_who_went_back_is_not_updated_past_it_even_to_a_new_major() {
        // Back on 1.0.0 after 2.0.0: up to what they left is offered, not installed.
        assert!(!installs_on_its_own("1.0.0", "1.5.0", true, Some("2.0.0")));
        assert!(!installs_on_its_own("1.0.0", "2.0.0", true, Some("2.0.0")));
        // Anything newer than what they left installs again.
        assert!(installs_on_its_own("1.0.0", "2.0.1", true, Some("2.0.0")));
        assert!(installs_on_its_own("1.0.0", "3.0.0", true, Some("2.0.0")));
        // Never gone back.
        assert!(installs_on_its_own("1.0.0", "2.0.0", true, Some("1.0.0")));
        assert!(installs_on_its_own("1.0.0", "2.0.0", true, None));
        // The switch off waits for Restart to Update.
        assert!(!installs_on_its_own("1.0.0", "2.0.0", false, None));
    }

    #[test]
    fn a_downloaded_update_is_still_restart_to_update() {
        // macOS and Windows: what the tray and General offer is unchanged.
        let updates = Updates::new(None, None, true);
        *updates.phase.lock().unwrap() = Phase::Installing {
            release: Release {
                version: "0.12.0".into(),
                url: String::new(),
            },
        };
        assert_eq!(updates.available(), None);
        assert!(updates.view()["dismissed"].is_null());
        assert_eq!(updates.dismiss("0.12.0"), None);
    }

    #[test]
    fn only_plain_versions_reach_a_command() {
        assert!(plain_version("0.12.0"));
        assert!(plain_version("10.0.123"));
        for bad in [
            "0.12",
            "0.12.0-beta.1",
            "0.12.0;rm",
            "v0.12.0",
            "",
            "0..1",
            "1.2.3.4",
        ] {
            assert!(!plain_version(bad), "{bad}");
        }
    }
}
