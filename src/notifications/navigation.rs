//! An account-scoped destination for the desktop's private launch IPC: a
//! chat, or the account's Decisions. It carries no executable, arbitrary
//! path, or browser URL.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::auth::client::ServerUrl;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WireTarget", into = "WireTarget")]
pub struct Target {
    server: ServerUrl,
    account: String,
    place: Place,
}

/// Where in an account a notification leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Chat(u64),
    Decisions,
}

impl Place {
    pub fn path(self, account: &str) -> String {
        match self {
            Self::Chat(chat) => format!("/{account}/chats/{chat}"),
            Self::Decisions => format!("/{account}/decisions"),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTarget {
    server: String,
    account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    decisions: bool,
}

impl Target {
    pub fn new(server: ServerUrl, account: String, place: Place) -> Result<Self> {
        if account.len() != 9
            || !account.bytes().all(|b| b.is_ascii_digit())
            || place == Place::Chat(0)
        {
            bail!("invalid chat destination");
        }
        Ok(Self {
            server,
            account,
            place,
        })
    }

    pub fn parse(value: &str) -> Result<Self> {
        if value.len() > 4096 {
            bail!("chat destination is too long");
        }
        Ok(serde_json::from_str(value)?)
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a chat destination serializes")
    }

    pub fn server(&self) -> &ServerUrl {
        &self.server
    }
    pub fn path(&self) -> String {
        self.place.path(&self.account)
    }
}

impl TryFrom<WireTarget> for Target {
    type Error = anyhow::Error;

    fn try_from(value: WireTarget) -> Result<Self> {
        let server = ServerUrl::parse(&value.server)?;
        // Reject normalization tricks, whitespace, implicit schemes, and paths.
        if value.server != server.origin() || value.server.len() > 2048 {
            bail!("chat destination needs a canonical server origin");
        }
        let place = match (value.chat, value.decisions) {
            (Some(chat), false) => Place::Chat(chat),
            (None, true) => Place::Decisions,
            _ => bail!("a destination is a chat or Decisions"),
        };
        Self::new(server, value.account, place)
    }
}

impl From<Target> for WireTarget {
    fn from(value: Target) -> Self {
        Self {
            server: value.server.origin(),
            account: value.account,
            chat: match value.place {
                Place::Chat(chat) => Some(chat),
                Place::Decisions => None,
            },
            decisions: value.place == Place::Decisions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chat_destination_round_trips_without_an_arbitrary_url() {
        let target = Target::new(
            ServerUrl::parse("https://example.test").unwrap(),
            "482139075".into(),
            Place::Chat(16),
        )
        .unwrap();
        assert_eq!(Target::parse(&target.encode()).unwrap(), target);
        assert_eq!(
            target.encode(),
            r#"{"server":"https://example.test","account":"482139075","chat":16}"#,
            "an app from before Decisions still reads a chat"
        );
        for server in [
            "example.test",
            " https://example.test",
            "https://example.test/",
            "https://example.test/path",
            "https://user@example.test",
            "https://example.test?x",
            "file:///tmp/chat",
            "http://example.test",
        ] {
            assert!(
                Target::parse(
                    &json!({"server":server,"account":"482139075","chat":16}).to_string()
                )
                .is_err(),
                "{server}"
            );
        }
        for value in [
            json!({"server":"https://example.test","account":"../secret","chat":16}),
            json!({"server":"https://example.test","account":"482139075","chat":0}),
            json!({"server":"https://example.test","account":"482139075","chat":16,"url":"https://other.test"}),
            json!({"server":"https://example.test","account":"482139075"}),
            json!({"server":"https://example.test","account":"482139075","decisions":false}),
            json!({"server":"https://example.test","account":"482139075","chat":16,"decisions":true}),
        ] {
            assert!(Target::parse(&value.to_string()).is_err());
        }
        assert!(Target::parse(&"x".repeat(4097)).is_err());
    }

    #[test]
    fn decisions_are_a_destination_of_their_own() {
        let target = Target::new(
            ServerUrl::parse("https://example.test").unwrap(),
            "482139075".into(),
            Place::Decisions,
        )
        .unwrap();
        assert_eq!(Target::parse(&target.encode()).unwrap(), target);
        assert_eq!(target.path(), "/482139075/decisions");
    }
}
