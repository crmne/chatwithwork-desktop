//! An account-scoped chat destination for the desktop's private launch IPC.
//! It carries no executable, arbitrary path, or browser URL.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::auth::client::ServerUrl;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WireTarget", into = "WireTarget")]
pub struct Target {
    server: ServerUrl,
    account: String,
    chat: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTarget {
    server: String,
    account: String,
    chat: u64,
}

impl Target {
    pub fn new(server: ServerUrl, account: String, chat: u64) -> Result<Self> {
        if account.len() != 9 || !account.bytes().all(|b| b.is_ascii_digit()) || chat == 0 {
            bail!("invalid chat destination");
        }
        Ok(Self {
            server,
            account,
            chat,
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
    pub fn account(&self) -> &str {
        &self.account
    }
    pub fn chat(&self) -> u64 {
        self.chat
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
        Self::new(server, value.account, value.chat)
    }
}

impl From<Target> for WireTarget {
    fn from(value: Target) -> Self {
        Self {
            server: value.server.origin(),
            account: value.account,
            chat: value.chat,
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
            16,
        )
        .unwrap();
        assert_eq!(Target::parse(&target.encode()).unwrap(), target);
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
        ] {
            assert!(Target::parse(&value.to_string()).is_err());
        }
        assert!(Target::parse(&"x".repeat(4097)).is_err());
    }
}
