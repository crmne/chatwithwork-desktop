//! The app's header, with the platform's caption controls and drag behavior.

use egui::Ui;

use super::{Page, Theme};
use crate::model::Platform;

// GNOME's round caption buttons are 28 points high, with 14 above and below.
pub const HEIGHT: f32 = if cfg!(target_os = "linux") {
    56.0
} else {
    40.0
};

pub fn show(ui: &mut Ui, theme: &Theme, page: Page) {
    if ui.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let p = theme.palette;
    let fill = if page == Page::Chat {
        super::chat::tokens::Palette::new(theme.dark).canvas
    } else {
        p.window
    };
    egui::Panel::top("window_header")
        .exact_size(HEIGHT)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(fill))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if page != Page::Welcome && page != Page::Chat {
                let mut sidebar = rect;
                sidebar.max.x = sidebar.min.x + theme.metrics.sidebar_width;
                ui.painter().rect_filled(sidebar, 0, p.sidebar);
            }
            let room = fastframe_titlebar::footprint(ui.ctx(), HEIGHT);
            let mut drag_rect = rect;
            drag_rect.min.x += room.left;
            drag_rect.max.x -= room.right;
            fastframe_titlebar::drag(ui, drag_rect);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Chat with Work",
                egui::FontId::proportional(12.0),
                p.weak,
            );
            if theme.platform != Platform::MacOs {
                fastframe_titlebar::controls(
                    ui,
                    &fastframe_titlebar::Style {
                        icon: p.text,
                        button: p.control_hover,
                        header_height: HEIGHT,
                    },
                    &fastframe_titlebar::Labels {
                        minimize: "Minimize window",
                        maximize: "Maximize window",
                        restore: "Restore window",
                        close: "Close window",
                    },
                );
            }
        });
}

pub fn resize(ui: &mut Ui, platform: Platform) {
    if platform != Platform::MacOs {
        fastframe_titlebar::resize(ui);
    }
}
