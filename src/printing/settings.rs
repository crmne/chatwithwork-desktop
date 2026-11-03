//! Which printers Chat with Work may use: changed here, by the `cww` command
//! and the control channel, never by the server.

use anyhow::{Result, bail};

use super::SystemPrinter;
use crate::config::{BotLoan, Config, Printer};

/// A printer's short ID, from its system name.
fn slug(queue: &str) -> String {
    let mut slug = String::new();
    for c in queue.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(40).collect();
    if slug.is_empty() {
        "printer".into()
    } else {
        slug.trim_end_matches('-').to_string()
    }
}

fn unique_id(config: &Config, queue: &str) -> String {
    let base = slug(queue);
    let taken = |id: &str| config.printers.iter().any(|printer| printer.id == id);
    let mut id = base.clone();
    let mut n = 2;
    while taken(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn find(config: &Config, which: &str) -> Result<usize> {
    let found = config
        .printers
        .iter()
        .position(|printer| printer.id == which || printer.queue == which);
    match found {
        Some(index) => Ok(index),
        None => bail!(
            "no printer {which:?} here. Add it first: `cww printers add <name>`, from `cww printers`"
        ),
    }
}

/// Let Chat with Work use a printer of the system's, for the owner's chats
/// (`chats`) or only for the bots it's lent to. Adding one that's there
/// changes its name and `chats`.
pub fn add(
    config: &mut Config,
    system: &[SystemPrinter],
    queue: &str,
    label: Option<&str>,
    chats: bool,
) -> Result<Printer> {
    if !system.iter().any(|printer| printer.queue == queue) {
        bail!("this computer has no printer {queue:?}. `cww printers` lists those it has");
    }
    let label = label.map(str::trim).filter(|label| !label.is_empty());
    let label = label.map(|label| label.chars().take(80).collect::<String>());
    if let Some(printer) = config
        .printers
        .iter_mut()
        .find(|printer| printer.queue == queue)
    {
        if let Some(label) = label {
            printer.label = label;
        }
        printer.chats = chats;
        return Ok(printer.clone());
    }
    let printer = Printer {
        id: unique_id(config, queue),
        queue: queue.to_string(),
        label: label.unwrap_or_else(|| queue.replace('_', " ")),
        chats,
        bots: Vec::new(),
    };
    config.printers.push(printer.clone());
    Ok(printer)
}

/// Stop letting Chat with Work use a printer, for chats and bots alike.
pub fn remove(config: &mut Config, which: &str) -> Result<Printer> {
    let index = find(config, which)?;
    Ok(config.printers.remove(index))
}

/// Lend a printer to a Chat with Work bot (`lent`), or stop. `unattended`
/// lets the bot print there without anyone approving each job. Lending again
/// replaces the loan.
pub fn set_bot(
    config: &mut Config,
    which: &str,
    bot: &str,
    name: Option<&str>,
    lent: bool,
    unattended: bool,
) -> Result<Printer> {
    if bot.is_empty() || bot.len() > 20 || !bot.bytes().all(|b| b.is_ascii_digit()) {
        bail!(
            "{bot:?} isn't a bot's ID: it's the number in the bot's address in Chat with Work, like 42"
        );
    }
    let index = find(config, which)?;
    let printer = &mut config.printers[index];
    let name: String = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(80).collect())
        .or_else(|| printer.loan(bot).map(|loan| loan.name.clone()))
        .unwrap_or_default();
    printer.bots.retain(|loan| loan.id != bot);
    if lent {
        printer.bots.push(BotLoan {
            id: bot.to_string(),
            name,
            unattended,
            claude_code: false,
        });
    }
    Ok(printer.clone())
}

/// What the tools are offered for: every printer anyone may use.
pub fn offered(config: &Config) -> Vec<Printer> {
    config
        .printers
        .iter()
        .filter(|printer| printer.chats || !printer.bots.is_empty())
        .cloned()
        .collect()
}
