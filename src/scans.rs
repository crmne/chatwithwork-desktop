//! Scans from the desktop app's scan page, sent to Chat with Work as this
//! computer's owner (PROTOCOL.md, section 13).
//!
//! The app makes the searchable PDF on the computer and hands it to the
//! daemon over the control socket; the daemon is the only part that holds
//! the device's token, so the scan goes the way attachments and dictation
//! do. It goes to one of two places:
//!
//! - a new chat, with the PDF attached (an upload, then a new chat, as the
//!   terminal's composer asks), or
//! - a bot whose trigger listens for scans from Chat with Work Desktop, as a
//!   trigger event (`POST /local_agent/scans`).
//!
//! The daemon keeps nothing: the bytes go from the request to the server.

use serde::{Deserialize, Serialize};

use crate::control::Refusal;

/// Where a scan goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// A new chat, private or in a project.
    Chat {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<u64>,
    },
    /// A bot's trigger that takes scans, by the trigger's ID from
    /// `scan_destinations`.
    Trigger { id: u64 },
}

/// The longest message sent with a scan into a new chat.
pub const MAX_MESSAGE_CHARS: usize = 2_000;
/// The most pages a scan may say it has.
pub const MAX_PAGES: u32 = 500;

/// A scan the app made, checked before anything leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub filename: String,
    pub pdf: Vec<u8>,
    pub pages: u32,
    /// The app's own ID for this scan, so a retry is heard once.
    pub scan_id: String,
    /// The scanner's name, for the bot's run ("Canon P-208II").
    pub scanner: Option<String>,
}

impl Scan {
    pub fn check(self) -> Result<Self, Refusal> {
        if !self.pdf.starts_with(b"%PDF-") {
            return Err(Refusal::new("invalid", "A scan must be a PDF"));
        }
        if self.pages == 0 || self.pages > MAX_PAGES {
            return Err(Refusal::new(
                "invalid",
                format!("A scan has 1 to {MAX_PAGES} pages"),
            ));
        }
        let id_ok = (8..=64).contains(&self.scan_id.len())
            && self
                .scan_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !id_ok {
            return Err(Refusal::new("invalid", "That isn't a scan's ID"));
        }
        let scanner = self
            .scanner
            .map(|s| {
                s.split(|c: char| c.is_control() || c.is_whitespace())
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(100)
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty());
        Ok(Self { scanner, ..self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan() -> Scan {
        Scan {
            filename: "scan.pdf".into(),
            pdf: b"%PDF-1.7\n...".to_vec(),
            pages: 2,
            scan_id: "0f8d2c1e-4b7a-4c55-9e10-2a6b3c4d5e6f".into(),
            scanner: Some("Canon\nP-208II  ".into()),
        }
    }

    #[test]
    fn destinations_read_as_the_app_sends_them() {
        let chat: Destination = serde_json::from_str(r#"{"kind":"chat"}"#).unwrap();
        assert_eq!(chat, Destination::Chat { project: None });
        let bot: Destination = serde_json::from_str(r#"{"kind":"trigger","id":12}"#).unwrap();
        assert_eq!(bot, Destination::Trigger { id: 12 });
        assert!(serde_json::from_str::<Destination>(r#"{"kind":"email"}"#).is_err());
    }

    #[test]
    fn only_pdfs_with_pages_and_an_id_leave() {
        let checked = scan().check().unwrap();
        assert_eq!(checked.scanner.as_deref(), Some("Canon P-208II"));
        assert!(
            Scan {
                pdf: b"GIF89a".to_vec(),
                ..scan()
            }
            .check()
            .is_err()
        );
        assert!(Scan { pages: 0, ..scan() }.check().is_err());
        assert!(
            Scan {
                scan_id: "../../etc".into(),
                ..scan()
            }
            .check()
            .is_err()
        );
        assert!(
            Scan {
                scanner: Some(" \n".into()),
                ..scan()
            }
            .check()
            .unwrap()
            .scanner
            .is_none()
        );
    }
}
