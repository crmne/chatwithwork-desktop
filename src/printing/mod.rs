//! Printing: a PDF from a chat's files on a printer this computer's owner
//! allowed (docs/printing.md, PROTOCOL.md section 8.21).
//!
//! Printers are the system's: CUPS on Linux and macOS, the print spooler on
//! Windows. The owner chooses here which of them Chat with Work may use, for
//! their own chats and for each bot, and whether a bot may print on one
//! without a person approving each job; the server can't change any of it.
//!
//! The confined worker can't start `lp` or reach the spooler, so, like
//! commands, a print is the supervisor's to do: it reads the config again,
//! finds the printer and the file by their IDs, keeps the count of pages
//! printed in the last day, and trusts nothing else it's told.

#[cfg(unix)]
mod cups;
pub mod settings;
pub mod tools;
pub(crate) mod transfer;
#[cfg(windows)]
mod windows;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::commands::RunError;
use crate::config::{Config, Printer};
use crate::paths::{Paths, write_private_file};

/// A rolling day, for the page limit and the receipts of jobs printed.
const DAY_SECONDS: u64 = 24 * 60 * 60;

/// A printer as the system knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPrinter {
    /// The system's name for it, as `lp -d` or the spooler takes it.
    pub queue: String,
    /// What the system calls it for people, where that differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
    /// `idle`, `printing`, or `stopped`.
    pub state: String,
    #[serde(default)]
    pub duplex: bool,
    #[serde(default)]
    pub color: bool,
}

/// One job, as the worker asks for it. `transfer` names a file it finished
/// receiving; the printer and the transfer are looked up, never trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub printer: String,
    pub transfer: String,
    pub title: String,
    pub copies: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplex: Option<bool>,
    /// A page list such as `1-3,7`, already checked against the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    /// Pages one copy prints, for the day's limit.
    pub pages_counted: u32,
    /// The bot it's for; none for a person's chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    /// Nobody approved it: only where the bot's loan of this printer says so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unattended: bool,
}

/// How a job went: kept as a receipt, so a call made twice prints once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub job: String,
    /// `queued`, or `spooling` while Windows still draws the pages.
    pub state: String,
    pub pages: u32,
    pub copies: u32,
    /// Set on an answer from a receipt: this job printed before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
}

/// One job on its way to the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    pub queue: String,
    pub file: PathBuf,
    pub title: String,
    pub copies: u32,
    pub duplex: Option<bool>,
    pub pages: Option<String>,
}

/// The printers this computer's system has.
pub fn system_printers() -> Result<Vec<SystemPrinter>, RunError> {
    #[cfg(unix)]
    {
        cups::printers()
    }
    #[cfg(windows)]
    {
        windows::printers()
    }
}

/// A job that may have printed: the system stopped answering partway.
pub(crate) const UNCONFIRMED: &str = "unconfirmed";

fn submit(job: &Job) -> Result<String, RunError> {
    #[cfg(unix)]
    {
        cups::submit(job)
    }
    #[cfg(windows)]
    {
        windows::submit(job)
    }
}

/// Where print files, receipts and the day's count live.
fn dir(paths: &Paths) -> PathBuf {
    paths.state_dir.join("printing")
}

/// Whether `caller` may print on `printer`: a person's chats where the owner
/// allowed chats, a bot where it's lent to it.
pub fn permitted(printer: &Printer, bot: Option<&str>) -> bool {
    match bot {
        Some(bot) => printer.loan(bot).is_some(),
        None => printer.chats,
    }
}

/// One job at a time, from the receipt's check to its writing, so a call made
/// again while Windows still draws the first prints nothing, and the day's
/// count holds.
static JOBS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Do one job, as the supervisor does. Every check is made here again.
/// Files nobody printed leave within minutes of their time, whether or not
/// anything else arrives. Ends with the daemon.
pub async fn purge_print_files(dir: PathBuf) {
    loop {
        let folder = dir.clone();
        let _ = tokio::task::spawn_blocking(move || transfer::purge(&folder)).await;
        tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
    }
}

/// One job at a time. Held from before the settings are read for a job, so a
/// job that waited still answers to the settings of its turn.
pub fn one_job_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    JOBS.lock().unwrap_or_else(|held| held.into_inner())
}

pub fn print(paths: &Paths, config: &Config, request: &Request) -> Result<Receipt, RunError> {
    print_through(paths, config, request, submit)
}

