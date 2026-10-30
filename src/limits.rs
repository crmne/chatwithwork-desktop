//! Daemon-side rate and volume limits.
//!
//! Read budgets and any locally configured call/change quotas hold regardless
//! of the server. Calls and changes have no timed quotas by default.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::Limits;
use crate::error::{ErrorCode, ToolError};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
/// Chats tracked for per-chat budgets; the oldest are forgotten first.
const MAX_TRACKED_CHATS: usize = 1024;

/// Whose read budget a read counts against: every folder's shared one
/// (`[limits]`), or a folder's own (`read_budget` on a root), which allows
/// `per_chat` characters a chat and twice that for all chats together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReadScope {
    Global,
    Folder { id: String, per_chat: usize },
}

impl ReadScope {
    fn key(&self) -> Option<String> {
        match self {
            Self::Global => None,
            Self::Folder { id, .. } => Some(id.clone()),
        }
    }

    /// The per-chat and all-chats budgets.
    fn budgets(&self, limits: &Limits) -> (usize, usize) {
        match self {
            Self::Global => (limits.read_chars_per_chat_hour, limits.read_chars_per_hour),
            Self::Folder { per_chat, .. } => (*per_chat, per_chat.saturating_mul(2)),
        }
    }
}

type Window = VecDeque<(Instant, usize)>;

#[derive(Default)]
struct Windows {
    calls: VecDeque<Instant>,
    /// Characters read, per scope (`None` is the shared budget).
    reads: HashMap<Option<String>, Window>,
    /// Characters read, per scope and chat.
    chat_reads: HashMap<(Option<String>, String), Window>,
    /// Changes made, with the bytes each wrote.
    changes: VecDeque<(Instant, u64)>,
    /// Bytes `read_bytes` returned.
    copies: VecDeque<(Instant, u64)>,
}

pub struct Limiter {
    limits: Mutex<Limits>,
    windows: Mutex<Windows>,
}

