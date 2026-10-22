//! The menu bar item (macOS), notification-area icon (Windows) or tray item
//! (Linux, through the AppIndicator library), with the platform's own menu:
//! the daemon's state as two greyed-out lines, then Open Chat with Work,
//! Pause Sharing, Settings and Quit.

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
    pub const SETTINGS: &str = "settings";
    pub const QUIT: &str = "quit";
}

pub const OPEN_LABEL: &str = "Open Chat with Work";

/// The label of the pause entry.
pub fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Resume Sharing"
    } else {
        "Pause Sharing"
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
    Open,
    TogglePause,
    Settings,
    Quit,
}

pub fn action(id: &str) -> Option<Action> {
    Some(match id {
        ids::OPEN => Action::Open,
        ids::PAUSE => Action::TogglePause,
        ids::SETTINGS => Action::Settings,
        ids::QUIT => Action::Quit,
        _ => return None,
    })
}

pub struct Tray {
    icon: TrayIcon,
    headline: MenuItem<tauri::Wry>,
    detail: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    view: TrayView,
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
        let headline = MenuItem::with_id(app, ids::HEADLINE, &view.headline, false, None::<&str>)?;
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
        let settings = MenuItem::with_id(
            app,
            ids::SETTINGS,
            platform.settings_item(),
            true,
            None::<&str>,
        )?;
        let quit = MenuItem::with_id(app, ids::QUIT, platform.quit_item(), true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &headline,
                &detail,
                &PredefinedMenuItem::separator(app)?,
                &open,
                &pause,
                &settings,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
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
                    crate::desktop::tray_action(tray.app_handle(), ids::OPEN);
                }
            })
            .build(app)?;
        Ok(Self {
            icon,
            headline,
            detail,
            pause,
            view: view.clone(),
        })
    }

    pub fn update(&mut self, view: &TrayView) {
        if *view == self.view {
            return;
        }
        let platform = Platform::current();
        let old = std::mem::replace(&mut self.view, view.clone());
        if view.headline != old.headline {
            let _ = self.headline.set_text(&view.headline);
        }
        // Tauri's menus can't hide an item, so a missing detail is an empty,
        // greyed-out line.
        if view.detail != old.detail {
            let _ = self
                .detail
                .set_text(view.detail.clone().unwrap_or_default());
        }
        if view.paused != old.paused {
            let _ = self.pause.set_text(pause_label(view.paused == Some(true)));
            let _ = self.pause.set_enabled(view.paused.is_some());
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
            headline: "Online, sharing 2 folders".into(),
            detail: detail.map(Into::into),
            paused: Some(false),
            tooltip: "Chat with Work Desktop: Online".into(),
            dimmed: false,
        }
    }

    #[test]
    fn entries_do_what_they_say() {
        assert_eq!(action("open"), Some(Action::Open));
        assert_eq!(action("pause"), Some(Action::TogglePause));
        assert_eq!(action("settings"), Some(Action::Settings));
        assert_eq!(action("quit"), Some(Action::Quit));
        assert_eq!(action("headline"), None);
        assert_eq!(action("detail"), None);
        assert_eq!(pause_label(true), "Resume Sharing");
        assert_eq!(pause_label(false), "Pause Sharing");
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
