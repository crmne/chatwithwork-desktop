//! Decisions and a bot's page, beside the sidebar, as the web draws them in
//! the terminal's own colours: Decisions lists everything waiting for the
//! person, the chosen one opened with its card, decided with a key; a bot's
//! page has its state, what starts it and what it uses, then its runs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;
use time::OffsetDateTime;

use super::app::{App, Focus, Hit, Hits, parse_ts};
use super::theme::{Signal, Theme};
use super::ui::{BOT, COLUMN, DOT, ellipsize, heading, key_hints, pad, row, spread, wrap};

/// "now", "5 min", "3 h", "2 d".
pub(super) fn ago(at: Option<&str>, now: OffsetDateTime) -> String {
    let Some(at) = at.and_then(parse_ts) else {
        return String::new();
    };
    let minutes = (now - at).whole_minutes().max(0);
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// The list's pane: a heading, then the lines, scrolled so the selected
/// line shows, then the keys.
#[allow(clippy::too_many_arguments)]
fn page(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: Vec<Span<'static>>,
    lines: Vec<(Line<'static>, Option<Hit>)>,
    selected: usize,
    keys: &[(&'static str, &'static str)],
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let area = pad(area, 2, 0);
    let width = area.width.min(COLUMN + 8);
    let area = Rect { width, ..area };
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);
    heading(frame, head, title, app, theme, now, hits);
    let height = body.height as usize;
    let start = (selected + 2).saturating_sub(height);
    for (i, (line, hit)) in lines.into_iter().skip(start).take(height).enumerate() {
        let at = row(body, i as u16);
        if let Some(hit) = hit {
            hits.add(at, hit);
        }
        frame.render_widget(Paragraph::new(line), at);
    }
    if app.focus == Focus::Main {
        frame.render_widget(Paragraph::new(key_hints(keys, theme)), row(foot, 1));
    }
}

// --------------------------------------------------------------- decisions

pub(super) fn decisions(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let pane = &app.chat;
    let width = (area.width.saturating_sub(4)).min(COLUMN + 8) as usize;
    let count = pane.decisions_count();
    let mut title = vec![Span::styled("Decisions", theme.strong())];
    if count > 0 {
        title.push(Span::styled(
            format!("  {count} waiting"),
            theme.signal_ink(Signal::Attention),
        ));
    }
    let mut lines: Vec<(Line<'static>, Option<Hit>)> = Vec::new();
    let mut selected_line = 0;
    let focused = app.focus == Focus::Main && app.modal.is_none();
    match (&pane.decisions, &pane.decisions_error) {
        (_, Some(error)) => {
            for part in wrap(error, width) {
                lines.push((Line::styled(part, theme.signal_ink(Signal::Negative)), None));
            }
        }
        (None, None) => lines.push((Line::styled("Loading…", theme.faint()), None)),
        (Some(list), None) if list.decisions.is_empty() => {
            lines.push((
                Line::from(vec![
                    Span::styled("☺ ", theme.signal(Signal::Positive)),
                    Span::styled("All clear", theme.strong()),
                ]),
                None,
            ));
            lines.push((
                Line::styled(
                    "Nothing waits for you. Bots and chats ask here when they need you.",
                    theme.muted(),
                ),
                None,
            ));
        }
        (Some(list), None) => {
            let chosen = pane.decision_row.min(list.decisions.len() - 1);
            for (i, decision) in list.decisions.iter().enumerate() {
                let here = i == chosen;
                if here {
                    selected_line = lines.len();
                }
                let marker = if here && focused {
                    Span::styled("▌", theme.ink())
                } else {
                    Span::raw(" ")
                };
                let snoozed = decision.snoozed_until.is_some();
                let label = decision
                    .label
                    .clone()
                    .unwrap_or_else(|| decision.kind.clone());
                let service = decision
                    .card
                    .as_ref()
                    .map(|c| format!(" · {}", c.service))
                    .unwrap_or_default();
                let glyph_style = if snoozed {
                    theme.faint()
                } else if decision.destructive {
                    theme.signal(Signal::Negative)
                } else {
                    theme.signal(Signal::Attention)
                };
                let when = ago(decision.since.as_deref(), now);
                let mut first = spread(
                    vec![
                        marker.clone(),
                        Span::styled(format!("{DOT} "), glyph_style),
                        Span::styled(
                            ellipsize(&format!("{label}{service}"), width.saturating_sub(12)),
                            if here { theme.strong() } else { theme.ink() },
                        ),
                    ],
                    vec![Span::styled(when, theme.faint())],
                    width as u16,
                );
                if here && focused {
                    first = first.patch_style(theme.selected());
                }
                lines.push((first, Some(Hit::Row(i))));
                let indent = "    ";
                if let Some(summary) = decision
                    .summary
                    .as_deref()
                    .or(decision.card.as_ref().map(|c| c.summary.as_str()))
                {
                    for part in wrap(summary, width.saturating_sub(4)) {
                        lines.push((
                            Line::styled(format!("{indent}{part}"), theme.ink()),
                            Some(Hit::Row(i)),
                        ));
                    }
                }
                let place = match &decision.chat {
                    Some(chat) => match &chat.project {
                        Some(project) => format!("{} · {project}", chat.title),
                        None => chat.title.clone(),
                    },
                    None => decision.origin.clone().unwrap_or_default(),
                };
                if !place.is_empty() {
                    lines.push((
                        Line::styled(
                            format!("{indent}{}", ellipsize(&place, width.saturating_sub(4))),
                            theme.faint(),
                        ),
                        Some(Hit::Row(i)),
                    ));
                }
                // The chosen one opens with its card's details and why
                // it asks, as the board's card does.
                if here {
                    if let Some(card) = &decision.card {
                        if !card.decidable {
                            let whose = card.waiting_for.as_deref().unwrap_or("someone else");
                            lines.push((
                                Line::styled(format!("{indent}Waiting for {whose}"), theme.muted()),
                                None,
                            ));
                        }
                        let label_width = card
                            .details
                            .iter()
                            .map(|d| d.label.chars().count())
                            .max()
                            .unwrap_or(0)
                            .min(16);
                        for detail in &card.details {
                            let room = width.saturating_sub(label_width + 8).max(8);
                            for (n, part) in wrap(&detail.value, room).into_iter().enumerate() {
                                let label = if n == 0 {
                                    format!(
                                        "{:<label_width$}",
                                        ellipsize(&detail.label, label_width)
                                    )
                                } else {
                                    " ".repeat(label_width)
                                };
                                lines.push((
                                    Line::from(vec![
                                        Span::raw(indent),
                                        Span::styled(label, theme.faint()),
                                        Span::raw("  "),
                                        Span::styled(part, theme.ink()),
                                    ]),
                                    None,
                                ));
                            }
                        }
                    }
                    for line in decision.why.asks.iter().flatten().take(2) {
                        for part in wrap(line, width.saturating_sub(4)) {
                            lines.push((
                                Line::styled(format!("{indent}{part}"), theme.muted()),
                                None,
                            ));
                        }
                    }
                }
                lines.push((Line::raw(""), None));
            }
        }
    }
    let chosen = pane
        .decisions
        .as_ref()
        .and_then(|d| d.decisions.get(pane.decision_row));
    let decidable = chosen.and_then(|d| d.card.as_ref()).filter(|c| c.decidable);
    let keys: Vec<(&'static str, &'static str)> = match decidable {
        Some(card) if card.allow_for_rest_of_chat => vec![
            ("a", "approve"),
            ("A", "for the rest of the chat"),
            ("d", "deny"),
            ("enter", "open its chat"),
        ],
        Some(_) => vec![("a", "approve"), ("d", "deny"), ("enter", "open its chat")],
        None if chosen.is_some() => vec![("enter", "open its chat"), ("r", "read again")],
        None => vec![("r", "read again")],
    };
    page(
        frame,
        area,
        app,
        title,
        lines,
        selected_line,
        &keys,
        theme,
        now,
        hits,
    );
}

// --------------------------------------------------------------- a bot

fn tone(text: Option<&str>) -> Option<Signal> {
    match text? {
        "attention" | "waiting" => Some(Signal::Attention),
        "negative" | "failed" => Some(Signal::Negative),
        "live" | "working" => Some(Signal::Live),
        "positive" => Some(Signal::Positive),
        _ => None,
    }
}

pub(super) fn bot(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    id: u64,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let pane = &app.chat;
    let width = (area.width.saturating_sub(4)).min(COLUMN + 8) as usize;
    let row_of_sidebar = pane.bot(id);
    let page_state = pane.bot_page.as_ref().filter(|p| p.id == id);
    let data = page_state.and_then(|p| p.bot());
    let name = data
        .and_then(|b| b["name"].as_str())
        .map(str::to_string)
        .or(row_of_sidebar.map(|b| b.name.clone()))
        .unwrap_or_else(|| format!("Bot {id}"));
    let state_label = row_of_sidebar
        .and_then(|b| b.state_label.clone())
        .or_else(|| data.and_then(|b| b["state"].as_str()).map(state_words));
    let state_signal = row_of_sidebar
        .and_then(|b| b.dot.as_ref())
        .and_then(|d| tone(d.tone.as_deref()))
        .or_else(|| tone(data.and_then(|b| b["state"].as_str())));
    let mut title = vec![
        Span::styled(format!("{BOT} "), theme.strong()),
        Span::styled(name, theme.strong()),
    ];
    if let Some(label) = state_label {
        title.push(Span::raw("  "));
        title.push(Span::styled(
            label,
            state_signal.map_or(theme.muted(), |s| theme.signal_ink(s)),
        ));
    }
    let mut lines: Vec<(Line<'static>, Option<Hit>)> = Vec::new();
    let mut selected_line = 0;
    if let Some(error) = page_state.and_then(|p| p.error.as_ref()) {
        for part in wrap(error, width) {
            lines.push((
                Line::styled(part, theme.signal_ink(Signal::Attention)),
                None,
            ));
        }
        if let Some(line) = row_of_sidebar.and_then(|b| b.line.clone()) {
            lines.push((Line::raw(""), None));
            lines.push((Line::styled(line, theme.muted()), None));
        }
    } else if let Some(bot) = data {
        let mut about = Vec::new();
        if let Some(project) = bot["project"]["name"].as_str() {
            about.push(format!("In {project}"));
        }
        if let Some(description) = bot["description"].as_str().filter(|d| !d.is_empty()) {
            about.push(description.to_string());
        }
        for part in wrap(&about.join(" · "), width) {
            lines.push((Line::styled(part, theme.muted()), None));
        }
        let section = |lines: &mut Vec<(Line<'static>, Option<Hit>)>, label: &'static str| {
            lines.push((Line::raw(""), None));
            lines.push((Line::styled(label, theme.micro()), None));
        };
        if let Some(triggers) = bot["triggers"].as_array().filter(|t| !t.is_empty()) {
            section(&mut lines, "WHEN");
            for trigger in triggers {
                let mut spans = vec![Span::styled(
                    trigger["name"]
                        .as_str()
                        .unwrap_or("On a schedule")
                        .to_string(),
                    theme.ink(),
                )];
                if let Some(timing) = trigger["timing"].as_str() {
                    spans.push(Span::styled(format!(" · {timing}"), theme.muted()));
                }
                if trigger["paused"].as_bool() == Some(true) {
                    spans.push(Span::styled(" · paused", theme.faint()));
                }
                lines.push((Line::from(spans), None));
            }
        }
        if let Some(uses) = bot["integrations"].as_array().filter(|t| !t.is_empty()) {
            section(&mut lines, "USES");
            for integration in uses {
                let provider = integration["provider"].as_str().unwrap_or_default();
                let mut spans = vec![Span::styled(provider_name(provider), theme.ink())];
                if let Some(who) = integration["acts_as"].as_str() {
                    spans.push(Span::styled(format!(" · as {who}"), theme.muted()));
                }
                if integration["usable"].as_bool() == Some(false) {
                    spans.push(Span::styled(
                        " · needs connecting",
                        theme.signal_ink(Signal::Attention),
                    ));
                }
                lines.push((Line::from(spans), None));
            }
        }
        if let Some(missing) = bot["missing"].as_array().filter(|m| !m.is_empty()) {
            for gap in missing.iter().filter_map(Value::as_str) {
                lines.push((
                    Line::styled(format!("Works best with {gap}"), theme.faint()),
                    None,
                ));
            }
        }
        // Runs, with the page's filters.
        let state = page_state.expect("a page");
        lines.push((Line::raw(""), None));
        let mut words = vec![Span::styled("RUNS  ", theme.micro())];
        for (key, label) in state.filters() {
            let style = if key == state.filter {
                theme.strong().add_modifier(Modifier::UNDERLINED)
            } else {
                theme.faint()
            };
            words.push(Span::styled(label, style));
            words.push(Span::raw("  "));
        }
        lines.push((Line::from(words), None));
        let runs = state.runs();
        if runs.is_empty() {
            lines.push((
                Line::styled(
                    if state.loading {
                        "Loading…"
                    } else if state.filter.is_some() {
                        "None here."
                    } else {
                        "No runs yet. It runs when its trigger fires, or when you ask."
                    },
                    theme.faint(),
                ),
                None,
            ));
        }
        let focused = app.focus == Focus::Main && app.modal.is_none();
        let chosen = state.selected.min(runs.len().saturating_sub(1));
        for (i, run) in runs.iter().enumerate() {
            let here = i == chosen;
            if here {
                selected_line = lines.len();
            }
            let signal = if run["waiting"].as_bool() == Some(true) {
                Some(Signal::Attention)
            } else if run["state"].as_str() == Some("processing") {
                Some(Signal::Live)
            } else if run["outcome"].as_str() == Some("failed")
                || run["state"].as_str() == Some("error")
            {
                Some(Signal::Negative)
            } else {
                None
            };
            let mut left = vec![
                if here && focused {
                    Span::styled("▌", theme.ink())
                } else {
                    Span::raw(" ")
                },
                match signal {
                    Some(signal) => Span::styled(format!("{DOT} "), theme.signal(signal)),
                    None => Span::raw("  "),
                },
            ];
            let mut title = run["title"].as_str().unwrap_or("Run").to_string();
            if run["took_part"].as_bool() == Some(true) {
                title.push_str(" · you took part");
            }
            let when = ago(
                run["last_asked_at"].as_str().or(run["created_at"].as_str()),
                now,
            );
            left.push(Span::styled(
                ellipsize(&title, width.saturating_sub(12)),
                if here { theme.strong() } else { theme.ink() },
            ));
            let mut line = spread(left, vec![Span::styled(when, theme.faint())], width as u16);
            if here && focused {
                line = line.patch_style(theme.selected());
            }
            lines.push((line, Some(Hit::Row(i))));
        }
    } else {
        lines.push((Line::styled("Loading…", theme.faint()), None));
    }
    let keys = [
        ("enter", "open the run"),
        ("f", "filter"),
        ("o", "open in the browser"),
    ];
    page(
        frame,
        area,
        app,
        title,
        lines,
        selected_line,
        &keys,
        theme,
        now,
        hits,
    );
}

/// The bot's state in the web's words, where the sidebar didn't say.
fn state_words(state: &str) -> String {
    match state {
        "waiting" => "Waiting for you",
        "failed" => "Failed",
        "working" => "Working now",
        "paused" => "Paused",
        "listening" => "Listening",
        "next" => "Scheduled",
        _ => "Ready",
    }
    .to_string()
}

/// `google_drive` as "Google Drive".
fn provider_name(key: &str) -> String {
    let known = [
        ("github", "GitHub"),
        ("hubspot", "HubSpot"),
        ("onedrive", "OneDrive"),
        ("sharepoint", "SharePoint"),
        ("bigquery", "BigQuery"),
    ];
    if let Some((_, name)) = known.iter().find(|(k, _)| *k == key) {
        return name.to_string();
    }
    key.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn times_read_as_the_web_writes_them() {
        let now = datetime!(2026-09-25 08:20:00 UTC);
        assert_eq!(ago(Some("2026-09-25T08:19:40Z"), now), "now");
        assert_eq!(ago(Some("2026-09-25T08:00:00Z"), now), "20 min");
        assert_eq!(ago(Some("2026-09-25T05:00:00Z"), now), "3 h");
        assert_eq!(ago(Some("2026-09-21T08:00:00Z"), now), "4 d");
        assert_eq!(ago(None, now), "");
    }

    #[test]
    fn providers_read_as_names() {
        assert_eq!(provider_name("google_drive"), "Google Drive");
        assert_eq!(provider_name("github"), "GitHub");
    }
}