impl Limiter {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits: Mutex::new(limits),
            windows: Mutex::default(),
        }
    }

    pub fn set_limits(&self, limits: Limits) {
        *self.limits.lock().expect("limits") = limits;
    }

    /// Count one tool call against the per-minute limit.
    pub fn check_call(&self) -> Result<(), ToolError> {
        self.check_call_at(Instant::now())
    }

    fn check_call_at(&self, now: Instant) -> Result<(), ToolError> {
        let max = self.limits.lock().expect("limits").calls_per_minute as usize;
        if max == 0 {
            return Ok(());
        }
        let mut w = self.windows.lock().expect("windows");
        prune(&mut w.calls, now, MINUTE, |t| *t);
        if w.calls.len() >= max {
            // The call whose expiry brings the count under `max` again.
            let frees_in = w
                .calls
                .get(w.calls.len() - max)
                .map_or(MINUTE, |t| MINUTE.saturating_sub(now.duration_since(*t)));
            return Err(ToolError::new(
                ErrorCode::RateLimited,
                format!(
                    "This computer takes {max} tool calls a minute, and they are used up. {}",
                    wait_message(frees_in)
                ),
            ));
        }
        w.calls.push_back(now);
        Ok(())
    }

    /// How many characters one `read` may return right now for `chat`,
    /// from `scope`: what is left of its hourly budgets, at most a call's
    /// worth. Refused when nothing is left.
    pub fn read_allowance(&self, chat: &str, scope: &ReadScope) -> Result<usize, ToolError> {
        self.read_allowance_at(chat, scope, Instant::now())
    }

    fn read_allowance_at(
        &self,
        chat: &str,
        scope: &ReadScope,
        now: Instant,
    ) -> Result<usize, ToolError> {
        let per_call = self.limits.lock().expect("limits").read_chars_per_call;
        let allowance = per_call.min(self.read_remaining_at(chat, scope, now));
        if allowance == 0 {
            return Err(self.read_budget_used_up_at(chat, scope, now));
        }
        Ok(allowance)
    }

    /// Characters `chat` may still read from `scope` in this rolling hour,
    /// with no cap per call: what `grep`, `glob` and the diffs of a patch
    /// may return, cut where it runs out.
    pub fn read_remaining(&self, chat: &str, scope: &ReadScope) -> usize {
        self.read_remaining_at(chat, scope, Instant::now())
    }

    fn read_remaining_at(&self, chat: &str, scope: &ReadScope, now: Instant) -> usize {
        let (per_chat, all_chats) = scope.budgets(&self.limits.lock().expect("limits"));
        let key = scope.key();
        let mut w = self.windows.lock().expect("windows");
        let total: usize = match w.reads.get_mut(&key) {
            Some(q) => {
                prune(q, now, HOUR, |(t, _)| *t);
                q.iter().map(|(_, n)| n).sum()
            }
            None => 0,
        };
        let chat_used: usize = match w.chat_reads.get_mut(&(key, chat.to_string())) {
            Some(q) => {
                prune(q, now, HOUR, |(t, _)| *t);
                q.iter().map(|(_, n)| n).sum()
            }
            None => 0,
        };
        all_chats
            .saturating_sub(total)
            .min(per_chat.saturating_sub(chat_used))
    }

    /// The refusal for a read from `scope` once nothing is left of its
    /// budget for `chat`, saying when it frees up.
    pub fn read_budget_used_up(&self, chat: &str, scope: &ReadScope) -> ToolError {
        self.read_budget_used_up_at(chat, scope, Instant::now())
    }

    fn read_budget_used_up_at(&self, chat: &str, scope: &ReadScope, now: Instant) -> ToolError {
        let (per_chat, all_chats) = scope.budgets(&self.limits.lock().expect("limits"));
        let key = scope.key();
        let w = self.windows.lock().expect("windows");
        // How long until the oldest read in `q` leaves the hour, when `q`
        // has used up `budget`.
        // How long until enough reads in `q` leave the hour for it to fall
        // below `budget` again, when it has used it up.
        let frees = |q: Option<&Window>, budget: usize| {
            let q = q?;
            let mut used: usize = q.iter().map(|(_, n)| n).sum();
            let mut reads = q.iter();
            let mut frees_at = None;
            while used >= budget {
                let (t, n) = reads.next()?;
                used -= n;
                frees_at = Some(HOUR.saturating_sub(now.duration_since(*t)));
            }
            frees_at
        };
        let frees_in = frees(w.chat_reads.get(&(key.clone(), chat.to_string())), per_chat)
            .into_iter()
            .chain(frees(w.reads.get(&key), all_chats))
            .max()
            .unwrap_or(HOUR);
        let what = match scope {
            ReadScope::Global => "The hourly reading allowance on this computer is used up.",
            ReadScope::Folder { .. } => "The hourly reading allowance for this folder is used up.",
        };
        ToolError::new(
            ErrorCode::RateLimited,
            format!(
                "{what} {} The person can raise a folder's allowance on the computer \
                 (cww roots read-budget).",
                wait_message(frees_in)
            ),
        )
    }

    /// Record characters returned by a read from `scope`.
    pub fn record_read(&self, chat: &str, scope: &ReadScope, chars: usize) {
        self.record_read_at(chat, scope, chars, Instant::now());
    }

    fn record_read_at(&self, chat: &str, scope: &ReadScope, chars: usize, now: Instant) {
        if chars == 0 {
            return;
        }
        let key = scope.key();
        let mut w = self.windows.lock().expect("windows");
        w.reads
            .entry(key.clone())
            .or_default()
            .push_back((now, chars));
        let chat_key = (key, chat.to_string());
        if !w.chat_reads.contains_key(&chat_key) && w.chat_reads.len() >= MAX_TRACKED_CHATS {
            w.chat_reads
                .retain(|_, q| q.back().is_some_and(|(t, _)| now.duration_since(*t) < HOUR));
        }
        w.chat_reads
            .entry(chat_key)
            .or_default()
            .push_back((now, chars));
    }

    /// Bytes `read_bytes` may return right now, across all chats, at most
    /// `wanted`. Refused when the hour's copy budget is used up.
    pub fn copy_allowance(&self, wanted: u64) -> Result<u64, ToolError> {
        self.copy_allowance_at(wanted, Instant::now())
    }

    fn copy_allowance_at(&self, wanted: u64, now: Instant) -> Result<u64, ToolError> {
        let max = self.limits.lock().expect("limits").copy_bytes_per_hour;
        let mut w = self.windows.lock().expect("windows");
        prune(&mut w.copies, now, HOUR, |(t, _)| *t);
        let used: u64 = w.copies.iter().map(|(_, n)| n).sum();
        let left = max.saturating_sub(used);
        if left == 0 {
            return Err(ToolError::new(
                ErrorCode::RateLimited,
                format!(
                    "This computer lets {} MB of files be copied an hour, and they are used up. \
                     Try again later.",
                    max / (1024 * 1024)
                ),
            ));
        }
        Ok(left.min(wanted))
    }

    /// Record bytes `read_bytes` returned.
    pub fn record_copy(&self, bytes: u64) {
        self.record_copy_at(bytes, Instant::now());
    }

    fn record_copy_at(&self, bytes: u64, now: Instant) {
        if bytes > 0 {
            let mut w = self.windows.lock().expect("windows");
            w.copies.push_back((now, bytes));
        }
    }
}

