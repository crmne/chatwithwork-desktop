//! The first time the app starts, it turns notifications on, so the
//! background agent can tell the person when a chat needs them or has
//! finished without their opening Settings first. macOS shows its own
//! prompt, Windows a first notification that says they're on, and Linux
//! needs nothing. Once answered, it never asks again: the person changes
//! it in Settings > Notifications or in the system's settings.
//!
//! The daemon's status starts as `unavailable` and checks permission only
//! while paired, so the app first asks it to check (which starts the
//! notification helper), asks for permission only when nobody has
//! answered, and remembers only an answer the helper gave. Anything else
//! (no agent, no helper, a prompt left unanswered) is tried again at the
//! next launch.

use std::time::{Duration, Instant};

use cww::control::ControlRequest;
use serde_json::Value;

use crate::control::Client;
use crate::paths::Paths;
use crate::state::AppState;

/// How long to wait for the agent and its helper, and for the person to
/// answer the system's prompt.
const PATIENCE: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_secs(2);

/// Where asking has got to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Nothing sent yet.
    Start,
    /// A quiet check was sent; waiting for the helper's answer.
    Checking,
    /// Permission was requested; waiting for the person, or Windows.
    Asked,
}

/// What to do next, from the step and the permission the daemon reports.
#[derive(Debug, PartialEq)]
enum Next {
    Check,
    Ask,
    /// An answer the helper gave: remember it and stop.
    Answered,
    Wait,
}

fn next(step: Step, permission: Option<&str>) -> Next {
    match (step, permission) {
        (Step::Start, _) => Next::Check,
        // Linux can't be asked and reports unknown: nothing to turn on.
        (_, Some("granted" | "denied" | "unknown")) => Next::Answered,
        (Step::Checking, Some("not_determined")) => Next::Ask,
        // Still unavailable (the helper starting, or missing), or the
        // prompt still open.
        _ => Next::Wait,
    }
}

fn permission(status: &Value) -> Option<&str> {
    status.get("permission").and_then(Value::as_str)
}

/// Asks for notifications once, on its own thread, unless an earlier
/// launch got an answer.
pub fn ask_once(paths: Paths) {
    if AppState::load(&paths).notifications_asked {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cww-notifications".into())
        .spawn(move || {
            let client = Client::new(paths.socket_path());
            let deadline = Instant::now() + PATIENCE;
            let mut step = Step::Start;
            while Instant::now() < deadline {
                let status = match client.request(&ControlRequest::NotificationStatus) {
                    Ok(status) => status,
                    Err(_) => {
                        // No agent yet: start over once it's up.
                        step = Step::Start;
                        std::thread::sleep(POLL);
                        continue;
                    }
                };
                let request = match next(step, permission(&status)) {
                    Next::Answered => return remember(&paths),
                    Next::Wait => {
                        std::thread::sleep(POLL);
                        continue;
                    }
                    Next::Check => false,
                    Next::Ask => true,
                };
                match client.request(&ControlRequest::NotificationPermission { request }) {
                    Ok(_) => step = if request { Step::Asked } else { Step::Checking },
                    Err(e) => log::warn!("couldn't ask about notifications: {e}"),
                }
                std::thread::sleep(POLL);
            }
            log::info!("notifications weren't answered; asking again at the next launch");
        });
    if let Err(e) = spawned {
        log::warn!("can't ask for notifications: {e}");
    }
}

fn remember(paths: &Paths) {
    let mut state = AppState::load(paths);
    state.notifications_asked = true;
    if let Err(e) = state.save(paths) {
        log::warn!("couldn't remember asking for notifications: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_checks_first_whatever_the_status_says() {
        for permission in [
            None,
            Some("unavailable"),
            Some("granted"),
            Some("not_determined"),
        ] {
            assert_eq!(next(Step::Start, permission), Next::Check, "{permission:?}");
        }
    }

    #[test]
    fn it_asks_only_when_the_check_found_nobody_had_answered() {
        assert_eq!(next(Step::Checking, Some("not_determined")), Next::Ask);
        assert_eq!(next(Step::Checking, Some("unavailable")), Next::Wait);
        assert_eq!(next(Step::Checking, None), Next::Wait);
    }

    #[test]
    fn only_the_helpers_answers_count() {
        for answered in ["granted", "denied", "unknown"] {
            assert_eq!(
                next(Step::Checking, Some(answered)),
                Next::Answered,
                "{answered}"
            );
            assert_eq!(
                next(Step::Asked, Some(answered)),
                Next::Answered,
                "{answered}"
            );
        }
        // Asked, and the prompt is still open or the helper restarting.
        assert_eq!(next(Step::Asked, Some("not_determined")), Next::Wait);
        assert_eq!(next(Step::Asked, Some("unavailable")), Next::Wait);
    }

    #[test]
    fn asking_is_remembered_beside_the_welcome() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        AppState {
            onboarded: true,
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
        remember(&paths);
        let state = AppState::load(&paths);
        assert!(state.onboarded && state.notifications_asked);
    }
}
