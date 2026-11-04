//! The menu bar item (macOS), notification-area icon (Windows) or tray item
//! (Linux, through the AppIndicator library), with the platform's own menu:
//! the daemon's state as a greyed-out line (and a second one while there is
//! more to say: indexing, or the last access), then Open Chat with Work,
//! Scan…, Pause Sharing, Stop Using the Browser while the browser is on,
//! Settings and Quit. Settings opens this computer's part of Chat with
//! Work's own Settings once the computer is paired and the Chat with Work
//! window is signed in, else the app's own settings window.

use tauri::AppHandle;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::model::{Platform, TrayView};

mod ids {
    pub const HEADLINE: &str = "headline";
    pub const DETAIL: &str = "detail";
    pub const OPEN: &str = "open";
    pub const PAUSE: &str = "pause";
    pub const BROWSER: &str = "browser";
    pub const SETTINGS: &str = "settings";
    pub const SCAN: &str = "scan";
    pub const QUIT: &str = "quit";
    pub const UPDATE: &str = "update";
    pub const UPDATE_AVAILABLE: &str = "update-available";
}

/// The first line, as an action, while this computer has to sign in again.
pub const RECONNECT_LABEL: &str = "Reconnect…";

/// The entry while an update waits for a restart.
pub fn update_label(version: &str) -> String {
    format!("Restart to Update to {version}")
}

/// The entry while a newer release waits for a package manager: it opens
/// General, which says how to update this copy.
pub fn available_label(version: &str) -> String {
    format!("Update to {version}…")
}

/// What the update entry above Quit offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEntry {
    /// Restart to Update: the app installs it.
    Restart(String),
    /// A package manager installs it: shows how.
    Available(String),
}

impl UpdateEntry {
    fn id(&self) -> &'static str {
        match self {
            Self::Restart(_) => ids::UPDATE,
            Self::Available(_) => ids::UPDATE_AVAILABLE,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Restart(version) => update_label(version),
            Self::Available(version) => available_label(version),
        }
    }
}

pub const SCAN_LABEL: &str = "Scan…";

pub const OPEN_LABEL: &str = "Open Chat with Work";

/// The label of the pause entry.
pub fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Resume Sharing"
    } else {
        "Pause Sharing"
    }
}

/// The label of the browser's kill switch.
pub fn browser_label(stopped: bool) -> &'static str {
    if stopped {
        "Resume Using the Browser"
    } else {
        "Stop Using the Browser"
    }
}

/// The tooltip. Linux panels show the detail line under the state; Windows
/// and macOS show the state alone.
pub fn tooltip(view: &TrayView, platform: Platform) -> String {
    match (&view.detail, platform) {
        (Some(detail), Platform::Linux) => format!("{}\n{detail}", view.tooltip),
        _ => view.tooltip.clone(),
    }
}

/// What a menu entry asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reconnect,
    Open,
    TogglePause,
    ToggleBrowser,
    Settings,
    Scan,
    RestartToUpdate,
    ShowUpdate,
    Quit,
}

pub fn action(id: &str) -> Option<Action> {
    Some(match id {
        ids::HEADLINE => Action::Reconnect,
        ids::OPEN => Action::Open,
        ids::PAUSE => Action::TogglePause,
        ids::BROWSER => Action::ToggleBrowser,
        ids::SETTINGS => Action::Settings,
        ids::SCAN => Action::Scan,
        ids::UPDATE => Action::RestartToUpdate,
        ids::UPDATE_AVAILABLE => Action::ShowUpdate,
        ids::QUIT => Action::Quit,
        _ => return None,
    })
}

pub struct Tray {
    icon: TrayIcon,
    menu: Menu<tauri::Wry>,
    headline: MenuItem<tauri::Wry>,
    /// In the menu only while there is a detail: Tauri's menus can't hide an
    /// item, and an empty one is a blank line.
    detail: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    /// In the menu only while the browser is on, after Pause Sharing.
    browser: MenuItem<tauri::Wry>,
    /// Restart to Update, or Update to X.Y.Z…, just above Quit while there
    /// is one.
    update: Option<(UpdateEntry, MenuItem<tauri::Wry>)>,
    view: TrayView,
}

/// Where the browser's entry goes: after Pause Sharing, which follows the
/// headline, the detail while there is one, a separator, Open and Scan….
fn browser_position(detail_shown: bool) -> usize {
    5 + usize::from(detail_shown)
}

const COLOR: &[u8] = include_bytes!("../assets/tray-64.png");
const TEMPLATE: &[u8] = include_bytes!("../assets/tray-template-36.png");