/// When a budget frees up, and what the caller should do until then: a
/// model that keeps calling only spends its turns on refusals.
pub fn wait_message(frees_in: Duration) -> String {
    let seconds = frees_in.as_secs().max(1);
    let when = if seconds < 90 {
        format!("in about {seconds} seconds")
    } else {
        format!("in about {} minutes", seconds.div_ceil(60))
    };
    format!(
        "It frees up {when}: stop calling this computer's tools until then, and tell the person."
    )
}

impl Limiter {
    /// Count one change that will write `bytes`, against the change budgets
    /// (separate from the read budgets). Refused when any is used up.
    pub fn check_change(&self, bytes: u64) -> Result<(), ToolError> {
        self.check_change_at(bytes, Instant::now())
    }

    fn check_change_at(&self, bytes: u64, now: Instant) -> Result<(), ToolError> {
        self.check_changes_at(&[bytes], now)
    }

    /// Count several changes at once, one per entry of `bytes` (what each
    /// writes): all of them, or none when they don't all fit.
    pub fn check_changes(&self, bytes: &[u64]) -> Result<(), ToolError> {
        self.check_changes_at(bytes, Instant::now())
    }

    fn check_changes_at(&self, bytes: &[u64], now: Instant) -> Result<(), ToolError> {
        let limits = self.limits.lock().expect("limits").clone();
        if limits.changes_per_minute == 0
            && limits.changes_per_day == 0
            && limits.change_bytes_per_hour == 0
        {
            return Ok(());
        }
        let count = bytes.len();
        let total = bytes
            .iter()
            .fold(0_u64, |sum, bytes| sum.saturating_add(*bytes));
        let mut w = self.windows.lock().expect("windows");
        prune(&mut w.changes, now, DAY, |(t, _)| *t);
        let within = |window: Duration| {
            w.changes
                .iter()
                .filter(move |(t, _)| now.duration_since(*t) < window)
        };
        let last_minute = within(MINUTE).count();
        let last_day = w.changes.len();
        let last_hour_bytes: u64 = within(HOUR).map(|(_, b)| b).sum();
        if limits.changes_per_minute > 0 && last_minute + count > limits.changes_per_minute as usize
        {
            return Err(ToolError::new(
                ErrorCode::RateLimited,
                format!(
                    "This computer allows {} changes a minute. Try again shortly.",
                    limits.changes_per_minute
                ),
            ));
        }
        if limits.changes_per_day > 0 && last_day + count > limits.changes_per_day as usize {
            return Err(ToolError::new(
                ErrorCode::RateLimited,
                format!(
                    "This computer allows {} changes a day, and they are used up. Try again \
                     later.",
                    limits.changes_per_day
                ),
            ));
        }
        if limits.change_bytes_per_hour > 0
            && last_hour_bytes.saturating_add(total) > limits.change_bytes_per_hour
        {
            return Err(ToolError::new(
                ErrorCode::RateLimited,
                format!(
                    "This computer allows changes to write {} MB an hour, and this one would go \
                     over. Try again later.",
                    limits.change_bytes_per_hour / (1024 * 1024)
                ),
            ));
        }
        w.changes.extend(bytes.iter().map(|b| (now, *b)));
        Ok(())
    }
}

