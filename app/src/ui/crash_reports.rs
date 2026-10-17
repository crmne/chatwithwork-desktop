//! Crash reports: asked once, then a switch on the General page. The choice
//! is `crash_reports` in config.toml, which every part of the Local Agent
//! reads when it crashes (`cww::crash_reports`).

use cww::crash_reports::{ABOUT, Consent, ENV};
use egui::{RichText, Ui};

use super::widgets::{self, setting_row};
use super::{Page, SettingsApp};

pub(super) const QUESTION: &str = "Send crash reports to the Chat with Work team?";
const LATER: &str = "You can change this any time under General.";

#[derive(Default)]
pub(super) struct CrashReportsState {
    pub consent: Consent,
    /// The question was closed without an answer; ask again next time.
    pub dismissed: bool,
    pub error: Option<String>,
}

impl SettingsApp {
    pub(super) fn set_crash_reports(&mut self, on: bool) {
        match cww::crash_reports::set(&self.shared.paths.daemon, on) {
            Ok(()) => {
                self.crash_reports.consent.enabled = Some(on);
                self.crash_reports.error = None;
            }
            Err(e) => {
                self.crash_reports.error =
                    Some(format!("Couldn't save the crash report setting: {e:#}"));
            }
        }
    }

    /// The question, over any page but the welcome page, which asks in a
    /// step of its own, until it's answered or closed.
    pub(super) fn crash_reports_question(&mut self, ctx: &egui::Context) {
        if self.page == Page::Welcome
            || !self.crash_reports.consent.undecided()
            || self.crash_reports.dismissed
        {
            return;
        }
        let theme = self.theme;
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("crash_reports_question")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(theme.strong(QUESTION));
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(ABOUT).color(theme.palette.text)).wrap());
            ui.add_space(2.0);
            ui.add(egui::Label::new(theme.small(LATER)).wrap());
            if let Some(error) = self.crash_reports.error.clone() {
                ui.add_space(6.0);
                widgets::notice(ui, &theme, &error, theme.palette.danger);
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &theme, "Send Crash Reports").clicked() {
                    answer = Some(true);
                }
                if ui.button("Don't Send").clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(on) = answer {
            self.set_crash_reports(on);
        } else if modal.should_close() {
            self.crash_reports.dismissed = true;
        }
    }

    /// The welcome page's step: two buttons, then what was chosen.
    pub(super) fn welcome_crash_reports(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        ui.add(egui::Label::new(RichText::new(ABOUT).color(theme.palette.text)).wrap());
        ui.add_space(4.0);
        match self.crash_reports.consent.enabled {
            Some(on) => {
                ui.label(theme.weak(if on {
                    "Crash reports are on. You can change this under General."
                } else {
                    "Crash reports are off. You can change this under General."
                }));
            }
            None => {
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &theme, "Send Crash Reports").clicked() {
                        self.set_crash_reports(true);
                    }
                    if ui.button("Don't Send").clicked() {
                        self.set_crash_reports(false);
                    }
                });
            }
        }
        if let Some(error) = self.crash_reports.error.clone() {
            widgets::notice(ui, &theme, &error, theme.palette.danger);
        }
    }

    /// The General page's switch.
    pub(super) fn crash_reports_row(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let from_env = self.crash_reports.consent.from_env;
        let description = if from_env {
            format!("{ABOUT} Set by {ENV} in the environment.")
        } else {
            ABOUT.to_string()
        };
        setting_row(ui, &theme, "Send crash reports", &description, |ui| {
            let mut on = self.crash_reports.consent.sends();
            let changed = ui
                .add_enabled_ui(!from_env, |ui| {
                    widgets::switch(ui, &theme, &mut on, "Send crash reports")
                })
                .inner
                .changed();
            if changed {
                self.set_crash_reports(on);
            }
        });
        if let Some(error) = self.crash_reports.error.clone() {
            ui.add_space(6.0);
            widgets::notice(ui, &theme, &error, theme.palette.danger);
        }
    }
}
