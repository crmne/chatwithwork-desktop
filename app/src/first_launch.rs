//! The first time the app starts, it turns notifications on, so the
//! background agent can tell the person when a chat needs them or has
//! finished without their opening Settings first. macOS shows its own
//! prompt, Windows a first notification that says they're on, and Linux
//! needs nothing. It asks once: whatever the answer, the person changes it
//! later in Settings > Notifications or in the system's settings.

use std::time::Duration;

use cww::control::ControlRequest;
use serde_json::Value;

use crate::control::Client;
use crate::paths::Paths;
use crate::state::AppState;

/// How long to wait for the background agent and its notification helper.
const PATIENCE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_secs(2);

/// What to do with the agent's notification status.
#[derive(Debug, PartialEq)]
enum Next {
    /// Nobody has asked yet: ask.
    Ask,
    /// Already answered, or nothing to allow here: done, without asking.
    Done,
    /// The agent or its helper isn't up yet: look again.
    Wait,
}

fn next(status: &Value) -> Next {
    match status.get("permission").and_then(Value::as_str) {
        Some("not_determined") => Next::Ask,
        Some("granted" | "denied" | "unavailable") => Next::Done,
        _ => Next::Wait,
    }
}

/// Asks for notifications once, on its own thread, if this is the first
/// launch that could. Gives up quietly when the agent never answers, and
/// tries again at the next launch.
pub fn ask_once(paths: Paths) {
    if AppState::load(&paths).notifications_asked {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cww-notifications".into())
        .spawn(move || {
            let client = Client::new(paths.socket_path());
            let deadline = std::time::Instant::now() + PATIENCE;
            while std::time::Instant::now() < deadline {
                match client
                    .request(&ControlRequest::NotificationStatus)
                    .map(|s| next(&s))
                {
                    Ok(Next::Ask) => {
                        match client
                            .request(&ControlRequest::NotificationPermission { request: true })
                        {
                            Ok(_) => remember(&paths),
                            Err(e) => log::warn!("couldn't ask for notifications: {e}"),
                        }
                        return;
                    }
                    Ok(Next::Done) => return remember(&paths),
                    Ok(Next::Wait) | Err(_) => std::thread::sleep(POLL),
                }
            }
            log::info!("the background agent didn't answer; notifications are asked next time");
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
    use serde_json::json;

    #[test]
    fn it_asks_only_while_nobody_has_answered() {
        assert_eq!(next(&json!({ "permission": "not_determined" })), Next::Ask);
        for answered in ["granted", "denied", "unavailable"] {
            assert_eq!(
                next(&json!({ "permission": answered })),
                Next::Done,
                "{answered}"
            );
        }
        assert_eq!(next(&json!({ "permission": "unknown" })), Next::Wait);
        assert_eq!(next(&json!({})), Next::Wait);
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
