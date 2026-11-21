//! Registering `cww` as the extension's native messaging host, for the
//! current user, in every Chromium-family browser it knows:
//!
//! - Linux and macOS: `com.chatwithwork.desktop.json` in each browser's
//!   `NativeMessagingHosts` folder (created only where the browser's own
//!   folder exists).
//! - Windows: the manifest in cww's data folder, and its path as the
//!   default value of `HKCU\Software\<browser>\NativeMessagingHosts\
//!   com.chatwithwork.desktop` for each browser.
//!
//! The manifest allows only the extension IDs `bridge::origins` names.
//! This runs in the `cww` command, the desktop app, and the daemon's
//! unconfined side (its supervisor, or the daemon itself where it runs
//! without a sandbox), which keeps the registration while the browser is
//! on (`keep_registered`), never in the confined worker, which can't write
//! there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;

use crate::paths::Paths;

pub const HOST_NAME: &str = "com.chatwithwork.desktop";

/// A browser's per-user configuration folder, under the user's
/// configuration root (`~/.config` on Linux, `~/Library/Application
/// Support` on macOS), and its registry key on Windows.
pub struct Browser {
    pub name: &'static str,
    pub linux: Option<&'static str>,
    pub macos: Option<&'static str>,
    pub windows: Option<&'static str>,
}

pub const BROWSERS: &[Browser] = &[
    Browser {
        name: "Google Chrome",
        linux: Some("google-chrome"),
        macos: Some("Google/Chrome"),
        windows: Some(r"Software\Google\Chrome"),
    },
    Browser {
        name: "Google Chrome Beta",
        linux: Some("google-chrome-beta"),
        macos: Some("Google/Chrome Beta"),
        windows: None,
    },
    Browser {
        name: "Google Chrome Dev",
        linux: Some("google-chrome-unstable"),
        macos: Some("Google/Chrome Dev"),
        windows: None,
    },
    Browser {
        name: "Google Chrome Canary",
        linux: None,
        macos: Some("Google/Chrome Canary"),
        windows: None,
    },
    Browser {
        name: "Chromium",
        linux: Some("chromium"),
        macos: Some("Chromium"),
        windows: Some(r"Software\Chromium"),
    },
    Browser {
        name: "Microsoft Edge",
        linux: Some("microsoft-edge"),
        macos: Some("Microsoft Edge"),
        windows: Some(r"Software\Microsoft\Edge"),
    },
    Browser {
        name: "Microsoft Edge Beta",
        linux: None,
        macos: Some("Microsoft Edge Beta"),
        windows: None,
    },
    Browser {
        name: "Microsoft Edge Dev",
        linux: None,
        macos: Some("Microsoft Edge Dev"),
        windows: None,
    },
    Browser {
        name: "Microsoft Edge Canary",
        linux: None,
        macos: Some("Microsoft Edge Canary"),
        windows: None,
    },
    Browser {
        name: "Brave",
        linux: Some("BraveSoftware/Brave-Browser"),
        macos: Some("BraveSoftware/Brave-Browser"),
        windows: Some(r"Software\BraveSoftware\Brave-Browser"),
    },
    Browser {
        name: "Brave Beta",
        linux: None,
        macos: Some("BraveSoftware/Brave-Browser-Beta"),
        windows: None,
    },
    Browser {
        name: "Brave Nightly",
        linux: None,
        macos: Some("BraveSoftware/Brave-Browser-Nightly"),
        windows: None,
    },
    // On Windows, Vivaldi reads Google Chrome's key, which is registered.
    Browser {
        name: "Vivaldi",
        linux: Some("vivaldi"),
        macos: Some("Vivaldi"),
        windows: None,
    },
    // Arc keeps its profiles one folder down, in "User Data".
    Browser {
        name: "Arc",
        linux: None,
        macos: Some("Arc/User Data"),
        windows: None,
    },
    Browser {
        name: "Helium",
        linux: Some("net.imput.helium"),
        macos: Some("net.imput.helium"),
        windows: None,
    },
];

