//! The commands this computer runs for Chat with Work (`run`), as the
//! terminal shows them: one line over the composer while any runs, and
//! `/commands`, the list with what the chosen one prints and a way to stop
//! it. They come from the daemon here, never from the server
//! (`commands::follow`).

use ratatui::text::{Line, Span};

use super::app::App;
use super::theme::{Signal, Theme};
use super::ui::{DOT, RING, ellipsize, spread};
use crate::commands::follow::{Listed, clean, elapsed, one_line};

/// Rows of the list the dialog shows at most.
const LIST_ROWS: usize = 6;

/// How a command stands, as a signal.
pub(super) fn signal(command: &Listed) -> Signal {
    if command.running() {
        Signal::Live
    } else if command.succeeded() {
        Signal::Positive
    } else if command.summary.status.state == "stopped" && command.summary.status.limit.is_none() {
        Signal::Idle
    } else {
        Signal::Negative
    }
}

/// One line over the composer while a command runs: what, where, for how
/// long, and how to follow it.
pub(super) fn running_line(app: &App, width: usize, theme: &Theme) -> Option<Line<'static>> {
    let running: Vec<&Listed> = app
        .commands
        .commands
        .iter()
        .filter(|c| c.running())
        .collect();
    let first = running.first()?;
    let hint = "/commands";
    let text = if running.len() == 1 {
        format!(
            "Running {} in {} · {}",
            first.summary.command,
            first.place(),
            elapsed(first.summary.status.duration_ms)
        )
    } else {
        let names: Vec<&str> = running.iter().map(|c| c.summary.command.as_str()).collect();
        format!("{} commands running · {}", running.len(), names.join(", "))
    };
    let room = width.saturating_sub(hint.chars().count() + 5);
    Some(spread(
        vec![
            Span::styled(DOT, theme.signal(Signal::Live)),
            Span::raw(" "),
            Span::styled(ellipsize(&one_line(&text), room), theme.muted()),
        ],
        vec![Span::styled(hint, theme.faint())],
        width as u16,
    ))
}

/// The dialog's lines: the list, then what the chosen one printed, filling
/// `height` rows.
pub(super) fn dialog_lines(
    app: &App,
    selected: Option<&str>,
    width: usize,
    height: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let list = &app.commands.commands;
    if list.is_empty() {
        return vec![Line::styled(
            "Nothing runs here now. Commands Chat with Work runs in your shared folders \
             show here while they run.",
            theme.muted(),
        )];
    }
    let at = selected
        .and_then(|id| list.iter().position(|c| c.id() == id))
        .unwrap_or(0);
    let first = at.saturating_sub(LIST_ROWS - 1);
    let mut lines: Vec<Line<'static>> = list
        .iter()
        .enumerate()
        .skip(first)
        .take(LIST_ROWS)
        .map(|(i, command)| row(command, i == at, width, theme))
        .collect();
    if list.len() > LIST_ROWS {
        lines.push(Line::styled(
            format!("{} of {}", at + 1, list.len()),
            theme.faint(),
        ));
    }
    let Some(chosen) = list.get(at) else {
        return lines;
    };
    lines.push(Line::raw(""));
    let mut about = format!("{} · {}", chosen.place(), chosen.state_label());
    if chosen.summary.network {
        about.push_str(" · with network");
    }
    if let Some(error) = &chosen.summary.status.error {
        about.push_str(&format!(" · {error}"));
    }
    lines.push(Line::from(vec![
        Span::styled("$ ", theme.faint()),
        Span::styled(
            ellipsize(&one_line(&chosen.summary.command), width.saturating_sub(2)),
            theme.strong(),
        ),
    ]));
    lines.push(Line::styled(
        ellipsize(&about, width),
        theme.signal_ink(signal(chosen)),
    ));
    lines.push(Line::raw(""));
    let room = height.saturating_sub(lines.len()).max(1);
    let printed = app
        .commands
        .printed
        .as_ref()
        .filter(|p| p.id == chosen.id());
    match printed {
        Some(printed) if !printed.text.is_empty() => {
            let text = clean(&printed.text);
            let all: Vec<&str> = text.lines().collect();
            let cut = printed.cut > 0 || all.len() > room;
            let fit = if cut {
                room.saturating_sub(1).max(1)
            } else {
                room
            };
            if cut {
                lines.push(Line::styled("…", theme.faint()));
            }
            lines.extend(
                all[all.len().saturating_sub(fit)..]
                    .iter()
                    .map(|l| Line::styled(ellipsize(l, width), theme.ink())),
            );
        }
        Some(printed) if printed.done => {
            lines.push(Line::styled("It printed nothing.", theme.faint()));
        }
        _ => lines.push(Line::styled("Nothing printed yet.", theme.faint())),
    }
    lines
}

/// One command in the list: state, command line, how long.
fn row(command: &Listed, chosen: bool, width: usize, theme: &Theme) -> Line<'static> {
    let glyph = if command.running() { DOT } else { RING };
    let right = if command.running() {
        elapsed(command.summary.status.duration_ms)
    } else {
        command.state_label()
    };
    let room = width.saturating_sub(right.chars().count() + 5);
    let style = if chosen {
        theme.strong()
    } else {
        theme.muted()
    };
    spread(
        vec![
            Span::styled(if chosen { "› " } else { "  " }, theme.strong()),
            Span::styled(glyph, theme.signal(signal(command))),
            Span::raw(" "),
            Span::styled(ellipsize(&one_line(&command.summary.command), room), style),
        ],
        vec![Span::styled(right, theme.signal_ink(signal(command)))],
        width as u16,
    )
}
