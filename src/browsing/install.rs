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
//! This runs in the `cww` command or the desktop app, never in the
//! confined daemon, which can't write there.

use std::path::{Path, PathBuf};

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
        name: "Brave",
        linux: Some("BraveSoftware/Brave-Browser"),
        macos: Some("BraveSoftware/Brave-Browser"),
        windows: Some(r"Software\BraveSoftware\Brave-Browser"),
    },
    Browser {
        name: "Vivaldi",
        linux: Some("vivaldi"),
        macos: Some("Vivaldi"),
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
    let text = serde_json::to_string_pretty(&manifest(exe, origins))?;
    let mut done = Vec::new();
    #[cfg(not(windows))]
    {
        let _ = paths;
        for (browser, file) in manifest_paths(&config_root()?, cfg!(target_os = "macos")) {
            let dir = file.parent().context("manifest folder")?;
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            std::fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
            done.push(Registered {
                browser,
                location: file.display().to_string(),
            });
        }
    }
    #[cfg(windows)]
    {
        let file = paths.data_dir.join(format!("{HOST_NAME}.json"));
        std::fs::create_dir_all(&paths.data_dir)?;
        std::fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
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
    }
    Ok(done)
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