fn print_through(
    paths: &Paths,
    config: &Config,
    request: &Request,
    submit: impl FnOnce(&Job) -> Result<String, RunError>,
) -> Result<Receipt, RunError> {
    if !transfer::valid_id(&request.transfer) {
        return Err(RunError::new("invalid_argument", "not a print file's id"));
    }
    let printer = config
        .printers
        .iter()
        .find(|printer| printer.id == request.printer)
        .filter(|printer| permitted(printer, request.bot.as_deref()))
        .ok_or_else(|| RunError::new("not_lent", "that printer isn't allowed on this computer"))?;
    if request.unattended
        && !request
            .bot
            .as_deref()
            .and_then(|bot| printer.loan(bot))
            .is_some_and(|loan| loan.unattended)
    {
        return Err(RunError::new(
            "needs_approval",
            "a person has to approve this bot's prints on this printer",
        ));
    }
    let dir = dir(paths);
    transfer::purge(&dir);
    if let Some(receipt) = read_receipt(&dir, &request.transfer) {
        if receipt.state == UNCONFIRMED {
            return Err(RunError::new(
                UNCONFIRMED,
                "this job may have printed, so it won't go out again: check the printer",
            ));
        }
        return Ok(Receipt {
            already: true,
            ..receipt
        });
    }
    let file = transfer::finished(&dir, &request.transfer)
        .ok_or_else(|| RunError::new("not_found", "that file hasn't arrived here yet"))?;
    check_counts(request)?;
    let sheets = request.pages_counted.saturating_mul(request.copies);
    let limit = config.limits.print_pages_per_day;
    let now = now();
    let mut counted = read_count(&dir, now);
    if limit > 0 && counted.total().saturating_add(sheets) > limit {
        return Err(RunError::new(
            "rate_limited",
            format!(
                "this computer prints at most {limit} pages a day, and {} are left today",
                limit.saturating_sub(counted.total())
            ),
        ));
    }
    // Counted first, and kept unless the job certainly didn't print: paper
    // that isn't counted is what the limit is there to stop. A receipt goes
    // first too: a job that may have printed never goes out a second time.
    counted.0.push((now, sheets));
    write_count(&dir, &counted)?;
    let mut receipt = Receipt {
        job: String::new(),
        state: UNCONFIRMED.into(),
        pages: request.pages_counted,
        copies: request.copies,
        already: false,
    };
    write_receipt(&dir, &request.transfer, &receipt)?;
    let job = submit(&Job {
        queue: printer.queue.clone(),
        file: file.clone(),
        title: request.title.clone(),
        copies: request.copies,
        duplex: request.duplex,
        pages: request.pages.clone(),
    })
    .inspect_err(|error| {
        if error.code != UNCONFIRMED {
            let _ = std::fs::remove_file(receipt_path(&dir, &request.transfer));
            counted.0.pop();
            let _ = write_count(&dir, &counted);
        }
    })?;
    receipt.job = job;
    receipt.state = "queued".into();
    let _ = write_receipt(&dir, &request.transfer, &receipt);
    let _ = std::fs::remove_file(file);
    Ok(receipt)
}

/// What a job says it prints, as far as it can be told without reading the
/// PDF, which stays in the confined worker: the day's limit counts this.
fn check_counts(request: &Request) -> Result<(), RunError> {
    let invalid = |message: &str| RunError::new("invalid_argument", message);
    if !(1..=tools::MAX_COPIES).contains(&request.copies) {
        return Err(invalid("that's not a number of copies"));
    }
    if request.pages_counted == 0 {
        return Err(invalid("a job prints at least one page"));
    }
    if let Some(pages) = &request.pages {
        let ranges = tools::parse_pages(pages).map_err(|_| invalid("that's not a page list"))?;
        let listed =
            tools::count_pages(&ranges, u32::MAX).map_err(|_| invalid("that's not a page list"))?;
        if request.pages_counted < listed {
            return Err(invalid("that page list is longer than the job says"));
        }
    }
    Ok(())
}

/// Pages this computer may still print today; `None` without a limit.
pub fn pages_left(paths: &Paths, config: &Config) -> Option<u32> {
    let limit = config.limits.print_pages_per_day;
    (limit > 0).then(|| limit.saturating_sub(read_count(&dir(paths), now()).total()))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pages printed in the last day, by when.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Count(Vec<(u64, u32)>);

impl Count {
    fn total(&self) -> u32 {
        self.0
            .iter()
            .fold(0u32, |total, (_, pages)| total.saturating_add(*pages))
    }
}

fn read_count(dir: &Path, now: u64) -> Count {
    let mut count: Count = std::fs::read(dir.join("pages.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    count
        .0
        .retain(|(at, _)| now.saturating_sub(*at) < DAY_SECONDS);
    count
}

fn write_count(dir: &Path, count: &Count) -> Result<(), RunError> {
    serde_json::to_vec(count)
        .ok()
        .and_then(|bytes| write_private_file(&dir.join("pages.json"), &bytes).ok())
        .ok_or_else(|| RunError::new("unavailable", "couldn't save today's page count"))
}

fn receipt_path(dir: &Path, transfer: &str) -> PathBuf {
    dir.join(format!("{transfer}.receipt"))
}

fn read_receipt(dir: &Path, transfer: &str) -> Option<Receipt> {
    let path = receipt_path(dir, transfer);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_receipt(dir: &Path, transfer: &str, receipt: &Receipt) -> Result<(), RunError> {
    serde_json::to_vec(receipt)
        .ok()
        .and_then(|bytes| write_private_file(&receipt_path(dir, transfer), &bytes).ok())
        .ok_or_else(|| RunError::new("unavailable", "couldn't save the job's receipt"))
}

#[cfg(test)]
mod tests;