/// The icon, faded while nothing is being served. macOS gets the template
/// image it tints to match the menu bar.
fn icon(dimmed: bool) -> Image<'static> {
    let bytes = if cfg!(target_os = "macos") {
        TEMPLATE
    } else {
        COLOR
    };
    let image = Image::from_bytes(bytes).expect("the tray icon is a PNG");
    if !dimmed {
        return image.to_owned();
    }
    let mut rgba = image.rgba().to_vec();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[3] = (pixel[3] as u16 * 2 / 5) as u8;
    }
    Image::new_owned(rgba, image.width(), image.height())
}

impl Tray {
    pub fn new(app: &AppHandle, view: &TrayView) -> tauri::Result<Self> {
        let platform = Platform::current();
        let reconnects = view.reconnects;
        let headline = MenuItem::with_id(
            app,
            ids::HEADLINE,
            if reconnects {
                RECONNECT_LABEL
            } else {
                &view.headline
            },
            reconnects,
            None::<&str>,
        )?;
        let detail = MenuItem::with_id(
            app,
            ids::DETAIL,
            view.detail.clone().unwrap_or_default(),
            false,
            None::<&str>,
        )?;
        let open = MenuItem::with_id(app, ids::OPEN, OPEN_LABEL, true, None::<&str>)?;
        let pause = MenuItem::with_id(
            app,
            ids::PAUSE,
            pause_label(view.paused == Some(true)),
            view.paused.is_some(),
            None::<&str>,
        )?;
        let browser = MenuItem::with_id(
            app,
            ids::BROWSER,
            browser_label(view.browser_stopped == Some(true)),
            true,
            None::<&str>,
        )?;
        let settings = MenuItem::with_id(
            app,
            ids::SETTINGS,
            platform.settings_item(),
            true,
            None::<&str>,
        )?;
        let scan = MenuItem::with_id(app, ids::SCAN, SCAN_LABEL, true, None::<&str>)?;
        let quit = MenuItem::with_id(app, ids::QUIT, platform.quit_item(), true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &headline,
                &PredefinedMenuItem::separator(app)?,
                &open,
                &scan,
                &pause,
                &PredefinedMenuItem::separator(app)?,
                &settings,
                &quit,
            ],
        )?;
        if view.detail.is_some() {
            menu.insert(&detail, 1)?;
        }
        if view.browser_stopped.is_some() {
            menu.insert(&browser, browser_position(view.detail.is_some()))?;
        }
        let icon = TrayIconBuilder::with_id("cww-app")
            .icon(icon(view.dimmed))
            .icon_as_template(cfg!(target_os = "macos"))
            .tooltip(tooltip(view, platform))
            .menu(&menu)
            // macOS opens the menu on any click; Windows opens the app on a
            // left click and the menu on a right one. Linux trays always
            // show the menu.
            .show_menu_on_left_click(cfg!(target_os = "macos"))
            .on_menu_event(|app, event| crate::desktop::tray_action(app, event.id().as_ref()))
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    // On macOS the same click opens the menu.
                    if !cfg!(target_os = "macos") {
                        crate::desktop::tray_action(tray.app_handle(), ids::OPEN);
                    }
                }
            })
            .build(app)?;
        Ok(Self {
            icon,
            menu,
            headline,
            detail,
            pause,
            browser,
            update: None,
            view: view.clone(),
        })
    }

    /// Offers `entry` above Quit, or takes the entry away.
    pub fn set_update(&mut self, app: &AppHandle, entry: Option<UpdateEntry>) {
        if self.update.as_ref().map(|(e, _)| e) == entry.as_ref() {
            return;
        }
        if let Some((_, item)) = self.update.take() {
            let _ = self.menu.remove(&item);
        }
        let Some(entry) = entry else {
            return;
        };
        let Ok(item) = MenuItem::with_id(app, entry.id(), entry.label(), true, None::<&str>) else {
            return;
        };
        // Just above Quit, the last entry.
        let position = self
            .menu
            .items()
            .map_or(0, |items| items.len().saturating_sub(1));
        if self.menu.insert(&item, position).is_ok() {
            self.update = Some((entry, item));
        }
    }

    pub fn update(&mut self, view: &TrayView) {
        if *view == self.view {
            return;
        }
        let platform = Platform::current();
        let old = std::mem::replace(&mut self.view, view.clone());
        let reconnects = view.reconnects;
        if view.headline != old.headline || reconnects != old.reconnects {
            let _ = self.headline.set_text(if reconnects {
                RECONNECT_LABEL
            } else {
                &view.headline
            });
            let _ = self.headline.set_enabled(reconnects);
        }
        // A missing detail leaves the menu, rather than showing as an
        // empty line.
        if view.detail != old.detail {
            if let Some(detail) = &view.detail {
                let _ = self.detail.set_text(detail);
            }
            match (old.detail.is_some(), view.detail.is_some()) {
                (false, true) => {
                    let _ = self.menu.insert(&self.detail, 1);
                }
                (true, false) => {
                    let _ = self.menu.remove(&self.detail);
                }
                _ => {}
            }
        }
        if view.paused != old.paused {
            let _ = self.pause.set_text(pause_label(view.paused == Some(true)));
            let _ = self.pause.set_enabled(view.paused.is_some());
        }
        // The browser's entry shows only while the browser is on.
        if view.browser_stopped != old.browser_stopped {
            if let Some(stopped) = view.browser_stopped {
                let _ = self.browser.set_text(browser_label(stopped));
            }
            match (
                old.browser_stopped.is_some(),
                view.browser_stopped.is_some(),
            ) {
                (false, true) => {
                    let position = browser_position(view.detail.is_some());
                    let _ = self.menu.insert(&self.browser, position);
                }
                (true, false) => {
                    let _ = self.menu.remove(&self.browser);
                }
                _ => {}
            }
        }
        if tooltip(view, platform) != tooltip(&old, platform) {
            let _ = self.icon.set_tooltip(Some(tooltip(view, platform)));
        }
        if view.dimmed != old.dimmed {
            let _ = self.icon.set_icon(Some(icon(view.dimmed)));
            let _ = self.icon.set_icon_as_template(cfg!(target_os = "macos"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Health;

    fn view(detail: Option<&str>) -> TrayView {
        TrayView {
            health: Health::Online,
            headline: "Sharing 2 folders".into(),
            detail: detail.map(Into::into),
            paused: Some(false),
            tooltip: "Chat with Work Desktop: Online".into(),
            dimmed: false,
            browser_stopped: None,
            reconnects: false,
        }
    }

    #[test]
    fn entries_do_what_they_say() {
        assert_eq!(action("open"), Some(Action::Open));
        assert_eq!(action("pause"), Some(Action::TogglePause));
        assert_eq!(action("settings"), Some(Action::Settings));
        assert_eq!(action("scan"), Some(Action::Scan));
        assert_eq!(action("quit"), Some(Action::Quit));
        assert_eq!(action("update"), Some(Action::RestartToUpdate));
        assert_eq!(update_label("0.7.0"), "Restart to Update to 0.7.0");
        assert_eq!(action("update-available"), Some(Action::ShowUpdate));
        assert_eq!(available_label("0.7.0"), "Update to 0.7.0…");
        let available = UpdateEntry::Available("0.7.0".into());
        assert_eq!(action(available.id()), Some(Action::ShowUpdate));
        assert_eq!(available.label(), "Update to 0.7.0…");
        let restart = UpdateEntry::Restart("0.7.0".into());
        assert_eq!(action(restart.id()), Some(Action::RestartToUpdate));
        assert_eq!(restart.label(), "Restart to Update to 0.7.0");
        assert_eq!(action("headline"), Some(Action::Reconnect));
        assert_eq!(action("detail"), None);
        assert_eq!(pause_label(true), "Resume Sharing");
        assert_eq!(pause_label(false), "Pause Sharing");
        assert_eq!(action("browser"), Some(Action::ToggleBrowser));
        assert_eq!(browser_label(false), "Stop Using the Browser");
        assert_eq!(browser_label(true), "Resume Using the Browser");
        assert_eq!(browser_position(false), 5);
        assert_eq!(browser_position(true), 6);
    }

    #[test]
    fn linux_tooltips_carry_the_detail() {
        let indexing = view(Some("Indexing 1 folder…"));
        assert_eq!(
            tooltip(&indexing, Platform::Linux),
            "Chat with Work Desktop: Online\nIndexing 1 folder…"
        );
        assert_eq!(
            tooltip(&indexing, Platform::Windows),
            "Chat with Work Desktop: Online"
        );
        assert_eq!(
            tooltip(&view(None), Platform::Linux),
            "Chat with Work Desktop: Online"
        );
    }

    #[test]
    fn the_icon_fades_while_nothing_is_served() {
        let bright = icon(false);
        let faded = icon(true);
        assert_eq!(bright.width(), faded.width());
        assert_ne!(bright.rgba(), faded.rgba());
    }
}
