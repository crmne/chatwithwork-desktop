//! The app's own settings, next to the daemon's config (`app.json`).

use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// The welcome page was finished or skipped.
    pub onboarded: bool,
    /// The first launch asked for notification permission (first_launch.rs).
    pub notifications_asked: bool,
}

impl AppState {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read(paths.app_state_file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let file = paths.app_state_file();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, serde_json::to_vec_pretty(self).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reads_back_and_ignores_older_fields() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert!(!AppState::load(&paths).onboarded);
        AppState {
            onboarded: true,
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
        assert!(AppState::load(&paths).onboarded);
        // The egui app also kept its title bar and chat choices here.
        std::fs::write(
            paths.app_state_file(),
            r#"{"onboarded":true,"custom_titlebar":false,"organization":null}"#,
        )
        .unwrap();
        assert!(AppState::load(&paths).onboarded);
    }
}
