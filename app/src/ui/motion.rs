//! Reduced motion: the system's accessibility setting, unless
//! `[desktop] reduce_motion` in `config.toml` or `CWW_REDUCE_MOTION`
//! (`1` or `0`) says otherwise. Read once per process.

use std::sync::OnceLock;

/// Whether to keep motion to what's needed: no fading in of streamed text.
pub fn reduced(paths: &cww::paths::Paths) -> bool {
    static REDUCED: OnceLock<bool> = OnceLock::new();
    *REDUCED.get_or_init(|| {
        let configured = cww::config::Config::load(paths)
            .ok()
            .and_then(|c| c.desktop.reduce_motion);
        decide(
            std::env::var("CWW_REDUCE_MOTION").ok().as_deref(),
            configured,
            crate::platform::prefers_reduced_motion,
        )
    })
}

/// The variable, then the config, then the system; motion by default.
fn decide(
    variable: Option<&str>,
    configured: Option<bool>,
    system: impl FnOnce() -> Option<bool>,
) -> bool {
    match variable.map(str::trim) {
        Some("1" | "true" | "yes") => return true,
        Some("0" | "false" | "no") => return false,
        _ => {}
    }
    configured.or_else(system).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::decide;

    #[test]
    fn the_variable_then_the_config_then_the_system() {
        assert!(decide(Some("1"), Some(false), || Some(false)));
        assert!(!decide(Some("0"), Some(true), || Some(true)));
        assert!(decide(None, Some(true), || Some(false)));
        assert!(!decide(Some("maybe"), Some(false), || Some(true)));
        assert!(decide(None, None, || Some(true)));
        assert!(!decide(None, None, || None));
    }
}
