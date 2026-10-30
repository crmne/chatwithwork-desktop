//! The local audit log as the Activity page shows it: every request from
//! Chat with Work, and the daemon's own events, newest first, each as a
//! sentence, a detail line and a badge, grouped by day.

use serde::Serialize;

use crate::model::{AuditEntry, describe_tool};

/// One row of the Activity page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// "Today", "Yesterday", "Wednesday, Sep 23".
    pub day: String,
    /// "14:32:05".
    pub clock: String,
    pub text: String,
    pub detail: Option<String>,
    /// For tool calls: what came of it, and its tone (`positive`, `live`,
    /// `negative`, `attention`).
    pub badge: Option<(&'static str, &'static str)>,
    pub refused: bool,
}

/// The log, oldest first as the daemon sends it, as rows newest first.
pub fn rows(entries: &[AuditEntry]) -> Vec<Row> {
    entries
        .iter()
        .rev()
        .map(|entry| {
            let (text, detail) = describe(entry);
            Row {
                day: crate::time::day(&entry.ts),
                clock: crate::time::clock(&entry.ts),
                text,
                detail,
                badge: badge(entry),
                refused: refused(entry),
            }
        })
        .collect()
}

fn badge(entry: &AuditEntry) -> Option<(&'static str, &'static str)> {
    if entry.event != "tool" {
        return None;
    }
    Some(match entry.decision.as_deref() {
        Some("allowed") if entry.dry_run == Some(true) => ("Checked", "positive"),
        Some("allowed") if entry.effect.is_some() => ("Changed", "live"),
        Some("allowed") => ("Answered", "positive"),
        Some("denied") => ("Refused", "negative"),
        _ => ("Failed", "attention"),
    })
}

/// Refused means denied by policy; failed calls show only under All.
fn refused(entry: &AuditEntry) -> bool {
    entry.event == "tool" && entry.decision.as_deref() == Some("denied")
}

/// A sentence for the entry, and a detail line.
fn describe(entry: &AuditEntry) -> (String, Option<String>) {
    if entry.event == "tool" {
        let text = capitalize(&describe_tool(entry));
        let mut details = Vec::new();
        if let Some(reason) = &entry.reason {
            details.push(reason.clone());
        } else if let Some(code) = &entry.code {
            details.push(code.replace('_', " "));
        }
        if let Some(n) = entry.results {
            details.push(if n == 1 {
                "1 result".into()
            } else {
                format!("{n} results")
            });
        }
        if entry.dry_run == Some(true) {
            details.push("nothing changed".into());
        }
        if let Some(written) = entry.written {
            details.push(format!("{} written", size(written)));
        }
        if entry.trash.is_some() {
            details.push("the old version is in the trash".into());
        }
        if let Some(bytes) = entry.bytes {
            details.push(format!("{} sent", size(bytes)));
        }
        if let Some(chat) = &entry.chat_id {
            details.push(format!("chat {chat}"));
        }
        let detail = (!details.is_empty()).then(|| details.join(" · "));
        return (text, detail);
    }
    let text = match entry.event.as_str() {
        "started" => "The background agent started",
        "stopped" => "The background agent stopped",
        "connected" => "Connected to Chat with Work",
        "disconnected" => "Disconnected from Chat with Work",
        "paused" => "Sharing paused",
        "resumed" => "Sharing resumed",
        "reloaded" => "Settings applied",
        "exclusion_added" => "Custom exclusion added",
        "exclusion_changed" => "Custom exclusion changed",
        "exclusion_removed" => "Custom exclusion removed",
        "paired" => "Paired with Chat with Work",
        "logged_out" => "Pairing removed from this computer",
        "revoked" => "Chat with Work revoked this computer",
        other => return (capitalize(&other.replace('_', " ")), entry.detail.clone()),
    };
    (text.to_string(), entry.detail.clone())
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_as_sentences() {
        let read = AuditEntry {
            event: "tool".into(),
            tool: Some("read".into()),
            path: Some("docs:plans/q3.md".into()),
            decision: Some("allowed".into()),
            bytes: Some(2048),
            chat_id: Some("7".into()),
            ..AuditEntry::default()
        };
        assert_eq!(
            describe(&read),
            (
                "Read docs:plans/q3.md".into(),
                Some("2.0 KB sent · chat 7".into())
            )
        );
        assert!(!refused(&read));

        let denied = AuditEntry {
            event: "tool".into(),
            tool: Some("read".into()),
            path: Some("docs:.env".into()),
            decision: Some("denied".into()),
            code: Some("denied".into()),
            reason: Some("this path is on the deny list (.env*)".into()),
            ..AuditEntry::default()
        };
        assert!(refused(&denied));
        assert_eq!(
            describe(&denied).1.as_deref(),
            Some("this path is on the deny list (.env*)")
        );

        let paused = AuditEntry {
            event: "paused".into(),
            ..AuditEntry::default()
        };
        assert_eq!(describe(&paused).0, "Sharing paused");
        assert!(!refused(&paused));

        let moved = AuditEntry {
            event: "tool".into(),
            tool: Some("move".into()),
            path: Some("docs:a.md".into()),
            to: Some("docs:archive/a.md".into()),
            effect: Some("moved".into()),
            decision: Some("allowed".into()),
            ..AuditEntry::default()
        };
        assert_eq!(describe(&moved).0, "Moved docs:a.md to docs:archive/a.md");
        let replaced = AuditEntry {
            event: "tool".into(),
            tool: Some("write".into()),
            path: Some("docs:a.md".into()),
            effect: Some("replaced".into()),
            decision: Some("allowed".into()),
            written: Some(2048),
            trash: Some("/home/u/.local/share/Trash/files/a.md".into()),
            ..AuditEntry::default()
        };
        assert_eq!(
            describe(&replaced),
            (
                "Replaced docs:a.md".into(),
                Some("2.0 KB written · the old version is in the trash".into())
            )
        );
        let checked = AuditEntry {
            event: "tool".into(),
            tool: Some("delete".into()),
            path: Some("docs:old".into()),
            effect: Some("trashed".into()),
            dry_run: Some(true),
            decision: Some("allowed".into()),
            ..AuditEntry::default()
        };
        assert_eq!(
            describe(&checked),
            (
                "Checked a change to docs:old".into(),
                Some("nothing changed".into())
            )
        );

        let failed = AuditEntry {
            event: "tool".into(),
            tool: Some("read".into()),
            decision: Some("error".into()),
            ..AuditEntry::default()
        };
        assert!(!refused(&failed), "a failure isn't a refusal");
        assert_eq!(badge(&failed), Some(("Failed", "attention")));
        assert_eq!(badge(&checked), Some(("Checked", "positive")));
        assert_eq!(badge(&replaced), Some(("Changed", "live")));
        assert_eq!(badge(&denied), Some(("Refused", "negative")));
        assert_eq!(badge(&paused), None);
        let rows = rows(&[paused, denied]);
        assert_eq!(rows[0].text, "Read docs:.env", "newest first");
        assert!(rows[0].refused);
    }
}