/// The host manifest naming `exe` and allowing `origins`.
pub fn manifest(exe: &Path, origins: &[String]) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "Chat with Work Desktop",
        "path": exe,
        "type": "stdio",
        "allowed_origins": origins,
    })
}

/// Where each browser found on this computer looks for the manifest.
/// `config_root` is the user's configuration root.
pub fn manifest_paths(config_root: &Path, macos: bool) -> Vec<(&'static str, PathBuf)> {
    BROWSERS
        .iter()
        .filter_map(|browser| {
            let dir = if macos { browser.macos } else { browser.linux }?;
            let profile = config_root.join(dir);
            profile.is_dir().then(|| {
                (
                    browser.name,
                    profile
                        .join("NativeMessagingHosts")
                        .join(format!("{HOST_NAME}.json")),
                )
            })
        })
        .collect()
}

/// What `register` did, per browser.
#[derive(Debug, serde::Serialize)]
pub struct Registered {
    pub browser: &'static str,
    pub location: String,
}

/// Register the host for every browser found, pointing at `exe` (the
/// stable path of this `cww`, or `cww-agent.exe` on Windows, which opens
/// no console window).
pub fn register(paths: &Paths, exe: &Path, origins: &[String]) -> Result<Vec<Registered>> {
    write(paths, exe, origins, false)
}

/// Like `register`, but writes only what is missing or different, and
/// says only what it wrote: a browser installed or first opened since the
/// last time, or a manifest that named another `cww`.
pub fn refresh(paths: &Paths, exe: &Path, origins: &[String]) -> Result<Vec<Registered>> {
    write(paths, exe, origins, true)
}

fn write(
    paths: &Paths,
    exe: &Path,
    origins: &[String],
    only_changed: bool,
) -> Result<Vec<Registered>> {
    let text = serde_json::to_string_pretty(&manifest(exe, origins))?;
    #[cfg(not(windows))]
    {
        let _ = paths;
        write_in(
            &config_root()?,
            cfg!(target_os = "macos"),
            &text,
            only_changed,
        )
    }
    #[cfg(windows)]
    {
        let file = paths.data_dir.join(format!("{HOST_NAME}.json"));
        // The keys name the file, so they only change with its folder.
        if only_changed && std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
            return Ok(Vec::new());
        }
        // The keys first, the file last: a key that fails leaves the file
        // as it was, so the next refresh tries every key again.
        let mut done = Vec::new();
        std::fs::create_dir_all(&paths.data_dir)?;
        for browser in BROWSERS {
            let Some(key) = browser.windows else { continue };
            let key = format!(r"{key}\NativeMessagingHosts\{HOST_NAME}");
            registry::set_default(&key, &file.display().to_string())
                .with_context(|| format!("registering with {}", browser.name))?;
            done.push(Registered {
                browser: browser.name,
                location: format!(r"HKCU\{key}"),
            });
        }
        std::fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
        Ok(done)
    }
}

/// Write `text` as the manifest of every browser found under
/// `config_root`, skipping those that already have it when `only_changed`.
#[cfg_attr(windows, allow(dead_code))]
fn write_in(
    config_root: &Path,
    macos: bool,
    text: &str,
    only_changed: bool,
) -> Result<Vec<Registered>> {
    let mut done = Vec::new();
    for (browser, file) in manifest_paths(config_root, macos) {
        if only_changed && std::fs::read_to_string(&file).ok().as_deref() == Some(text) {
            continue;
        }
        let dir = file.parent().context("manifest folder")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))?;
        done.push(Registered {
            browser,
            location: file.display().to_string(),
        });
    }
    Ok(done)
}

