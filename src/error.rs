//! Errors returned to the server as tool-level errors.
//!
//! Every code here is part of the wire protocol (see PROTOCOL.md).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Arguments are missing, malformed, or out of range.
    InvalidArgument,
    /// The path is not of the form `root_id:relative/path`, or contains an
    /// absolute path, `..`, a NUL byte, a backslash, or a drive letter.
    InvalidPath,
    /// No root with that ID is shared.
    UnknownRoot,
    /// Resolution would leave the root (through a symlink or otherwise).
    OutsideRoot,
    /// The path matches the deny list, is a symlink that is not followed, is a
    /// hard link, or is not a regular file or directory.
    Denied,
    NotFound,
    NotAFile,
    NotADirectory,
    /// The file is larger than the configured limit.
    TooLarge,
    /// The file type can't be turned into text.
    Unsupported,
    /// A rate or volume limit was hit. Retrying later may work.
    RateLimited,
    /// The user paused the agent.
    Paused,
    /// A change in a shared folder that doesn't allow changes.
    NotWritable,
    /// A change would overwrite something that is already there.
    Exists,
    /// A kind of file the daemon never changes: executables, launchers,
    /// documents it can't write whole, or a binary file as text.
    NotChangeable,
    /// The system trash can't take the old version, so nothing changed.
    TrashUnavailable,
    /// The file changed since the caller looked at it.
    Conflict,
    /// A bot's call reached a folder that isn't lent to that bot.
    NotLent,
    /// A bot's command or a local browser action lacks the required
    /// per-action approval.
    NeedsApproval,
    /// The browser with the extension isn't connected to the daemon.
    BrowserUnavailable,
    /// The person stopped the browser, in the extension or on the computer.
    BrowserStopped,
    /// The page isn't on a site the person allowed.
    SiteNotAllowed,
    /// The tab isn't in the Chat with Work tab group.
    NotInGroup,
    /// The Chat with Work tab group has no tab.
    NoTab,
    /// A password, payment or one-time code field, never typed into or read.
    SensitiveField,
    /// A click said not to submit would submit, send, buy or delete.
    WouldSubmit,
    /// The page or the browser failed to do what was asked.
    PageError,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::InvalidPath => "invalid_path",
            Self::UnknownRoot => "unknown_root",
            Self::OutsideRoot => "outside_root",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::NotAFile => "not_a_file",
            Self::NotADirectory => "not_a_directory",
            Self::TooLarge => "too_large",
            Self::Unsupported => "unsupported",
            Self::RateLimited => "rate_limited",
            Self::Paused => "paused",
            Self::NotWritable => "not_writable",
            Self::Exists => "exists",
            Self::NotChangeable => "not_changeable",
            Self::TrashUnavailable => "trash_unavailable",
            Self::Conflict => "conflict",
            Self::NotLent => "not_lent",
            Self::NeedsApproval => "needs_approval",
            Self::BrowserUnavailable => "browser_unavailable",
            Self::BrowserStopped => "browser_stopped",
            Self::SiteNotAllowed => "site_not_allowed",
            Self::NotInGroup => "not_in_group",
            Self::NoTab => "no_tab",
            Self::SensitiveField => "sensitive_field",
            Self::WouldSubmit => "would_submit",
            Self::PageError => "page_error",
            Self::Internal => "internal",
        }
    }

    /// Whether the audit log records this as a policy denial rather than an
    /// ordinary error.
    pub fn is_denial(self) -> bool {
        matches!(
            self,
            Self::InvalidPath
                | Self::UnknownRoot
                | Self::OutsideRoot
                | Self::Denied
                | Self::RateLimited
                | Self::Paused
                | Self::NotWritable
                | Self::NotChangeable
                | Self::NotLent
                | Self::NeedsApproval
                | Self::BrowserStopped
                | Self::SiteNotAllowed
                | Self::NotInGroup
                | Self::SensitiveField
                | Self::WouldSubmit
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn invalid_path(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidPath, message)
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Denied, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}
