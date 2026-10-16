//! Speech stays a preference of this computer, shared with `cww tui`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use cww::dictation::{Mode, Model, Preferences};
use cww::paths::Paths;
use egui::Ui;

use super::{theme::Theme, widgets};

pub(super) struct Settings {
    pub preferences: Preferences,
    error: Option<String>,
    download: Option<Download>,
}

struct Download {
    model: Model,
    cancelled: Arc<AtomicBool>,
    bytes: Arc<AtomicU64>,
    result: mpsc::Receiver<Result<(), String>>,
}

impl Drop for Download {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Settings {
    pub fn new(paths: &Paths) -> Self {
        let loaded = Preferences::load(paths);
        Self {
            preferences: loaded.as_ref().copied().unwrap_or_default(),
            error: loaded.err().map(|e| e.to_string()),
            download: None,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, paths: &Paths) {
        if let Some(download) = &self.download {
            match download.result.try_recv() {
                Ok(result) => {
                    self.error = result.err();
                    self.download = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("The download stopped. Try again.".into());
                    self.download = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        widgets::section(ui, theme, "Dictation");
        let before = self.preferences;
        widgets::group(ui, theme, |ui| {
            mode_choice(
                ui,
                theme,
                &mut self.preferences.mode,
                Mode::Cloud,
                "Cloud transcription",
            );
            ui.label(theme.small(
                "Uses credits. Audio is transcribed in the EU through your Chat with Work server.",
            ));
            ui.add_space(6.0);
            mode_choice(
                ui,
                theme,
                &mut self.preferences.mode,
                Mode::Local,
                "On this computer",
            );
            ui.add(egui::Label::new(theme.small("Whisper keeps audio on this computer and uses no credits. All models support multiple languages. Local failures never upload your recording.")).wrap());
            widgets::row_separator(ui, theme);
            ui.horizontal_wrapped(|ui| {
                let label = ui.label("Whisper model");
                egui::ComboBox::from_id_salt("speech-model")
                    .selected_text(self.preferences.model.label())
                    .show_ui(ui, |ui| {
                        for model in Model::ALL {
                            ui.selectable_value(&mut self.preferences.model, model, model.label());
                        }
                    })
                    .response
                    .labelled_by(label.id);
            });
            if let Some(download) = &self.download {
                let bytes = download.bytes.load(Ordering::Relaxed);
                ui.add_space(8.0);
                ui.add(
                    egui::ProgressBar::new(bytes as f32 / download.model.bytes() as f32).text(
                        format!(
                            "Downloading {}: {:.1} / {:.1} MB",
                            download.model.id(),
                            bytes as f64 / 1e6,
                            download.model.bytes() as f64 / 1e6
                        ),
                    ),
                );
                let cancelling = download.cancelled.load(Ordering::SeqCst);
                if ui
                    .add_enabled(
                        !cancelling,
                        egui::Button::new(if cancelling {
                            "Cancelling…"
                        } else {
                            "Cancel download"
                        }),
                    )
                    .clicked()
                {
                    download.cancelled.store(true, Ordering::SeqCst);
                }
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else {
                let model = self.preferences.model;
                ui.add_space(6.0);
                if model.downloaded(paths) {
                    ui.horizontal(|ui| {
                        ui.label(theme.small("Downloaded. Ready for local dictation."));
                        if ui.button("Remove model").clicked() {
                            self.error = model.remove(paths).err().map(|e| e.to_string());
                        }
                    });
                } else {
                    ui.add(egui::Label::new(theme.small("Download from Hugging Face once, then use it offline. Model files are verified before use and are not included in the app.")).wrap());
                    if ui.button("Download model").clicked() {
                        self.start(ui.ctx(), paths, model);
                    }
                }
            }
        });
        if before != self.preferences {
            match self.preferences.save(paths) {
                Ok(()) => self.error = None,
                Err(error) => {
                    self.preferences = before;
                    self.error = Some(error.to_string());
                }
            }
        }
        if let Some(error) = &self.error {
            ui.add_space(6.0);
            widgets::notice(ui, theme, error, theme.palette.danger);
        }
    }

    fn start(&mut self, ctx: &egui::Context, paths: &Paths, model: Model) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(AtomicU64::new(0));
        let (tx, result) = mpsc::sync_channel(1);
        let task_cancel = Arc::clone(&cancelled);
        let task_bytes = Arc::clone(&bytes);
        let paths = paths.clone();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("cww-speech-download".into())
            .spawn(move || {
                let result = model.download(&paths, &task_cancel, |bytes| {
                    task_bytes.store(bytes, Ordering::Relaxed);
                });
                let _ = tx.send(result.map_err(|error| error.to_string()));
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.download = Some(Download {
                    model,
                    cancelled,
                    bytes,
                    result,
                });
                self.error = None;
            }
            Err(_) => self.error = Some("Could not start the download. Try again.".into()),
        }
    }
}

fn mode_choice(ui: &mut Ui, theme: &Theme, mode: &mut Mode, value: Mode, label: &str) {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0, theme.palette.weak);
        ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::new(1.0, theme.palette.accent);
        ui.radio_value(mode, value, label);
    });
}