/// The browsers found on this computer that have our host registered, and
/// those found without it, by name. On Windows every key is written at
/// once, so it is all or nothing.
pub fn registrations(paths: &Paths) -> (Vec<&'static str>, Vec<&'static str>) {
    #[cfg(not(windows))]
    {
        let _ = paths;
        match config_root() {
            Ok(root) => registrations_in(&root, cfg!(target_os = "macos")),
            Err(_) => (Vec::new(), Vec::new()),
        }
    }
    #[cfg(windows)]
    {
        let names = BROWSERS
            .iter()
            .filter(|b| b.windows.is_some())
            .map(|b| b.name)
            .collect();
        if paths.data_dir.join(format!("{HOST_NAME}.json")).is_file() {
            (names, Vec::new())
        } else {
            (Vec::new(), names)
        }
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn registrations_in(config_root: &Path, macos: bool) -> (Vec<&'static str>, Vec<&'static str>) {
    let ours = |file: &Path| {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|value| value["name"] == HOST_NAME)
    };
    let (with, without): (Vec<_>, Vec<_>) = manifest_paths(config_root, macos)
        .into_iter()
        .partition(|(_, file)| ours(file));
    (
        with.into_iter().map(|(name, _)| name).collect(),
        without.into_iter().map(|(name, _)| name).collect(),
    )
}

/// The program the browsers start: the stable path of this `cww`, or on
/// Windows `cww-agent.exe` beside it, which opens no console window.
pub fn host_exe() -> Result<PathBuf> {
    let exe = crate::service::stable_exe()?;
    if cfg!(windows) {
        let agent = exe.with_file_name("cww-agent.exe");
        if agent.is_file() {
            return Ok(agent);
        }
    }
    Ok(exe)
}

/// How often the daemon looks for a browser installed or first opened
/// since it last registered. The extension tries to connect every 30
/// seconds, so a new browser connects within about a minute and a half.
pub const KEEP_REGISTERED_EVERY: Duration = Duration::from_secs(60);

/// Whether this daemon keeps the registration: only one that uses the
/// person's own settings. A `CWW_HOME` daemon (the tests, a second copy)
/// must never point the person's browsers at itself.
pub fn keeps_registration(cww_home: Option<&std::ffi::OsStr>) -> bool {
    cww_home.is_none_or(|home| home.is_empty())
}

/// Run on a thread of the daemon's unconfined side: while the browser is
/// on, register with every browser that lacks it, now and every
/// `KEEP_REGISTERED_EVERY`. When it sees the browser turned off, it
/// removes the registrations, as `cww browser disable` does, in case a
/// pass wrote one after that command removed them.
pub fn keep_registered(paths: Paths) {
    let mut failing = false;
    let mut was_on = false;
    loop {
        match pass(&paths, &mut was_on) {
            Ok(done) => {
                failing = false;
                for registered in done {
                    tracing::info!(browser = registered.browser, "registered with a browser");
                }
            }
            Err(e) => {
                if !failing {
                    tracing::warn!("couldn't keep the browsers' registration: {e:#}");
                }
                failing = true;
            }
        }
        std::thread::sleep(KEEP_REGISTERED_EVERY);
    }
}

fn pass(paths: &Paths, was_on: &mut bool) -> Result<Vec<Registered>> {
    let on = crate::config::Config::load(paths)?.browser.enabled;
    if turned_off(was_on, on) {
        unregister(paths)?;
    }
    refresh_if_on(paths)
}

/// Whether the browser was on at the last pass and is off now.
fn turned_off(was_on: &mut bool, on: bool) -> bool {
    std::mem::replace(was_on, on) && !on
}

fn refresh_if_on(paths: &Paths) -> Result<Vec<Registered>> {
    let config = crate::config::Config::load(paths)?;
    if !config.browser.enabled {
        return Ok(Vec::new());
    }
    refresh(
        paths,
        &host_exe()?,
        &crate::browsing::bridge::origins(&config.browser.extension_ids),
    )
}

/// Remove every registration `register` made.
pub fn unregister(paths: &Paths) -> Result<()> {
    #[cfg(not(windows))]
    {
        let _ = paths;
        for (_, file) in manifest_paths(&config_root()?, cfg!(target_os = "macos")) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", file.display())),
            }
        }
    }
    #[cfg(windows)]
    {
        for browser in BROWSERS {
            let Some(key) = browser.windows else { continue };
            registry::remove(&format!(r"{key}\NativeMessagingHosts\{HOST_NAME}"));
        }
        let _ = std::fs::remove_file(paths.data_dir.join(format!("{HOST_NAME}.json")));
    }
    Ok(())
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteTreeW, RegSetKeyValueW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Set the default value of `HKCU\<key>`, creating the key.
    pub fn set_default(key: &str, value: &str) -> anyhow::Result<()> {
        let key = wide(key);
        let value = wide(value);
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the
        // call, and the size given is the value's, terminator included.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        anyhow::ensure!(
            status == 0,
            "the registry refused the change (error {status})"
        );
        Ok(())
    }

    pub fn remove(key: &str) {
        let key = wide(key);
        // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr());
        }
    }
}