fn prune<T>(q: &mut VecDeque<T>, now: Instant, window: Duration, at: impl Fn(&T) -> Instant) {
    while q
        .front()
        .is_some_and(|x| now.duration_since(at(x)) >= window)
    {
        q.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: ReadScope = ReadScope::Global;

    fn limits() -> Limits {
        Limits {
            calls_per_minute: 3,
            read_chars_per_call: 100,
            read_chars_per_chat_hour: 250,
            read_chars_per_hour: 400,
            ..Limits::default()
        }
    }

    #[test]
    fn limits_calls_per_minute() {
        let l = Limiter::new(limits());
        let t0 = Instant::now();
        for _ in 0..3 {
            l.check_call_at(t0).unwrap();
        }
        assert_eq!(
            l.check_call_at(t0).unwrap_err().code,
            ErrorCode::RateLimited
        );
        l.check_call_at(t0 + MINUTE).unwrap();
    }

    #[test]
    fn default_calls_and_changes_do_not_stop_a_coding_session() {
        let l = Limiter::new(Limits::default());
        let now = Instant::now();
        // More calls, changes and bytes than any former default budget,
        // without advancing time to let a window expire.
        for _ in 0..1000 {
            l.check_call_at(now).unwrap();
            l.check_changes_at(&[128 * 1024, 128 * 1024], now).unwrap();
        }
        let windows = l.windows.lock().unwrap();
        assert!(windows.calls.is_empty());
        assert!(windows.changes.is_empty());
    }

    #[test]
    fn change_limits_can_be_enabled_independently() {
        // Opting into one limit must not make the other zero values
        // refuse everything.
        let now = Instant::now();
        for limits in [
            Limits {
                changes_per_minute: 2,
                ..Limits::default()
            },
            Limits {
                changes_per_day: 2,
                ..Limits::default()
            },
            Limits {
                change_bytes_per_hour: 2,
                ..Limits::default()
            },
        ] {
            let l = Limiter::new(limits);
            l.check_changes_at(&[1, 1], now).unwrap();
            assert_eq!(
                l.check_change_at(1, now).unwrap_err().code,
                ErrorCode::RateLimited
            );
        }
    }

    #[test]
    fn budgets_changes_apart_from_reads() {
        let l = Limiter::new(Limits {
            changes_per_minute: 2,
            changes_per_day: 3,
            change_bytes_per_hour: 100,
            ..limits()
        });
        let t0 = Instant::now();
        l.check_change_at(10, t0).unwrap();
        l.check_change_at(10, t0).unwrap();
        let err = l.check_change_at(10, t0).unwrap_err();
        assert_eq!(err.code, ErrorCode::RateLimited);
        assert!(
            err.message.contains("2 changes a minute"),
            "{}",
            err.message
        );
        // Too many bytes for the hour.
        let err = l.check_change_at(81, t0 + MINUTE).unwrap_err();
        assert!(err.message.contains("an hour"), "{}", err.message);
        l.check_change_at(80, t0 + MINUTE).unwrap();
        // Three a day.
        let err = l.check_change_at(0, t0 + HOUR).unwrap_err();
        assert!(err.message.contains("3 changes a day"), "{}", err.message);
        l.check_change_at(0, t0 + DAY).unwrap();
        // Reads are a separate budget.
        assert_eq!(l.read_allowance_at("a", &G, t0).unwrap(), 100);
    }

    #[test]
    fn budgets_reads_per_chat_and_total() {
        let l = Limiter::new(limits());
        let t0 = Instant::now();
        assert_eq!(l.read_allowance_at("a", &G, t0).unwrap(), 100);
        l.record_read_at("a", &G, 100, t0);
        l.record_read_at("a", &G, 100, t0);
        assert_eq!(l.read_allowance_at("a", &G, t0).unwrap(), 50);
        l.record_read_at("a", &G, 50, t0);
        assert_eq!(
            l.read_allowance_at("a", &G, t0).unwrap_err().code,
            ErrorCode::RateLimited
        );
        // Another chat still has room, up to the global budget.
        assert_eq!(l.read_allowance_at("b", &G, t0).unwrap(), 100);
        l.record_read_at("b", &G, 100, t0);
        assert_eq!(l.read_allowance_at("b", &G, t0).unwrap(), 50);
        // An hour later everything is free again.
        assert_eq!(l.read_allowance_at("a", &G, t0 + HOUR).unwrap(), 100);
    }

    #[test]
    fn a_folder_with_its_own_budget_counts_apart() {
        let l = Limiter::new(limits());
        let t0 = Instant::now();
        let code = ReadScope::Folder {
            id: "code".into(),
            per_chat: 300,
        };
        // The shared budget runs out for chat a...
        l.record_read_at("a", &G, 250, t0);
        assert_eq!(
            l.read_allowance_at("a", &G, t0).unwrap_err().code,
            ErrorCode::RateLimited
        );
        // ...while the folder's own still has all of its 300.
        assert_eq!(l.read_remaining_at("a", &code, t0), 300);
        assert_eq!(l.read_allowance_at("a", &code, t0).unwrap(), 100);
        l.record_read_at("a", &code, 300, t0);
        let err = l.read_allowance_at("a", &code, t0).unwrap_err();
        assert!(err.message.contains("this folder"), "{}", err.message);
        // All chats together get twice the folder's budget.
        assert_eq!(l.read_remaining_at("b", &code, t0), 300);
        l.record_read_at("b", &code, 200, t0);
        assert_eq!(l.read_remaining_at("c", &code, t0), 100);
        // Reads from the folder didn't touch the shared budget.
        assert_eq!(l.read_remaining_at("b", &G, t0), 150);
        assert_eq!(l.read_remaining_at("a", &code, t0 + HOUR), 300);
    }

    #[test]
    fn a_used_up_budget_says_when_it_frees_up_and_to_stop_until_then() {
        let l = Limiter::new(limits());
        let t0 = Instant::now();
        l.record_read_at("a", &G, 250, t0);
        let err = l.read_allowance_at("a", &G, t0 + 40 * MINUTE).unwrap_err();
        assert!(
            err.message.contains("in about 20 minutes"),
            "{}",
            err.message
        );
        assert!(err.message.contains("stop calling"), "{}", err.message);
        let calls = Limiter::new(Limits {
            calls_per_minute: 1,
            ..limits()
        });
        // With the limit lowered under calls already made, it frees up only
        // once enough of them have left the minute.
        let lowered = Limiter::new(limits());
        for s in [0, 10, 20] {
            lowered.check_call_at(t0 + Duration::from_secs(s)).unwrap();
        }
        lowered.set_limits(Limits {
            calls_per_minute: 1,
            ..limits()
        });
        let err = lowered
            .check_call_at(t0 + Duration::from_secs(30))
            .unwrap_err();
        assert!(
            err.message.contains("in about 50 seconds"),
            "{}",
            err.message
        );
        calls.check_call_at(t0).unwrap();
        let err = calls
            .check_call_at(t0 + Duration::from_secs(15))
            .unwrap_err();
        assert!(
            err.message.contains("in about 45 seconds"),
            "{}",
            err.message
        );
        // A read that leaves the hour without bringing it under the budget
        // doesn't free it: the one that does, later, is when.
        let l = Limiter::new(limits());
        l.record_read_at("b", &G, 50, t0);
        l.record_read_at("b", &G, 250, t0 + 30 * MINUTE);
        let err = l.read_allowance_at("b", &G, t0 + 40 * MINUTE).unwrap_err();
        assert!(
            err.message.contains("in about 50 minutes"),
            "{}",
            err.message
        );
    }

    #[test]
    fn counts_several_changes_at_once_or_none() {
        let l = Limiter::new(Limits {
            changes_per_minute: 5,
            changes_per_day: 100,
            change_bytes_per_hour: 100,
            ..limits()
        });
        let t0 = Instant::now();
        l.check_changes_at(&[10, 10, 10], t0).unwrap();
        // Three more don't fit in the minute's five: none is counted.
        let err = l.check_changes_at(&[1, 1, 1], t0).unwrap_err();
        assert!(
            err.message.contains("5 changes a minute"),
            "{}",
            err.message
        );
        l.check_changes_at(&[1, 1], t0).unwrap();
        // Bytes count together too.
        let err = l.check_changes_at(&[40, 40], t0 + MINUTE).unwrap_err();
        assert!(err.message.contains("an hour"), "{}", err.message);
        l.check_changes_at(&[34, 34], t0 + MINUTE).unwrap();
    }

    #[test]
    fn budgets_copied_bytes_per_hour() {
        let l = Limiter::new(Limits {
            copy_bytes_per_hour: 1000,
            ..limits()
        });
        let t0 = Instant::now();
        assert_eq!(l.copy_allowance_at(600, t0).unwrap(), 600);
        l.record_copy_at(600, t0);
        assert_eq!(l.copy_allowance_at(600, t0).unwrap(), 400);
        l.record_copy_at(400, t0);
        assert_eq!(
            l.copy_allowance_at(1, t0).unwrap_err().code,
            ErrorCode::RateLimited
        );
        // Reads are another budget.
        assert_eq!(l.read_allowance_at("a", &G, t0).unwrap(), 100);
        assert_eq!(l.copy_allowance_at(600, t0 + HOUR).unwrap(), 600);
    }
}
