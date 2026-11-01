//! The printing tools: `printers` lists what this caller may print on,
//! `print_bytes` receives a file a chunk at a time (no model is offered it),
//! and `print` prints it. A dry run of `print` needs no file: it says the
//! printer is there and allowed, for the approval card, so nothing leaves
//! Chat with Work before a person approves it.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::transfer::{self, Chunk};
use super::{Request, SystemPrinter, permitted};
use crate::commands::{RunError, Runner};
use crate::config::Printer;
use crate::error::{ErrorCode, ToolError};
use crate::tools::Caller;

pub const PRINT_TOOLS: [&str; 3] = ["printers", "print", "print_bytes"];

/// Copies of one job.
pub const MAX_COPIES: u32 = 99;

#[derive(Clone)]
pub struct PrintTools {
    pub runner: Runner,
    pub printers: Vec<Printer>,
    /// Where files on their way are kept (`crate::printing`).
    pub dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrintArgs {
    printer: String,
    #[serde(default)]
    transfer: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    copies: Option<u32>,
    #[serde(default)]
    duplex: Option<bool>,
    #[serde(default)]
    pages: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BytesArgs {
    printer: String,
    transfer: String,
    offset: u64,
    size: u64,
    sha256: String,
    data: String,
}

impl PrintTools {
    pub fn offered(&self) -> bool {
        !self.printers.is_empty() && !matches!(self.runner, Runner::Off(_))
    }

    fn printer(&self, id: &str, caller: &Caller) -> Result<&Printer, ToolError> {
        let bot = bot_of(caller);
        self.printers
            .iter()
            .find(|printer| printer.id == id)
            .filter(|printer| permitted(printer, bot))
            .ok_or_else(|| {
                ToolError::new(
                    if bot.is_some() {
                        ErrorCode::NotLent
                    } else {
                        ErrorCode::NotFound
                    },
                    format!(
                        "the printer {id} isn't {} on this computer (see `printers`)",
                        if bot.is_some() {
                            "lent to this bot"
                        } else {
                            "allowed for chats"
                        }
                    ),
                )
            })
    }

    /// Refuse a bot's job nobody approved where its loan doesn't allow that.
    fn check_unattended(&self, printer: &Printer, caller: &Caller) -> Result<(), ToolError> {
        if let Caller::Bot {
            id,
            unattended: true,
        } = caller
            && !printer.loan(id).is_some_and(|loan| loan.unattended)
        {
            return Err(ToolError::new(
                ErrorCode::NeedsApproval,
                format!(
                    "a person has to approve this bot's prints on {}",
                    printer.label
                ),
            ));
        }
        Ok(())
    }

    /// Run one of the tools. Returns the result and, for the audit log, what
    /// happened, never the file.
    pub async fn call(
        &self,
        name: &str,
        args: Value,
        caller: &Caller,
    ) -> Result<(Value, Option<String>), ToolError> {
        match name {
            "printers" => self.list(caller).await.map(|value| (value, None)),
            "print_bytes" => {
                let args: BytesArgs = parse(args)?;
                let printer = self.printer(&args.printer, caller)?;
                self.check_unattended(printer, caller)?;
                let dir = self.dir.clone();
                let received = tokio::task::spawn_blocking(move || {
                    transfer::receive(
                        &dir,
                        &Chunk {
                            transfer: &args.transfer,
                            offset: args.offset,
                            size: args.size,
                            sha256: &args.sha256,
                            data: &args.data,
                        },
                    )
                })
                .await
                .map_err(|_| ToolError::new(ErrorCode::Internal, "couldn't take the file"))??;
                Ok((serde_json::to_value(received).unwrap_or(Value::Null), None))
            }
            "print" => self.print(parse(args)?, caller).await,
            _ => unreachable!("checked against the tool names"),
        }
    }

    async fn list(&self, caller: &Caller) -> Result<Value, ToolError> {
        let (system, pages_left) = self.runner.printers().await.map_err(tool_error)?;
        let bot = bot_of(caller);
        let printers: Vec<Value> = self
            .printers
            .iter()
            .filter(|printer| permitted(printer, bot))
            .map(|printer| {
                let found = system.iter().find(|p| p.queue == printer.queue);
                let mut value = describe(printer, found);
                if let Some(bot) = bot {
                    value["unattended"] =
                        json!(printer.loan(bot).is_some_and(|loan| loan.unattended));
                }
                value
            })
            .collect();
        Ok(json!({ "printers": printers, "pages_left_today": pages_left }))
    }

    async fn print(
        &self,
        args: PrintArgs,
        caller: &Caller,
    ) -> Result<(Value, Option<String>), ToolError> {
        let printer = self.printer(&args.printer, caller)?.clone();
        self.check_unattended(&printer, caller)?;
        let copies = args.copies.unwrap_or(1);
        if !(1..=MAX_COPIES).contains(&copies) {
            return Err(ToolError::invalid_argument(format!(
                "`copies` is 1 to {MAX_COPIES}"
            )));
        }
        let ranges = args.pages.as_deref().map(parse_pages).transpose()?;
        if args.dry_run {
            let (system, pages_left) = self.runner.printers().await.map_err(tool_error)?;
            let found = system.iter().find(|p| p.queue == printer.queue);
            let Some(found) = found else {
                return Err(gone(&printer));
            };
            if found.state == "stopped" {
                return Err(ToolError::new(
                    ErrorCode::PrinterUnavailable,
                    format!("{} is stopped on this computer", printer.label),
                ));
            }
            if args.duplex == Some(true) && !found.duplex {
                return Err(ToolError::invalid_argument(format!(
                    "{} doesn't print on both sides",
                    printer.label
                )));
            }
            let mut preview = describe(&printer, Some(found));
            preview["dry_run"] = json!(true);
            preview["copies"] = json!(copies);
            preview["duplex_asked"] = json!(args.duplex);
            preview["pages_left_today"] = json!(pages_left);
            return Ok((preview, Some(format!("checked {}", printer.label))));
        }
        let transfer = args
            .transfer
            .filter(|id| transfer::valid_id(id))
            .ok_or_else(|| ToolError::invalid_argument("`transfer` names a file sent to print"))?;

        let dir = self.dir.clone();
        let id = transfer.clone();
        let counted = tokio::task::spawn_blocking(move || match transfer::finished(&dir, &id) {
            Some(file) => pages_in(&file).map(Some),
            None => Ok(None),
        })
        .await
        .map_err(|_| ToolError::new(ErrorCode::Internal, "couldn't read the file"))??;
        // No file is either a call made again after it printed, whose
        // receipt answers it, or one that never arrived, which is refused.
        let pages_counted = match counted {
            Some(total) => match &ranges {
                Some(ranges) => count_pages(ranges, total)?,
                None => total,
            },
            None => 0,
        };
        let bot = bot_of(caller).map(str::to_string);
        let unattended = matches!(
            caller,
            Caller::Bot {
                unattended: true,
                ..
            }
        );
        let receipt = self
            .runner
            .print(Request {
                printer: printer.id.clone(),
                transfer,
                title: clean_title(args.title.as_deref()),
                copies,
                duplex: args.duplex,
                pages: args.pages,
                pages_counted,
                bot,
                unattended,
            })
            .await
            .map_err(tool_error)?;
        let (value, detail) = match receipt {
            Some(receipt) => (
                json!({
                    "printer": printer.id,
                    "label": printer.label,
                    "job": receipt.job,
                    "state": receipt.state,
                    "pages": receipt.pages,
                    "copies": receipt.copies,
                    "already": receipt.already,
                }),
                format!(
                    "{} · {} pages × {} · job {}",
                    printer.label, receipt.pages, receipt.copies, receipt.job
                ),
            ),
            None => (
                json!({
                    "printer": printer.id,
                    "label": printer.label,
                    "state": "spooling",
                    "pages": pages_counted,
                    "copies": copies,
                }),
                format!("{} · {pages_counted} pages × {copies}", printer.label),
            ),
        };
        Ok((value, Some(detail)))
    }
}

fn bot_of(caller: &Caller) -> Option<&str> {
    match caller {
        Caller::Bot { id, .. } => Some(id),
        _ => None,
    }
}

fn describe(printer: &Printer, found: Option<&SystemPrinter>) -> Value {
    json!({
        "id": printer.id,
        "label": printer.label,
        "state": found.map_or("missing", |p| p.state.as_str()),
        "default": found.is_some_and(|p| p.default),
        "duplex": found.is_some_and(|p| p.duplex),
        "color": found.is_some_and(|p| p.color),
    })
}

fn gone(printer: &Printer) -> ToolError {
    ToolError::new(
        ErrorCode::PrinterUnavailable,
        format!("{} isn't on this computer any more", printer.label),
    )
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args)
        .map_err(|e| ToolError::invalid_argument(format!("invalid arguments: {e}")))
}

/// A job's title goes to the spooler and shows in its queue.
fn clean_title(title: Option<&str>) -> String {
    let title: String = title
        .unwrap_or("Chat with Work")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let title = title.trim();
    if title.is_empty() {
        "Chat with Work".into()
    } else {
        title.to_string()
    }
}

/// The ranges of a page list: `1-3,7`.
pub fn parse_pages(list: &str) -> Result<Vec<(u32, u32)>, ToolError> {
    let bad = || ToolError::invalid_argument("`pages` is a list like 1-3,7");
    let page = |text: &str| {
        (!text.is_empty() && text.len() <= 6 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
            .filter(|page| *page > 0)
            .ok_or_else(bad)
    };
    if list.len() > 200 {
        return Err(bad());
    }
    list.split(',')
        .map(|part| {
            let (from, to) = match part.split_once('-') {
                Some((from, to)) => (page(from)?, page(to)?),
                None => (page(part)?, page(part)?),
            };
            if from > to {
                Err(bad())
            } else {
                Ok((from, to))
            }
        })
        .collect()
}

/// Pages a page list prints of a file of `total`, each counted once it's
/// asked for.
pub fn count_pages(ranges: &[(u32, u32)], total: u32) -> Result<u32, ToolError> {
    let mut count = 0u32;
    for (from, to) in ranges {
        if *to > total {
            return Err(ToolError::invalid_argument(format!(
                "this file has {total} pages"
            )));
        }
        count = count.saturating_add(to - from + 1);
    }
    Ok(count)
}

/// How many pages a PDF has. Read here, in the confined worker, like any
/// document; a PDF it can't open, or one that's encrypted, doesn't print.
fn pages_in(file: &std::path::Path) -> Result<u32, ToolError> {
    let document = lopdf::Document::load(file)
        .map_err(|_| ToolError::new(ErrorCode::Unsupported, "that PDF can't be read"))?;
    if document.is_encrypted() {
        return Err(ToolError::new(
            ErrorCode::Unsupported,
            "that PDF is encrypted",
        ));
    }
    match document.get_pages().len() {
        0 => Err(ToolError::new(
            ErrorCode::Unsupported,
            "that PDF has no pages",
        )),
        pages => Ok(pages as u32),
    }
}

fn tool_error(err: RunError) -> ToolError {
    let code = match err.code.as_str() {
        "not_lent" => ErrorCode::NotLent,
        "needs_approval" => ErrorCode::NeedsApproval,
        "rate_limited" => ErrorCode::RateLimited,
        "not_found" => ErrorCode::NotFound,
        "invalid_argument" => ErrorCode::InvalidArgument,
        _ => ErrorCode::PrinterUnavailable,
    };
    ToolError::new(code, err.message)
}

/// The tools' definitions: name, title, description, input, kind.
pub fn definitions() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Value,
    &'static str,
)> {
    vec![
        (
            "printers",
            "List printers",
            "List the printers on this computer that its owner allows, with whether each is \
             ready and what it can do. Use a printer's `id` in `print`.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            "read",
        ),
        (
            "print",
            "Print a PDF",
            "Print a PDF on a printer the computer's owner allows. The person approves each \
             job, unless the owner lets this bot print on that printer unattended.",
            json!({
                "type": "object",
                "properties": {
                    "printer": { "type": "string", "description": "A printer's `id` from `printers`." },
                    "transfer": { "type": "string", "description": "The file, as sent with `print_bytes`. Not needed for a dry run." },
                    "title": { "type": "string", "maxLength": 120, "description": "The job's name in the print queue." },
                    "copies": { "type": "integer", "minimum": 1, "maximum": MAX_COPIES, "description": "Default 1." },
                    "duplex": { "type": "boolean", "description": "Both sides of the paper, or one. Left out, the printer's own default." },
                    "pages": { "type": "string", "description": "Which pages, like 1-3,7. Left out, all of them." },
                    "dry_run": { "type": "boolean", "description": "Check the printer and print nothing." }
                },
                "required": ["printer"],
                "additionalProperties": false
            }),
            "print",
        ),
        (
            "print_bytes",
            "Send a file to print",
            "Send a PDF to this computer for `print`, a chunk at a time. Chat with Work's own, \
             never offered to a model.",
            json!({
                "type": "object",
                "properties": {
                    "printer": { "type": "string", "description": "The printer it's for." },
                    "transfer": { "type": "string", "description": "16 to 64 lowercase hex digits naming this file." },
                    "offset": { "type": "integer", "minimum": 0 },
                    "size": { "type": "integer", "minimum": 1, "description": "The whole file's size in bytes." },
                    "sha256": { "type": "string", "description": "The whole file's SHA-256, in hex." },
                    "data": { "type": "string", "description": "This chunk in standard base64, at most 64 KiB before it." }
                },
                "required": ["printer", "transfer", "offset", "size", "sha256", "data"],
                "additionalProperties": false
            }),
            "bytes",
        ),
    ]
}
