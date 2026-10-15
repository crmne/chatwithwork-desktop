//! Newline-delimited JSON over inherited stdin/stdout, never a public socket.
//!
//! `Ready` announces protocol availability, not notification permission.
//! `Shown` means OS acceptance, not a visible banner. Only the supervisor's
//! explicit `RequestPermission` command may open a native permission prompt.
//! IDs are opaque, account-scoped replacement keys, never URLs or message text.
//! A supervisor assigns each show an `attempt`, echoed by lifecycle events,
//! so buffered callbacks cannot acknowledge a newer same-ID replacement.
//! Activation reports an ID to the parent and never opens anything itself.
//!
//! The broker retains at most [`MAX_ACTIVE`] observations. At capacity, a new
//! ID fails; replacing or withdrawing an existing ID remains possible. The
//! parent owns retention, fallback and durable routing. Withdrawal queues a
//! best-effort native close and immediately retires the observation, so new
//! notifications can use its slot. It emits no completion acknowledgement.
//! EOF or `Exit` releases observations, leaving delivered OS history intact.

use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 2;
pub const MAX_ACTIVE: usize = 32;
pub const MAX_FRAME_BYTES: usize = 32 * 1024;
pub const MAX_ID_BYTES: usize = 256;
pub const MAX_TITLE_BYTES: usize = 512;
pub const MAX_BODY_BYTES: usize = 4096;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Show {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    Withdraw {
        id: String,
    },
    Permission,
    RequestPermission,
    Exit,
}

// Serde ignores extra fields on internally tagged unit variants even with
// deny_unknown_fields. Empty struct variants make all commands strict while
// retaining the convenient unit variants in the public Rust interface.
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum StrictCommand {
    Show {
        id: String,
        #[serde(default)]
        attempt: Option<u64>,
        title: String,
        #[serde(default)]
        body: Option<String>,
    },
    Withdraw {
        id: String,
    },
    Permission {},
    RequestPermission {},
    Exit {},
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StrictCommand::deserialize(deserializer)? {
            StrictCommand::Show {
                id,
                attempt,
                title,
                body,
            } => Self::Show {
                id,
                attempt,
                title,
                body,
            },
            StrictCommand::Withdraw { id } => Self::Withdraw { id },
            StrictCommand::Permission {} => Self::Permission,
            StrictCommand::RequestPermission {} => Self::RequestPermission,
            StrictCommand::Exit {} => Self::Exit,
        })
    }
}

impl Command {
    /// Validate before retaining content or passing it to an OS API. Errors
    /// are static and contain none of the rejected private input.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Show {
                id, title, body, ..
            } => {
                validate_id(id)?;
                if title.trim().is_empty() || title.len() > MAX_TITLE_BYTES || title.contains('\0')
                {
                    return Err("Invalid notification title");
                }
                if body
                    .as_ref()
                    .is_some_and(|body| body.len() > MAX_BODY_BYTES || body.contains('\0'))
                {
                    return Err("Invalid notification body");
                }
            }
            Self::Withdraw { id } => validate_id(id)?,
            Self::Permission | Self::RequestPermission | Self::Exit => {}
        }
        Ok(())
    }

    /// A bounded valid routing ID for error reporting, if this command has one.
    pub fn id(&self) -> Option<&str> {
        let id = match self {
            Self::Show { id, .. } | Self::Withdraw { id } => id,
            _ => return None,
        };
        validate_id(id).ok().map(|()| id.as_str())
    }
}

pub fn validate_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(char::is_control) {
        Err("Invalid notification ID")
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    NotDetermined,
    Denied,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Ready {
        protocol: u8,
    },
    Permission {
        state: PermissionState,
    },
    Shown {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        message: String,
    },
    Activated {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
    },
    Dismissed {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
    },
    Closed {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_and_optional_fields_are_stable() {
        let command: Command =
            serde_json::from_str(r#"{"command":"show","id":"account:notice","title":"Ready"}"#)
                .unwrap();
        assert_eq!(
            command,
            Command::Show {
                id: "account:notice".into(),
                attempt: None,
                title: "Ready".into(),
                body: None,
            }
        );
        assert_eq!(
            serde_json::to_value(Event::Activated {
                id: "notice".into(),
                attempt: None,
                action: None,
            })
            .unwrap(),
            serde_json::json!({"event":"activated", "id":"notice"})
        );
        assert_eq!(
            serde_json::to_value(Event::Permission {
                state: PermissionState::NotDetermined,
            })
            .unwrap(),
            serde_json::json!({"event":"permission", "state":"not_determined"})
        );
        assert_eq!(
            serde_json::to_value(Command::RequestPermission).unwrap(),
            serde_json::json!({"command":"request_permission"})
        );
    }

    #[test]
    fn commands_do_not_accept_launch_targets_or_implicit_prompt_flags() {
        for input in [
            r#"{"command":"show","id":"n","title":"Ready","url":"https://example.com"}"#,
            r#"{"command":"show","id":"n","title":"Ready","request_permission":true}"#,
            r#"{"command":"permission","prompt":true}"#,
            r#"{"command":"launch","id":"n"}"#,
        ] {
            assert!(serde_json::from_str::<Command>(input).is_err());
        }
    }

    #[test]
    fn content_and_identifiers_are_bounded_without_echoing_rejected_input() {
        for id in [
            String::new(),
            "n\nprivate".into(),
            "x".repeat(MAX_ID_BYTES + 1),
        ] {
            let command = Command::Withdraw { id };
            assert_eq!(command.validate(), Err("Invalid notification ID"));
            assert_eq!(command.id(), None);
        }
        let mut command = Command::Show {
            id: "account:notice".into(),
            attempt: None,
            title: "é".repeat(MAX_TITLE_BYTES / 2),
            body: Some("x".repeat(MAX_BODY_BYTES)),
        };
        assert_eq!(command.validate(), Ok(()));
        if let Command::Show { title, .. } = &mut command {
            title.push('x');
        }
        assert_eq!(command.validate(), Err("Invalid notification title"));
        if let Command::Show { title, body, .. } = &mut command {
            *title = "Ready".into();
            *body = Some("x".repeat(MAX_BODY_BYTES + 1));
        }
        assert_eq!(command.validate(), Err("Invalid notification body"));
    }
}
