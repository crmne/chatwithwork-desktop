//! "Start at login" for the app itself, through fastframe-autostart: an XDG
//! autostart entry on Linux, a login item on macOS, and the Run key on
//! Windows. The daemon has its own service (`cww daemon install`), which
//! always starts at login.
//!
//! The app starts with `--background`, so it only shows its tray item. On
//! macOS that is the launch agent the bundle carries
//! (`packaging/macos/autostart.plist`), since a login item cannot pass
//! arguments.
//!
//! Every call blocks on files, the registry or `SMAppService`, so the
//! settings window runs them on a worker.

use anyhow::{Context, Result};
use fastframe_autostart::{Autostart, Start};

/// The argument a login start passes.
pub const BACKGROUND: &str = "--background";

/// The app's registration. The Linux entry keeps the path
/// (`autostart/cww-app.desktop`) and name earlier versions wrote, so
/// existing registrations read back.
fn autostart() -> Autostart {
    let name = if cfg!(windows) {
        "ChatWithWorkDesktop"
    } else {
        "Chat with Work"
    };
    Autostart::new("cww-app", name).hidden_argument(BACKGROUND)
}

pub fn is_enabled() -> bool {
    #[cfg(target_os = "macos")]
    if legacy::exists() {
        return true;
    }
    autostart().state().start().is_some()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let autostart = autostart();
    #[cfg(target_os = "macos")]
    if autostart.state() == fastframe_autostart::State::Unsupported {
        // macOS 11 and 12 have no SMAppService, and a run outside the
        // bundle has no login item: the LaunchAgent earlier versions wrote.
        return if enabled {
            legacy::write()
        } else {
            legacy::remove()
        };
    }
    let _state = autostart
        .set(enabled.then_some(Start::Hidden))
        .context("changing what starts at login")?;
    // Once the bundled agent is approved, the old LaunchAgent would start
    // the app a second time.
    #[cfg(target_os = "macos")]
    if !enabled || _state == fastframe_autostart::State::On(Start::Hidden) {
        legacy::remove()?;
    }
    Ok(())
}

/// Moves a LaunchAgent that earlier versions wrote to
/// `~/Library/LaunchAgents` onto the bundled agent, so the app keeps
/// starting at login, once. The old file goes only once the new
/// registration is approved; until then it keeps starting the app. The
/// loaded job is left alone: it may be this very process, and without the
/// file it is not loaded at the next login. Blocks; call it on a worker.
#[cfg(target_os = "macos")]
pub fn migrate() {
    use fastframe_autostart::State;
    if !legacy::exists() {
        return;
    }
    let autostart = autostart();
    if autostart.state() == State::Unsupported {
        return;
    }
    match autostart.set(Some(Start::Hidden)) {
        Ok(State::On(Start::Hidden)) => match legacy::remove() {
            Ok(()) => log::info!("moved start at login to the bundled agent"),
            Err(e) => log::warn!("{e:#}"),
        },
        Ok(state) => log::info!("keeping the old login agent while the new one is {state:?}"),
        Err(e) => log::warn!("keeping the old login agent: {e}"),
    }
}

#[cfg(target_os = "macos")]
mod legacy {
    //! The LaunchAgent in `~/Library/LaunchAgents` that versions before
    //! fastframe-autostart wrote, still used where `SMAppService` is not.

    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};

    const LABEL: &str = "com.chatwithwork.cww-app";

    fn path() -> Option<PathBuf> {
        crate::paths::home_dir().map(|h| h.join(format!("Library/LaunchAgents/{LABEL}.plist")))
    }

    pub fn exists() -> bool {
        path().is_some_and(|p| p.exists())
    }

    pub fn write() -> Result<()> {
        let path = path().context("no home folder")?;
        let exe = std::env::current_exe().context("finding the app")?;
        let exe = exe.canonicalize().unwrap_or(exe);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&path, plist(&exe)).with_context(|| format!("writing {}", path.display()))
    }

    pub fn remove() -> Result<()> {
        let Some(path) = path() else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", path.display()))
            }
            _ => Ok(()),
        }
    }

    pub(super) fn plist(exe: &Path) -> String {
        let escape = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>{background}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
            exe = escape(&exe.to_string_lossy()),
            background = super::BACKGROUND,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle's launch agent starts the app the way `--background`
    /// expects, under the bundle identifier in `Info.plist`.
    #[test]
    fn the_bundled_agent_starts_the_app_in_the_background() {
        let bundled = include_str!("../../packaging/macos/autostart.plist");
        assert_eq!(
            bundled.replace("\r\n", "\n"),
            fastframe_autostart::macos_agent_plist("com.chatwithwork.desktop", BACKGROUND),
            "regenerate packaging/macos/autostart.plist"
        );
        assert!(
            include_str!("../../packaging/macos/Info.plist")
                .contains("<string>com.chatwithwork.desktop</string>")
        );
        let bundle = include_str!("../../packaging/macos/bundle.sh");
        assert!(bundle.contains(&format!(
            "Contents/Library/LaunchAgents/{}",
            fastframe_autostart::macos_agent_plist_name("com.chatwithwork.desktop")
        )));
    }

    #[test]
    fn a_login_start_opens_no_window() {
        let Ok(crate::args::Parsed::Run(login)) =
            crate::args::parse([BACKGROUND.to_string()].into_iter())
        else {
            panic!("a login start parses");
        };
        assert!(login.background);
        assert_eq!(login.open, crate::args::Open::Nothing);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_old_launch_agent_runs_the_app_in_the_background() {
        let text = legacy::plist(std::path::Path::new(
            "/Applications/A & B.app/Contents/MacOS/cww-app",
        ));
        assert!(
            text.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/cww-app</string>")
        );
        assert!(text.contains("<string>--background</string>"));
    }
}
