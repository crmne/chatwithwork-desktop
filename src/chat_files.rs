//! Files from a chat, fetched for `copy_from_chat`: the server streams the
//! one file a transfer names (`GET /desktop/chat_file_transfers/<transfer>`),
//! to this device only, with its DPoP-bound token, after the person approved
//! saving it. The transfer is the server's short-lived grant for that file,
//! that chat and this device; the daemon checks its form and nothing else of
//! it, and checks the bytes that arrive against the size and SHA-256 the
//! approved call names.

use std::sync::{Arc, RwLock};

use crate::auth::client::Fetched;
use crate::auth::tokens::Tokens;
use crate::error::{ErrorCode, ToolError};

/// The most a file from a chat may be.
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// The tunnel's tokens, once it's up: the daemon fills this in each time it
/// connects (`Daemon::run`), and empties it when it's offline.
#[derive(Clone, Default)]
pub struct ChatFiles {
    tokens: Arc<RwLock<Option<Arc<Tokens>>>>,
}

/// A transfer is a URL-safe token of the server's: letters, digits, `-`,
/// `_`, `=` and `.`, at most 1 KiB, so it can only name that one path.
pub fn valid_transfer(transfer: &str) -> bool {
    (16..=1024).contains(&transfer.len())
        && transfer
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'.'))
}

impl ChatFiles {
    pub fn set(&self, tokens: Option<Arc<Tokens>>) {
        *self.tokens.write().unwrap_or_else(|e| e.into_inner()) = tokens;
    }

    /// Stream the file `transfer` names into `sink`. Blocks on the network.
    pub fn fetch(
        &self,
        transfer: &str,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), ToolError>,
    ) -> Result<(), ToolError> {
        if !valid_transfer(transfer) {
            return Err(ToolError::invalid_argument(
                "`transfer` isn't one the server made",
            ));
        }
        let Some(tokens) = self
            .tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Err(ToolError::new(
                ErrorCode::Unavailable,
                "This computer isn't connected to Chat with Work right now.",
            ));
        };
        let path = format!("desktop/chat_file_transfers/{transfer}");
        let mut retried = false;
        loop {
            let token = tokens.access_token().map_err(|e| {
                ToolError::new(
                    ErrorCode::Unavailable,
                    format!("Couldn't sign in to Chat with Work: {e}"),
                )
            })?;
            let fetched = tokens
                .client()
                .download(tokens.key(), &path, &token, sink)
                .map_err(|e| {
                    ToolError::new(
                        ErrorCode::Unavailable,
                        format!("Couldn't fetch the file: {e:#}"),
                    )
                })?;
            match fetched {
                Fetched::Done => return Ok(()),
                Fetched::Stopped(refusal) => return Err(refusal),
                Fetched::Refused { status: 401, .. } if !retried => {
                    tokens.invalidate();
                    retried = true;
                }
                Fetched::Refused { status, body } => {
                    let reason = body["error_description"]
                        .as_str()
                        .map(|s| {
                            s.chars()
                                .filter(|c| !c.is_control())
                                .take(300)
                                .collect::<String>()
                        })
                        .unwrap_or_else(|| format!("the server answered {status}"));
                    return Err(ToolError::new(
                        if status == 404 || status == 410 {
                            ErrorCode::NotFound
                        } else {
                            ErrorCode::NotAllowed
                        },
                        format!("Chat with Work didn't send the file: {reason}"),
                    ));
                }
            }
        }
    }
}

impl ChatFiles {
    /// Send the file at `file` (where it really is, checked by the reader)
    /// into the chat the server's `transfer` names (`copy_to_chat`):
    /// `POST /desktop/chat_file_uploads/<transfer>`, streamed, with its size
    /// and SHA-256 for the server to check. Answers what the server kept.
    pub fn send(
        &self,
        transfer: &str,
        file: &std::path::Path,
        size: u64,
        sha256: &str,
    ) -> Result<serde_json::Value, ToolError> {
        if !valid_transfer(transfer) {
            return Err(ToolError::invalid_argument(
                "`transfer` isn't one the server made",
            ));
        }
        let Some(tokens) = self
            .tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Err(ToolError::new(
                ErrorCode::Unavailable,
                "This computer isn't connected to Chat with Work right now.",
            ));
        };
        let path = format!("desktop/chat_file_uploads/{transfer}");
        let mut retried = false;
        loop {
            let token = tokens.access_token().map_err(|e| {
                ToolError::new(
                    ErrorCode::Unavailable,
                    format!("Couldn't sign in to Chat with Work: {e}"),
                )
            })?;
            let (status, body) = tokens
                .client()
                .upload_file(tokens.key(), &path, &token, file, size, sha256)
                .map_err(|e| {
                    ToolError::new(
                        ErrorCode::Unavailable,
                        format!("Couldn't send the file: {e:#}"),
                    )
                })?;
            match status {
                200..=299 => return Ok(body),
                401 if !retried => {
                    tokens.invalidate();
                    retried = true;
                }
                _ => {
                    let reason = body["error_description"]
                        .as_str()
                        .map(|s| {
                            s.chars()
                                .filter(|c| !c.is_control())
                                .take(300)
                                .collect::<String>()
                        })
                        .unwrap_or_else(|| format!("the server answered {status}"));
                    return Err(ToolError::new(
                        match status {
                            409 => ErrorCode::Exists,
                            413 => ErrorCode::TooLarge,
                            404 | 410 => ErrorCode::NotFound,
                            _ => ErrorCode::NotAllowed,
                        },
                        format!("Chat with Work didn't keep the file: {reason}"),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_can_name_only_its_own_path() {
        assert!(valid_transfer(
            "eyJfcmFpbHMiOnsiZGF0YSI6WzFdfX0--0123456789abcdef"
        ));
        for bad in [
            "short",
            "../../desktop/chats",
            "a/b/cccccccccccccccccc",
            "with space aaaaaaaaaaa",
            "q?x=1aaaaaaaaaaaaaaaa",
        ] {
            assert!(!valid_transfer(bad), "{bad}");
        }
        assert!(!valid_transfer(&"a".repeat(1025)));
    }

    #[test]
    fn without_a_connection_nothing_is_fetched() {
        let files = ChatFiles::default();
        let error = files
            .fetch(
                "eyJfcmFpbHMiOnsiZGF0YSI6WzFdfX0--0123456789abcdef",
                &mut |_| Ok(()),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
    }
}