#[cfg(not(windows))]
fn config_root() -> Result<PathBuf> {
    let home = crate::paths::home_dir()?;
    Ok(if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_the_browsers_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("google-chrome")).unwrap();
        std::fs::create_dir_all(tmp.path().join("BraveSoftware/Brave-Browser")).unwrap();
        let found = manifest_paths(tmp.path(), false);
        let names: Vec<_> = found.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["Google Chrome", "Brave"]);
        assert_eq!(
            found[0].1,
            tmp.path()
                .join("google-chrome/NativeMessagingHosts/com.chatwithwork.desktop.json")
        );
    }

    #[test]
    fn finds_arc_and_helium_where_they_keep_their_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Arc/User Data")).unwrap();
        std::fs::create_dir_all(tmp.path().join("net.imput.helium")).unwrap();
        let mac: Vec<_> = manifest_paths(tmp.path(), true)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(mac, ["Arc", "Helium"]);
        let linux: Vec<_> = manifest_paths(tmp.path(), false)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(linux, ["Helium"]);
        assert_eq!(
            manifest_paths(tmp.path(), true)[0].1,
            tmp.path()
                .join("Arc/User Data/NativeMessagingHosts/com.chatwithwork.desktop.json")
        );
    }

    #[test]
    fn refreshing_writes_only_browsers_that_lack_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("chromium")).unwrap();
        let text = "{\"name\": \"com.chatwithwork.desktop\"}";
        let first = write_in(tmp.path(), false, text, true).unwrap();
        assert_eq!(
            first.iter().map(|r| r.browser).collect::<Vec<_>>(),
            ["Chromium"]
        );
        assert!(write_in(tmp.path(), false, text, true).unwrap().is_empty());
        // A browser first opened later gets it on the next pass.
        std::fs::create_dir_all(tmp.path().join("google-chrome")).unwrap();
        assert_eq!(
            registrations_in(tmp.path(), false),
            (vec!["Chromium"], vec!["Google Chrome"])
        );
        let later = write_in(tmp.path(), false, text, true).unwrap();
        assert_eq!(
            later.iter().map(|r| r.browser).collect::<Vec<_>>(),
            ["Google Chrome"]
        );
        // One that names another cww is pointed back at this one.
        let changed = write_in(
            tmp.path(),
            false,
            "{\"name\": \"com.chatwithwork.desktop\", \"path\": \"/new\"}",
            true,
        )
        .unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(
            registrations_in(tmp.path(), false).0,
            ["Google Chrome", "Chromium"]
        );
    }

    #[test]
    fn removes_the_registrations_once_when_it_sees_the_browser_turned_off() {
        let mut was_on = false;
        assert!(!turned_off(&mut was_on, false), "never on: nothing to undo");
        assert!(!turned_off(&mut was_on, true));
        assert!(!turned_off(&mut was_on, true));
        assert!(turned_off(&mut was_on, false));
        assert!(!turned_off(&mut was_on, false), "only once");
    }

    #[test]
    fn only_a_daemon_with_the_persons_own_settings_registers() {
        assert!(keeps_registration(None));
        assert!(keeps_registration(Some(std::ffi::OsStr::new(""))));
        assert!(!keeps_registration(Some(std::ffi::OsStr::new(
            "/tmp/cww-e2e"
        ))));
    }

    #[test]
    fn the_manifest_names_the_host_and_its_extensions() {
        let value = manifest(
            Path::new("/usr/bin/cww"),
            &["chrome-extension://abc/".into()],
        );
        assert_eq!(value["name"], HOST_NAME);
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], "/usr/bin/cww");
        assert_eq!(value["allowed_origins"][0], "chrome-extension://abc/");
    }
}
