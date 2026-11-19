//! Drawing the app. [`render`] is a pure function of the state, the theme
//! and the time it is given, so snapshot tests can pin every screen.
//!
//! The layout follows the desktop app's chat page: on the left the name,
//! New chat, the search and your chats by day, with your name at the foot,
//! which opens the settings; on the right the open chat and the composer.
//! The settings (shared folders, activity, pairing, the daemon) are pages of
//! their own, drawn by [`super::pages`]. One status word shows at the top
//! right only while something needs attention.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use time::OffsetDateTime;

use super::app::{
    Access, App, ChatPane, Choice, Daemon, Focus, Following, FormRow, Hit, Hits, Modal, ModelList,
    Pairing, PickItem, PickerKind, SideItem, View, field_has_choices, parse_ts,
};
use super::chat::{Attachment, Entry, Question};
use super::commands::COMMANDS;
use super::markdown::{self, Footnotes};
use super::pages;
use super::sidebar::{Filter, Label, Row};
use super::theme::{Signal, Theme};
use crate::audit::{AuditEntry, Decision};

pub(super) const DOT: &str = "●";
pub(super) const RING: &str = "○";
const PROMPT: &str = "❯";
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The brand mark beside the name.
const MARK: &str = "◆";
/// A bot, as the web's bot mark: a little screen with eyes.
pub(super) const BOT: &str = "◙";
/// Decisions, as the web's seal with a question.
const DECISIONS: &str = "◇";
/// A project, HQ, and a project only you are on (the web's lock).
const FOLDER: &str = "□";
const HQ: &str = "◫";
const SOLO: &str = "■";

/// The widest a column of text gets, as the web's reading column.
pub(super) const COLUMN: u16 = 76;

pub fn render(frame: &mut Frame, app: &App, theme: &Theme, now: OffsetDateTime) {
    render_hits(frame, app, theme, now);
}

/// [`render`], saying where it drew what the mouse can click.
pub fn render_hits(frame: &mut Frame, app: &App, theme: &Theme, now: OffsetDateTime) -> Hits {
    let mut hits = Hits::default();
    let area = frame.area();
    let side_width = match area.width {
        _ if app.sidebar_hidden => 0,
        90.. => 32,
        64.. => 26,
        _ => 0,
    };
    let [side, main] =
        Layout::horizontal([Constraint::Length(side_width), Constraint::Min(0)]).areas(area);
    if side.width > 0 {
        sidebar(frame, side, app, theme, now, &mut hits);
    }
    match app.view {
        View::Chat => main_pane(frame, main, app, theme, now, &mut hits),
        View::Decisions => super::views::decisions(frame, main, app, theme, now, &mut hits),
        View::Bot(id) => super::views::bot(frame, main, app, id, theme, now, &mut hits),
        View::Settings(page) => {
            let bare = side.width == 0;
            pages::render(frame, main, app, page, bare, theme, now, &mut hits);
        }
    }
    if let Some(modal) = &app.modal {
        modal_box(frame, area, modal, app, theme);
    }
    hits
}

// ---------------------------------------------------------------- sidebar

fn sidebar(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(theme.line());
    let inner = pad(block.inner(area), 1, 0);
    frame.render_widget(block, area);
    let low = app
        .chat
        .list
        .credits
        .as_ref()
        .filter(|c| c.running_low && app.chat.ready());
    let [brand, body, foot] = Layout::vertical([
        Constraint::Length(if app.organization_name().is_some() {
            4
        } else {
            2
        }),
        Constraint::Min(0),
        Constraint::Length(if low.is_some() { 3 } else { 2 }),
    ])
    .areas(inner);
    let name = Line::from(vec![
        Span::styled(MARK, theme.strong()),
        Span::raw(" "),
        Span::styled("Chat with Work", theme.strong()),
    ]);
    frame.render_widget(Paragraph::new(name), brand);
    if let Some(organization) = app.organization_name() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                ellipsize(organization, brand.width as usize),
                theme.muted(),
            )),
            row(brand, 2),
        );
    }
    match app.view {
        View::Settings(page) => pages::nav(frame, body, page, theme, hits),
        _ => chats_section(frame, body, app, theme, now, hits),
    }
    account_line(frame, row(foot, 1), app, theme, hits);
    // The meter under the name, as the web shows it when credits run low.
    if let Some(credits) = low {
        let text = format!(" {}", credits_left(credits));
        frame.render_widget(
            Paragraph::new(Line::styled(
                ellipsize(&text, foot.width as usize),
                theme.signal_ink(Signal::Attention),
            )),
            row(foot, 2),
        );
    }
}

/// "1,250 of 5,000 credits left".
pub(super) fn credits_left(credits: &super::chat::Credits) -> String {
    format!(
        "{} of {} credits left",
        thousands(credits.left),
        thousands(credits.capacity)
    )
}

/// Your name at the foot, as the desktop app has it: it opens the settings.
fn account_line(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, hits: &mut Hits) {
    let name = app.chat.list.user.name.trim();
    let line = if name.is_empty() {
        Line::from(vec![
            Span::raw(" "),
            Span::styled("Settings", theme.muted()),
        ])
    } else {
        let initial: String = name
            .chars()
            .next()
            .into_iter()
            .flat_map(char::to_uppercase)
            .collect();
        Line::from(vec![
            Span::styled(format!("({initial})"), theme.muted()),
            Span::raw(" "),
            Span::styled(
                ellipsize(name, area.width.saturating_sub(5) as usize),
                theme.ink(),
            ),
        ])
    };
    let line = if matches!(app.view, View::Settings(_)) {
        line.patch_style(theme.selected())
    } else {
        line
    };
    hits.add(area, Hit::Settings);
    frame.render_widget(Paragraph::new(line), area);
}

/// The web's sidebar: New bot and New chat, Decisions, the search, then
/// Pinned, the projects' tree and Recent, scrolling under them.
fn chats_section(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    if area.height == 0 {
        return;
    }
    let pane = &app.chat;
    let focused = app.focus == Focus::Chats && app.modal.is_none();
    let width = area.width as usize;
    if !pane.ready() {
        let text = match &pane.access {
            Access::Loading => "Loading…".to_string(),
            Access::NeedsApproval {
                requested: false, ..
            } => "Chats need your OK · o asks".into(),
            Access::NeedsApproval { .. } => "Waiting for your OK".into(),
            Access::Unavailable(_) => "Chats aren't available here".into(),
            Access::Ready | Access::Unknown => match app.chat_daemon() {
                Daemon::NotRunning(_) => "Start the daemon to chat".into(),
                Daemon::Running(s) if !s.paired || s.connection.connection == "revoked" => {
                    "Pair this computer to chat".into()
                }
                _ => "Looking for the daemon…".into(),
            },
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {}", ellipsize(&text, width - 1)),
                theme.faint(),
            )),
            area,
        );
        return;
    }
    let selected = pane.selected_item();
    let here = |item: SideItem| focused && selected == Some(item);
    let marker = |item: SideItem| {
        if here(item) {
            Span::styled("▌", theme.ink())
        } else {
            Span::raw(" ")
        }
    };
    let highlight = |line: Line<'static>, item: SideItem| {
        if here(item) {
            line.patch_style(theme.selected())
        } else {
            line
        }
    };

    // New bot and New chat side by side, as the web's two buttons.
    let top = row(area, 0);
    let new_chat = highlight(
        Line::from(vec![
            marker(SideItem::NewChat),
            Span::styled("+ ", theme.strong()),
            Span::styled("New chat", theme.ink()),
        ]),
        SideItem::NewChat,
    );
    if pane.can_make_bots() {
        let half = top.width / 2;
        let left = Rect { width: half, ..top };
        let right = Rect {
            x: top.x + half,
            width: top.width - half,
            ..top
        };
        let new_bot = highlight(
            Line::from(vec![
                marker(SideItem::NewBot),
                Span::styled(format!("{BOT} "), theme.strong()),
                Span::styled("New bot", theme.strong()),
            ]),
            SideItem::NewBot,
        );
        frame.render_widget(Paragraph::new(new_bot), left);
        hits.add(left, Hit::Side(SideItem::NewBot));
        frame.render_widget(Paragraph::new(new_chat), right);
        hits.add(right, Hit::Side(SideItem::NewChat));
    } else {
        frame.render_widget(Paragraph::new(new_chat), top);
        hits.add(top, Hit::Side(SideItem::NewChat));
    }

    // Decisions, the home, with what waits; an older server has none.
    let count = pane.decisions_count();
    let has_decisions = pane.top_items().contains(&SideItem::Decisions);
    let on_decisions = app.view == View::Decisions;
    let name_style = if count > 0 {
        theme.signal_ink(Signal::Attention)
    } else if on_decisions {
        theme.strong()
    } else {
        theme.muted()
    };
    let end = if count > 0 {
        vec![
            Span::styled(count.to_string(), theme.signal_ink(Signal::Attention)),
            Span::raw(" "),
            Span::styled(DOT, theme.signal(Signal::Attention)),
            Span::raw(" "),
        ]
    } else {
        vec![Span::styled("All clear ", theme.faint())]
    };
    let name_style = if on_decisions {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    let decisions = spread(
        vec![
            marker(SideItem::Decisions),
            Span::styled(format!("{DECISIONS} "), name_style),
            Span::styled("Decisions", name_style),
        ],
        end,
        area.width,
    );
    if has_decisions {
        frame.render_widget(
            Paragraph::new(highlight(decisions, SideItem::Decisions)),
            row(area, 1),
        );
        hits.add(row(area, 1), Hit::Side(SideItem::Decisions));
    }

    let search = match &pane.search {
        Some(query) => Line::from(vec![
            Span::raw(" "),
            Span::styled("/ ", theme.signal_ink(Signal::Live)),
            Span::styled(tail(query, width.saturating_sub(4)), theme.ink()),
        ]),
        None => Line::from(vec![
            Span::raw(" "),
            Span::styled("/ ", theme.faint()),
            Span::styled("Search chats and bots", theme.faint()),
        ]),
    };
    frame.render_widget(Paragraph::new(search), row(area, 2));
    hits.add(row(area, 2), Hit::Search);

    let body = below(area, 4);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut targets: Vec<Vec<(u16, u16, Hit)>> = Vec::new();
    let mut selected_line = 0;
    let filter_words = |lines: &mut Vec<Line<'static>>, targets: &mut Vec<_>, label: &str| {
        let mut right = Vec::new();
        let mut spots = Vec::new();
        let mut x = 0u16;
        for filter in Filter::ALL {
            let style = if filter == pane.tree.filter {
                theme.strong().add_modifier(Modifier::UNDERLINED)
            } else {
                theme.faint()
            };
            let word = filter.label();
            spots.push((x, word.chars().count() as u16, Hit::Filter(filter)));
            x += word.chars().count() as u16 + 1;
            right.push(Span::styled(word.to_string(), style));
            right.push(Span::raw(" "));
        }
        let start = area.width.saturating_sub(x);
        let spots = spots
            .into_iter()
            .map(|(dx, w, hit)| (start + dx, w, hit))
            .collect();
        lines.push(spread(
            vec![Span::styled(format!(" {label}"), theme.micro())],
            right,
            area.width,
        ));
        targets.push(spots);
    };
    let has_projects = !pane.nodes().is_empty();
    for entry in pane.rows() {
        match entry {
            Row::Label(Label::Pinned) => {
                lines.push(Line::styled(" PINNED", theme.micro()));
                targets.push(Vec::new());
            }
            Row::Label(Label::Projects) => {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                    targets.push(Vec::new());
                }
                filter_words(&mut lines, &mut targets, "PROJECTS");
            }
            Row::Label(Label::Recent) => {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                    targets.push(Vec::new());
                }
                if has_projects {
                    lines.push(Line::styled(" RECENT", theme.micro()));
                    targets.push(Vec::new());
                } else {
                    filter_words(&mut lines, &mut targets, "RECENT");
                }
            }
            Row::Label(Label::Empty) => {
                let text = if pane.search.is_some() {
                    " No match"
                } else {
                    match pane.tree.filter {
                        Filter::Bots => " No bots yet",
                        _ => " No chats yet",
                    }
                };
                lines.push(Line::styled(text, theme.faint()));
                targets.push(Vec::new());
            }
            Row::Item { item, depth } => {
                if here(item) || selected == Some(item) {
                    selected_line = lines.len();
                }
                let line = side_row(
                    app,
                    item,
                    depth,
                    here(item),
                    marker(item),
                    width,
                    theme,
                    now,
                );
                lines.push(highlight(line, item));
                targets.push(vec![(0, area.width, Hit::Side(item))]);
            }
        }
    }

    let height = body.height as usize;
    let start = (selected_line + 1).saturating_sub(height);
    hits.add(body, Hit::Chats);
    for (i, spots) in targets.into_iter().skip(start).take(height).enumerate() {
        for (x, w, hit) in spots {
            hits.add(
                Rect {
                    x: body.x + x,
                    width: w.min(body.width.saturating_sub(x)),
                    ..row(body, i as u16)
                },
                hit,
            );
        }
    }
    let shown: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(shown), body);
}

/// One row of the sidebar's list: a project's header, a chat, a bot, or
/// "N more", indented by its depth in the tree.
#[allow(clippy::too_many_arguments)]
fn side_row(
    app: &App,
    item: SideItem,
    depth: u16,
    highlighted: bool,
    marker: Span<'static>,
    width: usize,
    theme: &Theme,
    now: OffsetDateTime,
) -> Line<'static> {
    let pane = &app.chat;
    let indent = "  ".repeat(depth as usize);
    let mut left = vec![marker, Span::raw(" "), Span::raw(indent.clone())];
    let mut right: Vec<Span<'static>> = Vec::new();
    let count = |n: String, signal: Signal| {
        vec![
            Span::styled(n, theme.signal_ink(signal)),
            Span::raw(" "),
            Span::styled(DOT, theme.signal(signal)),
            Span::raw(" "),
        ]
    };
    let name: String;
    let mut name_style = theme.muted();
    let mut shimmer = false;
    match item {
        SideItem::Project(id) => {
            let Some(node) = pane.node(id) else {
                return Line::raw("");
            };
            let open = pane.is_open(&node);
            left.push(Span::styled(if open { "▾ " } else { "▸ " }, theme.faint()));
            let icon = if node.hq {
                HQ
            } else if node.solo {
                SOLO
            } else {
                FOLDER
            };
            left.push(Span::styled(format!("{icon} "), theme.faint()));
            name = node.name.to_string();
            name_style = theme.ink();
            if !open && node.waiting > 0 {
                right = count(node.waiting.to_string(), Signal::Attention);
            } else if !open && node.working {
                right = vec![
                    Span::styled(DOT, theme.signal(Signal::Live)),
                    Span::raw(" "),
                ];
            }
        }
        SideItem::Chat(number) | SideItem::PinnedChat(number) => {
            let Some(chat) = pane.chat(number) else {
                return Line::raw("");
            };
            let open = app.view == View::Chat && pane.open == Some(number);
            left.push(Span::raw(if depth > 0 { "  " } else { "" }));
            if chat.schedule.is_some() {
                left.push(Span::styled("◷ ", theme.faint()));
            }
            name = chat.title.clone();
            if open {
                name_style = theme.strong();
            }
            let waiting = pane.waiting_in(number);
            if waiting > 0 {
                right = count(waiting.to_string(), Signal::Attention);
            } else if chat.processing() || (open && pane.working()) {
                right = vec![
                    Span::styled(DOT, theme.signal(Signal::Live)),
                    Span::raw(" "),
                ];
            } else if chat.state == "error" {
                right = vec![
                    Span::styled(DOT, theme.signal(Signal::Negative)),
                    Span::raw(" "),
                ];
            }
            shimmer = (open && pane.working()) || (pane.list_live && chat.processing());
        }
        SideItem::Bot(id) | SideItem::PinnedBot(id) => {
            let Some(bot) = pane.bot(id) else {
                return Line::raw("");
            };
            left.push(Span::raw(if depth > 0 { "  " } else { "" }));
            left.push(Span::styled(format!("{BOT} "), theme.strong()));
            name = bot.name.clone();
            name_style = if bot.paused {
                theme.faint()
            } else if app.view == View::Bot(id) {
                theme.strong()
            } else {
                theme.ink()
            };
            if let Some(dot) = &bot.dot {
                let signal = match dot.tone.as_deref() {
                    Some("attention") => Some(Signal::Attention),
                    Some("negative") => Some(Signal::Negative),
                    Some("live") => Some(Signal::Live),
                    Some("positive") => Some(Signal::Positive),
                    _ => None,
                };
                if let Some(signal) = signal {
                    right = match dot.number() {
                        Some(n) => count(n, signal),
                        None => vec![Span::styled(DOT, theme.signal(signal)), Span::raw(" ")],
                    };
                }
            }
        }
        SideItem::More(id) => {
            left.push(Span::raw("  "));
            name = format!("{} more", pane.more_in(id));
            name_style = theme.faint();
        }
        SideItem::Fewer(_) => {
            left.push(Span::raw("  "));
            name = "Show fewer".into();
            name_style = theme.faint();
        }
        SideItem::MoreProjects => {
            name = if pane.tree.all_projects {
                "Show fewer".into()
            } else {
                format!("{} more", pane.more_projects())
            };
            name_style = theme.faint();
        }
        SideItem::NewBot | SideItem::NewChat | SideItem::Decisions => return Line::raw(""),
    }
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let title = ellipsize(&name, width.saturating_sub(used + 1));
    if shimmer && !highlighted {
        left.extend(theme.shimmer_text(&title, sweep(now)));
    } else {
        left.push(Span::styled(title, name_style));
    }
    spread(left, right, width as u16)
}

// --------------------------------------------------------- status and main

/// The one status word, top right, while something needs attention:
/// nothing at all while Chat with Work can reach this computer and the open
/// chat is followed live.
pub(super) fn attention(
    app: &App,
    now: OffsetDateTime,
) -> Option<(&'static str, Signal, &'static str)> {
    match if app.view == View::Chat {
        app.chat_daemon()
    } else {
        &app.daemon
    } {
        Daemon::Unknown => None,
        Daemon::NotRunning(_) => Some((RING, Signal::Negative, "daemon stopped")),
        Daemon::Running(status) if status.paused => Some(("‖", Signal::Attention, "paused")),
        Daemon::Running(status) => match status.connection.connection.as_str() {
            "connected" => match app.chat.following {
                _ if app.view != View::Chat => None,
                Following::Offline => Some((DOT, Signal::Attention, "reconnecting")),
                Following::Refused | Following::Unsupported => {
                    Some((RING, Signal::Attention, "not live"))
                }
                _ => None,
            },
            "connecting" => Some((spinner(now), Signal::Attention, "connecting")),
            "not_paired" => Some((RING, Signal::Attention, "not paired")),
            "revoked" => Some((DOT, Signal::Negative, "needs to reconnect")),
            _ => Some((DOT, Signal::Negative, "offline")),
        },
    }
}

/// A heading on the left and the status word, if any, on the right of
/// `area`; the status word opens the settings.
pub(super) fn heading(
    frame: &mut Frame,
    area: Rect,
    left: Vec<Span<'static>>,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let right = match attention(app, now) {
        Some((glyph, signal, text)) => vec![
            Span::styled(glyph, theme.signal(signal)),
            Span::styled(format!(" {text}"), theme.signal_ink(signal)),
        ],
        None => Vec::new(),
    };
    let right_width: u16 = right.iter().map(|s| s.width() as u16).sum();
    if right_width > 0 {
        hits.add(
            Rect {
                x: area.right().saturating_sub(right_width),
                width: right_width.min(area.width),
                ..row(area, 0)
            },
            Hit::Settings,
        );
    }
    frame.render_widget(
        Paragraph::new(spread(left, right, area.width)),
        row(area, 0),
    );
}

fn main_pane(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    // The composer grows with what's typed and the list under it, up to
    // half the pane.
    let view = composer_view(app, theme, area.width.saturating_sub(6) as usize, now);
    let most = (area.height / 2).max(3);
    let composer_height = (view.lines.len() as u16 + 2).clamp(3, most);
    let mut notice = notice_lines(app, area.width.saturating_sub(4) as usize, theme);
    if notice.is_empty()
        && let Some(line) =
            super::running::running_line(app, area.width.saturating_sub(4) as usize, theme)
    {
        notice.push(line);
    }
    // No title over the chat, as on the web: the header only holds the
    // status word, while something needs attention.
    let organization = app.organization_name().filter(|_| frame.area().width < 64);
    let header_height = if attention(app, now).is_some() || organization.is_some() {
        2
    } else {
        0
    };
    let [header, body, notice_area, composer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(0),
        Constraint::Length(notice.len() as u16),
        Constraint::Length(composer_height),
    ])
    .areas(area);
    if header_height > 0 {
        let left = organization
            .map(|name| {
                vec![Span::styled(
                    ellipsize(name, header.width.saturating_sub(18) as usize),
                    theme.muted(),
                )]
            })
            .unwrap_or_default();
        heading(frame, pad(header, 2, 0), left, app, theme, now, hits);
    }
    chat_view(frame, pad(body, 2, 0), app, theme, now, hits);
    frame.render_widget(Paragraph::new(notice), pad(notice_area, 2, 0));
    composer_box(frame, pad(composer, 1, 0), app, theme, now, view, hits);
}

/// What just happened, or went wrong, above the composer until the next
/// key.
pub(super) fn notice_lines(app: &App, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if app.chat.dictation.phase != super::dictation::Phase::Idle {
        let state = &app.chat.dictation;
        let label = if state.phase == super::dictation::Phase::Recording {
            let bars: String = state
                .levels
                .iter()
                .map(|level| {
                    ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][usize::from(*level) * 7 / 1000]
                })
                .collect();
            match state.downloading() {
                Some(download) => format!(
                    "Listening  {bars}  {}  Esc to stop · {download}",
                    state.elapsed()
                ),
                None => format!("Listening  {bars}  {}  Esc to stop", state.elapsed()),
            }
        } else if let Some(download) = state.downloading()
            && state.phase == super::dictation::Phase::Transcribing
        {
            format!("{download}. Escape to discard.")
        } else {
            state.phase.label().to_owned()
        };
        return notice_line(&label, Signal::Idle, width, theme);
    }
    match &app.notice {
        Some(notice) => {
            let mut lines = notice_line(&notice.text, notice.signal, width, theme);
            lines.truncate(2);
            if notice.signal == Signal::Idle
                && let Some(first) = lines.first_mut()
                && let Some(glyph) = first.spans.first_mut()
            {
                *glyph = Span::styled(RING, theme.faint());
            }
            lines
        }
        None => Vec::new(),
    }
}
/// Something that needs saying, with a signal edge.
pub(super) struct Card {
    pub signal: Signal,
    pub title: String,
    pub lines: Vec<CardLine>,
}

pub(super) enum CardLine {
    Text(String),
    Command(String),
    Path(String),
    /// Something to read off the screen: a code, a link.
    Value(String),
    Keys(Vec<(&'static str, &'static str)>),
}

/// A pairing under way: the code to check, and where to approve it.
pub(super) fn pairing_card(app: &App) -> Option<Card> {
    match &app.pairing {
        Some(Pairing::Starting) => Some(Card {
            signal: Signal::Attention,
            title: "Pairing this computer".into(),
            lines: vec![CardLine::Text("Asking Chat with Work for a code…".into())],
        }),
        Some(Pairing::Waiting {
            code,
            url,
            name,
            fingerprint,
            opened,
        }) => Some(Card {
            signal: Signal::Attention,
            title: format!("Approve \"{name}\" on Chat with Work"),
            lines: vec![
                CardLine::Text(if *opened {
                    "The page is open in your browser. If it isn't, open:".into()
                } else {
                    "Open this page, sign in, and approve:".into()
                }),
                CardLine::Value(url.clone()),
                CardLine::Text("Check that it shows the same code:".into()),
                CardLine::Value(code.clone()),
                CardLine::Text(format!("Key fingerprint {fingerprint}")),
                CardLine::Keys(vec![("esc", "cancel")]),
            ],
        }),
        None => None,
    }
}

/// The cards the chat view leads with: the daemon's state, then the
/// first-run offer.
fn cards(app: &App) -> Vec<Card> {
    let mut cards: Vec<Card> = pairing_card(app).into_iter().collect();
    match &app.daemon {
        Daemon::Unknown => {}
        Daemon::NotRunning(offline) => {
            let mut lines = match &offline.error {
                Some(error) => vec![
                    CardLine::Text(error.clone()),
                    CardLine::Text("If the daemon isn't running, start it with:".into()),
                ],
                None => vec![CardLine::Text(
                    "Chat with Work can't search your folders until it runs. Start it in the \
                     background with:"
                        .into(),
                )],
            };
            lines.push(CardLine::Command("cww daemon install".into()));
            if !offline.paired && app.pairing.is_none() {
                lines.push(CardLine::Text(
                    "This computer isn't paired yet either.".into(),
                ));
            }
            lines.push(CardLine::Text(
                "Until then, this shows config.toml, and changes here are saved there.".into(),
            ));
            let mut keys = vec![("s", "start it")];
            if !offline.paired && app.pairing.is_none() {
                keys.push(("c", "pair"));
            }
            keys.push(("r", "look again"));
            lines.push(CardLine::Keys(keys));
            let title = if offline.error.is_some() {
                "Can't reach the daemon"
            } else {
                "The daemon isn't running"
            };
            cards.push(Card {
                signal: Signal::Negative,
                title: title.into(),
                lines,
            });
        }
        Daemon::Running(status) => match status.connection.connection.as_str() {
            "not_paired" if app.pairing.is_none() => cards.push(Card {
                signal: Signal::Attention,
                title: "This computer isn't paired".into(),
                lines: vec![
                    CardLine::Text(
                        "Chat with Work can't search your folders until you pair it. Press \
                             c to pair it here, or run cww login in a terminal."
                            .into(),
                    ),
                    CardLine::Keys(vec![("c", "pair this computer")]),
                ],
            }),
            "revoked" if app.pairing.is_none() => cards.push(Card {
                signal: Signal::Negative,
                title: status
                    .connection
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "This computer needs to sign in again".into()),
                lines: vec![
                    CardLine::Text("Your folders aren't shared until it reconnects.".into()),
                    CardLine::Keys(vec![("c", "reconnect")]),
                ],
            }),
            "offline" => cards.push(Card {
                signal: Signal::Attention,
                title: "Can't reach Chat with Work".into(),
                lines: vec![CardLine::Text(format!(
                    "{} The daemon keeps trying on its own.",
                    status
                        .connection
                        .last_error
                        .as_deref()
                        .map(|e| format!("Last error: {e}."))
                        .unwrap_or_else(|| "The connection dropped.".into())
                ))],
            }),
            _ => {}
        },
    }
    if let Some(card) = chat_card(app) {
        cards.push(card);
    }
    if let Some(suggestion) = app.offer() {
        cards.push(Card {
            signal: Signal::Live,
            title: format!("Share your {} folder?", suggestion.label),
            lines: vec![
                CardLine::Text(format!(
                    "Nothing is shared yet. Chat with Work can search your {} folder:",
                    suggestion.label
                )),
                CardLine::Path(suggestion.path.clone()),
                CardLine::Text(
                    "Keys, .env files and password databases inside it stay private either way."
                        .into(),
                ),
                CardLine::Keys(vec![
                    ("y", "share it"),
                    ("n", "not now"),
                    ("a", "pick another folder"),
                ]),
            ],
        });
    } else if !matches!(app.daemon, Daemon::Unknown) && app.daemon.roots().is_empty() {
        cards.push(Card {
            signal: Signal::Idle,
            title: "Nothing is shared yet".into(),
            lines: vec![
                CardLine::Text("Chat with Work can only search folders you share.".into()),
                CardLine::Keys(vec![("a", "share a folder")]),
            ],
        });
    }
    cards
}

/// What stands between this computer and the chats, when something does.
fn chat_card(app: &App) -> Option<Card> {
    match &app.chat.access {
        Access::NeedsApproval {
            requested: false, ..
        } => Some(Card {
            signal: Signal::Attention,
            title: "Chats need your OK".into(),
            lines: vec![
                CardLine::Text(
                    "This computer shares folders with Chat with Work. To read and send your \
                     chats here too, allow it in Chat with Work. It's a separate permission, \
                     and you can turn it off without unsharing anything."
                        .into(),
                ),
                CardLine::Keys(vec![("o", "ask, and open the page to allow it")]),
            ],
        }),
        Access::NeedsApproval { url, .. } => {
            let mut lines = vec![CardLine::Text(
                "Allow chats for this computer in Chat with Work, under Settings, Computers:"
                    .into(),
            )];
            if let Some(url) = url {
                lines.push(CardLine::Value(url.clone()));
            }
            lines.push(CardLine::Text("This updates by itself once you do.".into()));
            lines.push(CardLine::Keys(vec![
                ("o", "open the page"),
                ("r", "check now"),
            ]));
            Some(Card {
                signal: Signal::Attention,
                title: "Waiting for your OK".into(),
                lines,
            })
        }
        Access::Unavailable(failure) => {
            let (signal, title) = match failure.code.as_str() {
                "unsupported" => (Signal::Idle, "This server doesn't offer chats here"),
                "daemon_outdated" => (Signal::Attention, "The daemon needs restarting"),
                "revoked" => (Signal::Negative, "This computer needs to reconnect"),
                "unreachable" => (Signal::Attention, "Can't reach Chat with Work"),
                "forbidden" => (Signal::Attention, "Chats aren't available to you here"),
                _ => (Signal::Negative, "Chats aren't available"),
            };
            Some(Card {
                signal,
                title: title.into(),
                lines: vec![
                    CardLine::Text(failure.message.clone()),
                    CardLine::Keys(vec![("r", "try again")]),
                ],
            })
        }
        _ => None,
    }
}

pub(super) fn card_lines(card: &Card, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let (glyph, style) = match card.signal {
        Signal::Idle => (RING, theme.faint()),
        signal => (DOT, theme.signal(signal)),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(glyph, style),
        Span::raw(" "),
        Span::styled(card.title.clone(), theme.strong()),
    ])];
    for line in &card.lines {
        match line {
            CardLine::Text(text) => lines.extend(
                wrap(text, width)
                    .into_iter()
                    .map(|l| Line::styled(l, theme.muted())),
            ),
            CardLine::Command(command) => lines.push(command_line(command, theme)),
            CardLine::Path(path) => lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(ellipsize_start(path, width.saturating_sub(2)), theme.ink()),
            ])),
            // Links and codes wrap rather than lose their end.
            CardLine::Value(value) => lines.extend(
                wrap(value, width.saturating_sub(2))
                    .into_iter()
                    .map(|part| {
                        Line::from(vec![Span::raw("  "), Span::styled(part, theme.strong())])
                    }),
            ),
            CardLine::Keys(keys) => lines.push(key_hints(keys, theme)),
        }
    }
    lines
}

pub(super) fn render_card(
    frame: &mut Frame,
    area: Rect,
    card: &Card,
    lines: Vec<Line<'static>>,
    theme: &Theme,
) {
    // Ringed in the signal's colour, as the web's notices and approval cards
    // are: never a stripe down one side, and never a painted background.
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.edge(card.signal));
    let inner = pad(block.inner(area), 1, 0);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn chat_view(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    let mut y = area.y;
    let width = area.width.min(COLUMN);
    // Inside a conversation, only a pairing under way stays on top of it;
    // the status word says the rest.
    let in_chat =
        app.chat.ready() && (app.chat.open.is_some() || app.chat.pending_question.is_some());
    let cards = if in_chat {
        pairing_card(app).into_iter().collect()
    } else {
        cards(app)
    };
    for card in cards {
        let lines = card_lines(&card, width.saturating_sub(4) as usize, theme);
        let height = lines.len() as u16 + 2;
        if y + height > area.bottom() {
            break;
        }
        render_card(
            frame,
            Rect::new(area.x, y, width, height),
            &card,
            lines,
            theme,
        );
        y += height + 1;
    }
    let rest = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y));
    if app.chat.ready() {
        conversation(frame, rest, app, theme, now, hits);
    }
}

/// The open chat's transcript, newest at the bottom.
fn conversation(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    hits: &mut Hits,
) {
    if area.height == 0 {
        return;
    }
    let pane = &app.chat;
    let body = area;
    let (mut lines, marks, question) = transcript(app, body.width as usize, theme, now);
    let height = body.height as usize;
    let most = lines.len().saturating_sub(height);
    // Pinned after a send: the newest question a line under the top, its
    // answer under it, as long as there's more below than fits.
    let scroll = match question.filter(|_| pane.pinned) {
        Some(at) => {
            let start = at.saturating_sub(1).min(most);
            let scroll = most - start;
            pane.pinned_scroll.set(scroll);
            scroll
        }
        None => pane.scroll.min(most),
    };
    let end = lines.len() - scroll;
    let start = end.saturating_sub(height);
    lines.truncate(end);
    let shown = lines.split_off(start);
    hits.add(body, Hit::Transcript);
    for (i, line) in shown.iter().enumerate() {
        if let Some(url) = link_in(line) {
            hits.add(row(body, i as u16), Hit::Link(url));
        }
    }
    for (at, hit) in marks {
        if (start..end).contains(&at) {
            hits.add(row(body, (at - start) as u16), hit);
        }
    }
    frame.render_widget(Paragraph::new(shown), body);
    if scroll > 0 && body.height > 0 {
        let note = Line::styled(format!("↓ {scroll} more · end"), theme.faint())
            .alignment(Alignment::Right);
        frame.render_widget(Paragraph::new(note), row(body, body.height - 1));
    }
}

pub(super) fn command_line(command: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("  $ ", theme.faint()),
        Span::styled(command.to_string(), theme.strong()),
    ])
}

/// A whole web link a line shows, to open with a click.
fn link_in(line: &Line) -> Option<String> {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    // Cut short to fit: not the real address.
    (!url.ends_with('…')).then_some(url)
}

/// The conversation as lines, oldest first, with the lines a click acts on.
/// The third value is where the newest question starts.
fn transcript(
    app: &App,
    width: usize,
    theme: &Theme,
    now: OffsetDateTime,
) -> (Vec<Line<'static>>, Vec<(usize, Hit)>, Option<usize>) {
    let mut question = None;
    let mut marks: Vec<(usize, Hit)> = Vec::new();
    let pane = &app.chat;
    let width = width.max(16);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if pane.open.is_none() && pane.pending_question.is_none() {
        lines.push(Line::styled("Ask anything.", theme.strong()));
        lines.extend(
            wrap(
                "Chat with Work searches your shared folders and the services you connected, \
                 and the chat stays in Chat with Work on the web too.",
                width.min(72),
            )
            .into_iter()
            .map(|l| Line::styled(l, theme.muted())),
        );
        if let Some(reason) = pane.locked_reason() {
            lines.push(Line::raw(""));
            lines.push(notice_line(reason, Signal::Attention, width, theme).remove(0));
        }
        return (lines, marks, None);
    }
    if pane.transcript.is_none() && pane.loading {
        lines.push(Line::from(vec![
            Span::styled(spinner(now), theme.signal(Signal::Live)),
            Span::styled(" Reading the chat…", theme.faint()),
        ]));
    }
    if let Some(schedule) = pane.open_summary().and_then(|c| c.schedule.as_ref()) {
        lines.extend(
            wrap(&format!("◷ {}", schedule.label()), width)
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
        );
        lines.push(Line::raw(""));
    }
    if pane.list.web_search.allowed
        && pane
            .web_search
            .unwrap_or_else(|| pane.open_summary().is_some_and(|c| c.web_search))
    {
        lines.push(Line::styled(
            format!("Web search on · {}", pane.list.web_search.rate),
            theme.muted(),
        ));
    }
    let mut answered: Vec<u64> = Vec::new();
    let mut live_activity = false;
    let entries = pane.transcript.as_ref().map_or(&[][..], |t| &t.entries[..]);
    for entry in entries {
        match entry {
            Entry::User {
                content,
                author,
                attachments,
                ..
            } => {
                question = Some(lines.len());
                bubble(&mut lines, content, author.as_deref(), width, theme, false);
                attachment_lines(&mut lines, attachments, width, theme);
            }
            Entry::Activity {
                title,
                details,
                progress,
                pending,
                steps,
                ..
            } => {
                live_activity |= *pending;
                let progress = if *pending {
                    pane.progress.as_ref().or(progress.as_ref())
                } else {
                    None
                };
                activity_lines(
                    &mut lines,
                    ActivityView {
                        title,
                        details: details.as_deref(),
                        progress: progress.map(String::as_str),
                        pending: *pending,
                        steps,
                        expanded: pane.show_steps || *pending,
                    },
                    width,
                    theme,
                    now,
                );
            }
            Entry::Assistant {
                id,
                content,
                sources,
                answered_with,
            } => {
                answered.push(*id);
                let text = if content.trim().is_empty() {
                    pane.streamed.get(id).map_or("", String::as_str)
                } else {
                    content.as_str()
                };
                if !text.trim().is_empty() {
                    answer_lines(&mut lines, text, sources, width, theme);
                    // `.message__provenance`, under someone else's answer.
                    if let Some(person) = answered_with {
                        lines.push(Line::styled(
                            ellipsize(&format!("Answered with {person}'s sources"), width),
                            theme.faint(),
                        ));
                    }
                }
            }
            Entry::Notice { tone, text, action } => {
                let signal = if tone == "negative" {
                    Signal::Negative
                } else {
                    Signal::Attention
                };
                lines.extend(notice_line(text, signal, width, theme));
                if let Some(action) = action {
                    lines.push(Line::styled(
                        ellipsize(&format!("  {}: {}", action.label, action.url), width),
                        theme.muted(),
                    ));
                }
                lines.push(Line::raw(""));
            }
            Entry::Unknown => {}
        }
    }
    // A question still on its way goes before any answer streaming in
    // ahead of the server's copy of it: that answer is its own.
    if let Some(text) = &pane.pending_question {
        question = Some(lines.len());
        bubble(&mut lines, text, None, width, theme, true);
    }
    // Answers still being written that the last read didn't have yet.
    let mut writing = false;
    for (id, text) in &pane.streamed {
        if !answered.contains(id) && !text.trim().is_empty() {
            answer_lines(&mut lines, text, &[], width, theme);
            writing = true;
        }
    }
    if pane.working() && !live_activity {
        // As the web says it: "Writing" once the answer streams in.
        let summary = pane.open_summary();
        let word = if pane.stopping || summary.is_some_and(|c| c.stopping) {
            "Stopping…"
        } else if summary.is_some_and(|c| c.waiting_for_turn) {
            "Free models are busy. Retrying in a moment…"
        } else if writing || pane.streamed.values().any(|t| !t.trim().is_empty()) {
            "Writing"
        } else {
            "Thinking"
        };
        let mut spans = vec![working_mark(theme, now), Span::raw(" ")];
        spans.extend(theme.shimmer_text(word, sweep(now)));
        if let Some(progress) = &pane.progress {
            spans.push(Span::styled(format!(" · {progress}"), theme.faint()));
        }
        lines.push(Line::from(spans));
    }
    queue_lines(&mut lines, &mut marks, app, width, theme);
    waiting_lines(&mut lines, &mut marks, app, width, theme);
    if let Some(error) = &pane.error {
        lines.extend(notice_line(error, Signal::Negative, width, theme));
    }
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
    (lines, marks, question)
}

/// Messages sent while an answer runs, after the conversation as the web
/// shows them: right-aligned like questions, in quiet dashed bubbles, each
/// under who sent it (when someone else did), its tag and, for its sender,
/// the x that takes it back.
fn queue_lines(
    lines: &mut Vec<Line<'static>>,
    marks: &mut Vec<(usize, Hit)>,
    app: &App,
    width: usize,
    theme: &Theme,
) {
    let pane = &app.chat;
    let transcript = pane
        .transcript
        .as_ref()
        .filter(|t| Some(t.chat.number) == pane.open);
    let queued = transcript.map_or(&[][..], |t| &t.queued[..]);
    if queued.is_empty() && pane.pending_queued.is_empty() {
        return;
    }
    if lines.last().is_some_and(|l| l.width() > 0) {
        lines.push(Line::raw(""));
    }
    if let Some(note) = transcript.and_then(|t| t.queue_note()) {
        lines.push(Line::styled(note, theme.faint()).alignment(Alignment::Right));
        lines.push(Line::raw(""));
    }
    for item in queued {
        let unqueue = item.unqueueable() && pane.unqueueing != Some(item.id);
        if unqueue {
            marks.push((lines.len(), Hit::Unqueue(item.id)));
        }
        queued_bubble(
            lines,
            QueuedView {
                text: &item.content,
                author: item.author.as_deref(),
                tag: item.tag(),
                files: item.attachments.unwrap_or(0),
                unqueue,
            },
            width,
            theme,
        );
    }
    // Sent, and not in the queue yet.
    for pending in &pane.pending_queued {
        queued_bubble(
            lines,
            QueuedView {
                text: &pending.text,
                author: None,
                tag: if pending.steer {
                    "Queued"
                } else {
                    "After this answer"
                },
                files: pending.attachments.len() as u64,
                unqueue: false,
            },
            width,
            theme,
        );
    }
}

struct QueuedView<'a> {
    text: &'a str,
    author: Option<&'a str>,
    tag: &'a str,
    files: u64,
    unqueue: bool,
}

fn queued_bubble(lines: &mut Vec<Line<'static>>, item: QueuedView, width: usize, theme: &Theme) {
    let most = (width * 3 / 4).clamp(16, width);
    let mut body: Vec<(String, Style)> = Vec::new();
    if !item.text.trim().is_empty() {
        body.extend(
            wrap(item.text.trim(), most.saturating_sub(4))
                .into_iter()
                .map(|l| (l, theme.muted())),
        );
    }
    if item.files > 0 {
        body.push((format!("+ {}", files(item.files)), theme.faint()));
    }
    let inner = body
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let indent = " ".repeat(width.saturating_sub(inner + 4));
    let mut meta = Vec::new();
    if let Some(author) = item.author {
        meta.push(Span::styled(format!("{author}  "), theme.micro()));
    }
    meta.push(Span::styled(item.tag.to_string(), theme.muted()));
    if item.unqueue {
        meta.push(Span::styled("  ✕", theme.faint()));
    }
    lines.push(Line::from(meta).alignment(Alignment::Right));
    let edge = theme.line();
    let rule = "╌".repeat(inner + 2);
    lines.push(Line::from(vec![
        Span::raw(indent.clone()),
        Span::styled(format!("╭{rule}╮"), edge),
    ]));
    for (text, style) in body {
        let pad = inner - text.chars().count();
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled("┆ ", edge),
            Span::styled(format!("{text}{}", " ".repeat(pad)), style),
            Span::styled(" ┆", edge),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("╰{rule}╯"), edge),
    ]));
    lines.push(Line::raw(""));
}

/// The files sent with a question, under its bubble.
fn attachment_lines(
    lines: &mut Vec<Line<'static>>,
    attachments: &[Attachment],
    width: usize,
    theme: &Theme,
) {
    if attachments.is_empty() {
        return;
    }
    // Under the bubble's blank line.
    let blank = lines.pop();
    for file in attachments {
        let text = format!("+ {} · {}", file.filename, size(file.byte_size));
        lines.push(
            Line::styled(ellipsize(&text, width.saturating_sub(2)), theme.faint())
                .alignment(Alignment::Right),
        );
    }
    lines.extend(blank);
}

/// What the answer waits on: a change to approve, a question from a
/// tool's server, or someone else in a project chat.
fn waiting_lines(
    lines: &mut Vec<Line<'static>>,
    marks: &mut Vec<(usize, Hit)>,
    app: &App,
    width: usize,
    theme: &Theme,
) {
    let pane = &app.chat;
    let Some(transcript) = pane
        .transcript
        .as_ref()
        .filter(|t| !t.chat.processing() && Some(t.chat.number) == pane.open)
    else {
        return;
    };
    let focused = app.focus == Focus::Composer;
    for approval in transcript.approvals.iter().filter(|a| !a.decidable) {
        let who = approval.waiting_for.as_deref().unwrap_or("someone");
        lines.push(Line::from(vec![
            Span::styled(RING, theme.faint()),
            Span::styled(
                format!(
                    " Waiting for {who} to approve a change in {}",
                    approval.service
                ),
                theme.muted(),
            ),
        ]));
        lines.push(Line::raw(""));
    }
    for question in transcript.questions.iter().filter(|q| !q.decidable) {
        let who = question.waiting_for.as_deref().unwrap_or("someone");
        lines.push(Line::from(vec![
            Span::styled(RING, theme.faint()),
            Span::styled(
                format!(" Waiting for {who} to answer {}", question.service),
                theme.muted(),
            ),
        ]));
        lines.push(Line::raw(""));
    }
    if let Some(approval) = pane.pending_decision() {
        // The card's ring; its content is laid out four columns narrower.
        let start = lines.len();
        lines.push(card_edge(width, true, Signal::Attention, theme));
        let width = width.saturating_sub(4);
        let title = if pane.deciding {
            "Sending your answer…".to_string()
        } else {
            format!("Waiting for your approval · {}", approval.service)
        };
        lines.push(Line::from(vec![
            Span::styled(DOT, theme.signal(Signal::Attention)),
            Span::raw(" "),
            Span::styled(title, theme.strong()),
        ]));
        for part in wrap(&approval.summary, width.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(part, theme.ink()),
            ]));
        }
        approval_body(lines, approval, width, theme);
        let choices = ChatPane::choices(approval);
        let selected = pane.decision.min(choices.len() - 1);
        for (i, choice) in choices.iter().enumerate() {
            let text = match choice {
                Choice::Approve => "Approve",
                Choice::ApproveAll => "Approve for the rest of this chat",
                Choice::Deny => "Deny",
                Choice::DenyWithReason => "Deny, and say what to do instead",
            };
            marks.push((lines.len(), Hit::Choice(i)));
            lines.push(choice_line(
                &format!("{}. {text}", i + 1),
                focused && i == selected,
                theme,
            ));
        }
        tint_card(lines, start, width + 4, Signal::Attention, theme);
        lines.push(Line::raw(""));
    } else if let Some(question) = pane.pending_question() {
        let start = lines.len();
        lines.push(card_edge(width, true, Signal::Attention, theme));
        question_lines(lines, marks, app, question, width.saturating_sub(4), theme);
        // The form ends with a blank line of its own; the card closes over it.
        if lines.last().is_some_and(|l| l.width() == 0) {
            lines.pop();
        }
        tint_card(lines, start, width, Signal::Attention, theme);
        lines.push(Line::raw(""));
    }
}

/// What a change will do, as its card shows it: its fields, then the
/// preview of what it writes and its notes. The chat and Decisions both
/// show it before it's approved.
pub(super) fn approval_body(
    lines: &mut Vec<Line<'static>>,
    approval: &super::chat::Approval,
    width: usize,
    theme: &Theme,
) {
    detail_lines(
        lines,
        approval
            .fields()
            .iter()
            .map(|d| (d.label.as_str(), d.value.as_str())),
        width,
        theme,
    );
    if let Some(preview) = &approval.preview {
        if let Some(body) = &preview.body {
            answer_lines(lines, &body.markdown(), &[], width, theme);
        }
        for note in &preview.notes {
            let signal = if note.tone == "warning" {
                Signal::Attention
            } else {
                Signal::Idle
            };
            lines.extend(notice_line(&note.text, signal, width, theme));
        }
    }
}

/// The top or bottom of a card in the transcript, in the signal's ring.
fn card_edge(width: usize, top: bool, signal: Signal, theme: &Theme) -> Line<'static> {
    let (left, right) = if top { ("╭", "╮") } else { ("╰", "╯") };
    let fill = "─".repeat(width.saturating_sub(2));
    Line::styled(format!("{left}{fill}{right}"), theme.edge(signal))
}

/// Closes the card opened at `start` (its top edge): every line since gets
/// the ring at both sides, in the signal's colour, as the web rings a card
/// that needs someone; the terminal's own background stays behind it.
fn tint_card(
    lines: &mut Vec<Line<'static>>,
    start: usize,
    width: usize,
    signal: Signal,
    theme: &Theme,
) {
    let edge = theme.edge(signal);
    let inner = width.saturating_sub(4);
    for line in lines.iter_mut().skip(start + 1) {
        let pad = inner.saturating_sub(line.width());
        let mut spans = vec![Span::styled("│", edge), Span::raw(" ")];
        spans.extend(
            line.spans
                .drain(..)
                .map(|span| Span::styled(span.content, line.style.patch(span.style))),
        );
        spans.push(Span::raw(" ".repeat(pad + 1)));
        spans.push(Span::styled("│", edge));
        *line = Line::from(spans);
    }
    lines.push(card_edge(width, false, signal, theme));
}

/// `label  value` pairs, labels lined up.
fn detail_lines<'a>(
    lines: &mut Vec<Line<'static>>,
    details: impl Iterator<Item = (&'a str, &'a str)>,
    width: usize,
    theme: &Theme,
) {
    let details: Vec<(&str, &str)> = details.collect();
    let label_width = details
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(18);
    for (label, value) in details {
        let room = width.saturating_sub(label_width + 4).max(8);
        for (i, part) in wrap(value, room).into_iter().enumerate() {
            let label = if i == 0 {
                format!("  {:<label_width$}  ", ellipsize(label, label_width))
            } else {
                " ".repeat(label_width + 4)
            };
            lines.push(Line::from(vec![
                Span::styled(label, theme.faint()),
                Span::styled(part, theme.muted()),
            ]));
        }
    }
}

/// One answer to pick, marked when selected.
fn choice_line(text: &str, selected: bool, theme: &Theme) -> Line<'static> {
    if selected {
        Line::from(vec![
            Span::styled(PROMPT, theme.strong()),
            Span::raw(" "),
            Span::styled(text.to_string(), theme.strong()),
        ])
        .patch_style(theme.selected())
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(text.to_string(), theme.muted()),
        ])
    }
}

/// A question from a tool's server, as a form to fill in or a page to open.
fn question_lines(
    lines: &mut Vec<Line<'static>>,
    marks: &mut Vec<(usize, Hit)>,
    app: &App,
    question: &Question,
    width: usize,
    theme: &Theme,
) {
    let pane = &app.chat;
    let focused = app.focus == Focus::Composer && pane.form.editing.is_none();
    let title = if pane.deciding {
        "Sending your answer…".to_string()
    } else if question.is_url() {
        format!("{} asks you to open a page", question.service)
    } else {
        format!("{} asks", question.service)
    };
    lines.push(Line::from(vec![
        Span::styled(DOT, theme.signal(Signal::Attention)),
        Span::raw(" "),
        Span::styled(title, theme.strong()),
    ]));
    for part in wrap(&question.message, width.saturating_sub(2)) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(part, theme.ink()),
        ]));
    }
    let rows = ChatPane::question_rows(question);
    let selected = pane.form.row.min(rows.len() - 1);
    let label_width = question
        .fields
        .iter()
        .map(|f| f.label().chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    for (i, row) in rows.iter().enumerate() {
        let here = focused && i == selected;
        let line = match row {
            FormRow::Field(f) => {
                let field = &question.fields[*f];
                let value = pane.form.values.get(*f).map_or("", String::as_str);
                let (text, style) = if pane.form.editing == Some(*f) {
                    ("typing below…".to_string(), theme.faint())
                } else if !value.is_empty() {
                    (value.to_string(), theme.ink())
                } else if field_has_choices(field) {
                    let hint = if field.required {
                        "pick with ← →"
                    } else {
                        "–"
                    };
                    (hint.to_string(), theme.faint())
                } else if field.required {
                    (
                        "needed: enter to type it".to_string(),
                        theme.signal_ink(Signal::Attention),
                    )
                } else {
                    ("–".to_string(), theme.faint())
                };
                let label = format!("{:<label_width$}  ", ellipsize(field.label(), label_width));
                let room = width.saturating_sub(label_width + 4);
                let mut spans = vec![
                    if here {
                        Span::styled(PROMPT, theme.strong())
                    } else {
                        Span::raw(" ")
                    },
                    Span::raw(" "),
                    Span::styled(label, theme.muted()),
                    Span::styled(ellipsize(&text, room), style),
                ];
                if here && field_has_choices(field) {
                    spans.push(Span::styled("  ← →", theme.faint()));
                }
                let line = Line::from(spans);
                if here {
                    line.patch_style(theme.selected())
                } else {
                    line
                }
            }
            FormRow::OpenPage => {
                let host = question.host.as_deref().unwrap_or("the page");
                choice_line(&format!("Open {host} in the browser"), here, theme)
            }
            FormRow::Send if question.is_url() => choice_line("Done, carry on", here, theme),
            FormRow::Send => choice_line("Send", here, theme),
            FormRow::Decline => choice_line("Decline", here, theme),
        };
        marks.push((lines.len(), Hit::Ask(i)));
        lines.push(line);
    }
    if let Some(field) = rows
        .get(selected)
        .and_then(|r| match r {
            FormRow::Field(f) => question.fields.get(*f),
            _ => None,
        })
        .filter(|_| focused)
        && let Some(description) = &field.description
    {
        for part in wrap(description, width.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(part, theme.faint()),
            ]));
        }
    }
    if let Some(note) = &question.note {
        for part in wrap(note, width.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(part, theme.faint()),
            ]));
        }
    }
    lines.push(Line::raw(""));
}

/// A question, as a bubble on the right: its text in a rounded outline,
/// since the terminal's background is never painted. A message still
/// waiting in the queue has a dashed outline and dim text.
fn bubble(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    author: Option<&str>,
    width: usize,
    theme: &Theme,
    pending: bool,
) {
    let most = (width * 3 / 4).clamp(16, width);
    let wrapped = wrap(text.trim(), most.saturating_sub(4));
    // In terminal columns, so a wide character keeps the outline straight.
    let columns = |line: &str| Span::raw(line.to_string()).width();
    let inner = wrapped.iter().map(|l| columns(l)).max().unwrap_or(0);
    let indent = " ".repeat(width.saturating_sub(inner + 4));
    if let Some(author) = author {
        lines.push(Line::styled(author.to_string(), theme.micro()).alignment(Alignment::Right));
    }
    let (text_style, side, rule) = if pending {
        (theme.faint(), "┆", "┄")
    } else {
        (theme.ink(), "│", "─")
    };
    let edge = theme.line();
    let across = rule.repeat(inner + 2);
    lines.push(Line::from(vec![
        Span::raw(indent.clone()),
        Span::styled(format!("╭{across}╮"), edge),
    ]));
    for line in wrapped {
        let pad = inner.saturating_sub(columns(&line));
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(side, edge),
            Span::styled(format!(" {line}{} ", " ".repeat(pad)), text_style),
            Span::styled(side, edge),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("╰{across}╯"), edge),
    ]));
    lines.push(Line::raw(""));
}

struct ActivityView<'a> {
    title: &'a str,
    details: Option<&'a str>,
    progress: Option<&'a str>,
    pending: bool,
    steps: &'a [super::chat::Step],
    expanded: bool,
}

/// The tool work between two things said: one line, like the web's
/// collapsed activity, and its steps under it when expanded.
fn activity_lines(
    lines: &mut Vec<Line<'static>>,
    activity: ActivityView,
    width: usize,
    theme: &Theme,
    now: OffsetDateTime,
) {
    let mut spans = if activity.pending {
        let mut spans = vec![working_mark(theme, now), Span::raw(" ")];
        spans.extend(theme.shimmer_text(activity.title, sweep(now)));
        spans
    } else {
        let caret = if activity.expanded { "▾" } else { "▸" };
        vec![
            Span::styled(caret, theme.faint()),
            Span::raw(" "),
            Span::styled(activity.title.to_string(), theme.muted()),
        ]
    };
    if let Some(details) = activity.progress.or(activity.details) {
        let used: usize = spans.iter().map(Span::width).sum();
        let room = width.saturating_sub(used + 3);
        if room > 4 {
            spans.push(Span::styled(
                format!(" · {}", ellipsize(details, room)),
                theme.faint(),
            ));
        }
    }
    lines.push(Line::from(spans));
    if activity.expanded {
        for step in activity.steps {
            let (glyph, style) = if step.pending {
                (RING, theme.signal(Signal::Live))
            } else if step.tone.is_some() {
                ("!", theme.signal(Signal::Attention))
            } else if step.troubled() {
                ("⊘", theme.faint())
            } else {
                ("·", theme.faint())
            };
            let badge = [step.account.as_deref(), step.badge.as_deref()]
                .into_iter()
                .flatten()
                .map(|b| format!(" · {b}"))
                .reduce(|a, b| a + &b);
            let room = width.saturating_sub(4 + badge.as_deref().map_or(0, str::len));
            let mut spans = vec![
                Span::raw("  "),
                Span::styled(glyph, style),
                Span::raw(" "),
                Span::styled(ellipsize(&step.summary, room), theme.muted()),
            ];
            if let Some(badge) = badge {
                spans.push(Span::styled(badge, theme.faint()));
            }
            lines.push(Line::from(spans));
            if let Some(detail) = &step.details {
                for line in wrap(detail, width.saturating_sub(6)) {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(line, theme.faint()),
                    ]));
                }
            }
            if let Some(link) = &step.link {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        ellipsize(
                            &format!("{}: {}", link.label, link.url),
                            width.saturating_sub(4),
                        ),
                        theme.faint(),
                    ),
                ]));
            }
            if let Some(plan) = &step.plan {
                plan_lines(lines, plan, width, theme);
            }
            if let Some(code) = &step.code {
                answer_lines(lines, &code.markdown(), &[], width, theme);
            }
            for file in &step.files {
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(ellipsize(file, width.saturating_sub(6)), theme.faint()),
                ]));
            }
        }
    }
    for file in activity.steps.iter().flat_map(|s| &s.downloads) {
        lines.push(Line::styled(
            ellipsize(
                &format!("↓ {} · {}", file.filename, size(file.byte_size)),
                width,
            ),
            theme.ink(),
        ));
    }
    lines.push(Line::raw(""));
}

/// The assistant's plan, as the web's checklist shows it: a mark per item,
/// done ones ticked and struck through, removed ones struck through.
fn plan_lines(
    lines: &mut Vec<Line<'static>>,
    plan: &super::chat::Plan,
    width: usize,
    theme: &Theme,
) {
    if plan.items.is_empty() {
        return;
    }
    for item in &plan.items {
        let (mark, mark_style, text_style) = if item.done() {
            (
                "✓",
                theme.signal(Signal::Positive),
                theme.faint().add_modifier(Modifier::CROSSED_OUT),
            )
        } else if item.removed() {
            (
                "○",
                theme.signal(Signal::Negative),
                theme.faint().add_modifier(Modifier::CROSSED_OUT),
            )
        } else {
            ("○", theme.faint(), theme.ink())
        };
        for (n, line) in wrap(&item.content, width.saturating_sub(8))
            .into_iter()
            .enumerate()
        {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(if n == 0 { mark } else { " " }, mark_style),
                Span::raw(" "),
                Span::styled(line, text_style),
            ]));
        }
    }
}

/// An answer: Markdown, then its sources and links as footnotes.
fn answer_lines(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    sources: &[super::chat::Source],
    width: usize,
    theme: &Theme,
) {
    let mut notes = Footnotes::new(sources.iter().map(|s| (s.title.clone(), s.url.clone())));
    lines.extend(markdown::render(text, width, theme, &mut notes));
    if !notes.notes.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("SOURCES", theme.micro()));
        lines.extend(markdown::footnote_lines(&notes, width, theme));
    }
    lines.push(Line::raw(""));
}

pub(super) fn notice_line(
    text: &str,
    signal: Signal,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(2))
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let glyph = if i == 0 { DOT } else { " " };
            Line::from(vec![
                Span::styled(glyph, theme.signal(signal)),
                Span::raw(" "),
                Span::styled(l, theme.signal_ink(signal)),
            ])
        })
        .collect()
}

/// What the composer box holds: the prompt line or what's typed, the
/// files to send, and the slash list or a picker under them.
pub struct ComposerView {
    lines: Vec<Line<'static>>,
    /// Where the cursor goes, as (column, line), when it shows.
    cursor: Option<(u16, u16)>,
    /// The lines a click picks.
    rows: Vec<(u16, Hit)>,
}

/// Rows of a list shown at once under the composer.
const LIST_ROWS: usize = 8;

/// Lines of what's typed shown at once.
const INPUT_ROWS: usize = 8;

/// The composer's content for a box `width` columns wide inside.
fn composer_view(app: &App, theme: &Theme, width: usize, now: OffsetDateTime) -> ComposerView {
    let _ = now;
    let pane = &app.chat;
    let locked = pane.locked_reason();
    let usable = pane.ready() && locked.is_none();
    let focused = usable && app.focus == Focus::Composer && app.modal.is_none();
    let room = width.saturating_sub(3).max(4);
    let prompt = || Span::styled(PROMPT, theme.strong());
    let mut view = ComposerView {
        lines: Vec::new(),
        cursor: None,
        rows: Vec::new(),
    };

    if let Some(picker) = pane.picker.as_ref().filter(|_| usable) {
        let label = match picker.kind {
            PickerKind::Chats => "Resume a chat: ",
            PickerKind::Models => "Model: ",
            PickerKind::Projects => "Project: ",
        };
        let query = tail(&picker.query, room.saturating_sub(label.chars().count()));
        view.cursor = focused.then_some((
            (2 + label.chars().count() + query.chars().count()) as u16,
            0,
        ));
        view.lines.push(Line::from(vec![
            prompt(),
            Span::raw(" "),
            Span::styled(label, theme.faint()),
            Span::styled(query, theme.ink()),
        ]));
        picker_rows(&mut view, app, theme, width);
        return view;
    }

    let typed = !pane.input.is_empty();
    match (&pane.access, locked) {
        (Access::Ready, Some(reason)) => view.lines.push(Line::from(vec![
            prompt(),
            Span::raw(" "),
            Span::styled(ellipsize(reason, room), theme.signal_ink(Signal::Attention)),
        ])),
        (Access::Ready, None) if typed => {
            let (rows, (row, col)) = wrap_input(pane.input.text(), pane.input.cursor(), room);
            // Keep the cursor's line in view.
            let first = (row + 1).saturating_sub(INPUT_ROWS);
            for (i, text) in rows.iter().enumerate().skip(first).take(INPUT_ROWS) {
                let lead = if i == 0 { prompt() } else { Span::raw(" ") };
                view.lines.push(Line::from(vec![
                    lead,
                    Span::raw(" "),
                    Span::styled(text.clone(), theme.ink()),
                ]));
            }
            view.cursor = focused.then_some(((col + 2) as u16, (row - first) as u16));
            // While an answer runs, the web's two send buttons, as keys.
            if pane.queues() {
                let hint = "enter joins this answer · alt-enter sends after it · esc stops";
                view.lines.push(
                    Line::styled(ellipsize(hint, room), theme.faint()).alignment(Alignment::Right),
                );
            }
        }
        (Access::Ready, None) => {
            let editing = pane.form.editing.and_then(|i| {
                pane.pending_question()
                    .and_then(|q| q.fields.get(i))
                    .map(|f| f.label().to_string())
            });
            let text = if let Some(label) = editing {
                format!("{label}: type it, then enter")
            } else if pane.reason {
                "Say what to do instead, then enter".into()
            } else if pane.pending_decision().is_some() {
                "Approve or deny above: ↑ ↓ and enter, or y and n".into()
            } else if pane.pending_question().is_some() {
                "Answer above: ↑ ↓ and enter".into()
            } else if pane.working() && pane.stopping {
                "Stopping…".into()
            } else if pane.queues() {
                "Add to this answer, or queue what's next".into()
            } else if pane.working() {
                "Writing…  esc stops".into()
            } else if pane.open.is_some() {
                "Reply to Chat with Work".into()
            } else {
                "Ask anything…".into()
            };
            let mut spans = vec![
                prompt(),
                Span::raw(" "),
                Span::styled(ellipsize(&text, room), theme.faint()),
            ];
            // The one hint, as the desktop app's composer has its own.
            let hint = if pane.queues() && !pane.stopping {
                "esc stops"
            } else {
                "/ for commands"
            };
            let used = text.chars().count() + 2;
            let shown = !pane.working() || (pane.queues() && !pane.stopping);
            if shown && focused && used + hint.chars().count() + 4 <= width {
                spans.push(Span::raw(" ".repeat(width - used - hint.chars().count())));
                spans.push(Span::styled(hint, theme.faint()));
            }
            view.lines.push(Line::from(spans));
            view.cursor = focused.then_some((2, 0));
        }
        (access, _) => {
            let text = match access {
                Access::Loading => "Loading your chats…",
                Access::NeedsApproval {
                    requested: false, ..
                } => "Chats need your OK first: press o",
                Access::NeedsApproval { .. } => "Waiting for your OK in Chat with Work",
                Access::Unavailable(_) => "Chats aren't available here",
                _ => match app.chat_daemon() {
                    Daemon::NotRunning(_) => "Start the daemon to chat here: press s",
                    Daemon::Running(status)
                        if !status.paired || status.connection.connection == "revoked" =>
                    {
                        "Pair this computer to chat here: press c"
                    }
                    _ => "Looking for the daemon…",
                },
            };
            view.lines.push(Line::from(vec![
                prompt(),
                Span::raw(" "),
                Span::styled(ellipsize(text, room), theme.faint()),
            ]));
        }
    }
    if usable {
        for file in &pane.attachments {
            let state = match file.uploaded {
                Some((_, bytes)) => size(bytes),
                None => "uploading…".into(),
            };
            view.lines.push(Line::styled(
                ellipsize(&format!("  + {} · {state}", file.name), width),
                theme.faint(),
            ));
        }
    }
    menu_rows(&mut view, app, theme, width);
    view
}

/// What's typed, cut into rows of `room` characters at line breaks and
/// where a line is too long, and the cursor's (row, column) in them.
fn wrap_input(text: &str, cursor: usize, room: usize) -> (Vec<String>, (usize, usize)) {
    let room = room.max(1);
    let mut rows = Vec::new();
    let mut at = (0, 0);
    let mut offset = 0;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let first = rows.len();
        if chars.is_empty() {
            rows.push(String::new());
        }
        for chunk in chars.chunks(room) {
            rows.push(chunk.iter().collect());
        }
        if (offset..=offset + line.len()).contains(&cursor) {
            let column = line[..cursor - offset].chars().count();
            let row = (column / room).min(rows.len() - first - 1);
            at = (first + row, column - row * room);
        }
        offset += line.len() + 1;
    }
    (rows, at)
}

/// The slash commands that match, under what's typed.
fn menu_rows(view: &mut ComposerView, app: &App, theme: &Theme, width: usize) {
    let menu = app.menu();
    if menu.is_empty() {
        return;
    }
    let selected = app.chat.menu.min(menu.len() - 1);
    let first = (selected + 1).saturating_sub(LIST_ROWS);
    let name_width = 22;
    for (i, command) in menu.iter().enumerate().skip(first).take(LIST_ROWS) {
        let here = i == selected;
        let mut name = format!("/{}", command.name);
        if !command.args.is_empty() {
            name.push(' ');
            name.push_str(command.args);
        }
        let about = ellipsize(command.about, width.saturating_sub(name_width + 2));
        let mut line = Line::from(vec![
            if here {
                Span::styled("▌", theme.ink())
            } else {
                Span::raw(" ")
            },
            Span::styled(
                format!("{:<name_width$}", ellipsize(&name, name_width - 1)),
                if here { theme.strong() } else { theme.ink() },
            ),
            Span::styled(about, theme.faint()),
        ]);
        if here {
            line = line.patch_style(theme.selected());
        }
        view.rows.push((view.lines.len() as u16, Hit::Menu(i)));
        view.lines.push(line);
    }
}

/// The open picker's rows, filtered by what's typed.
fn picker_rows(view: &mut ComposerView, app: &App, theme: &Theme, width: usize) {
    let pane = &app.chat;
    let Some(picker) = &pane.picker else { return };
    let items = pane.pick_items();
    if items.is_empty() {
        let text = match (&pane.models, picker.kind) {
            (ModelList::Loading, PickerKind::Models) => " Loading models…",
            _ => " No match",
        };
        view.lines.push(Line::styled(text, theme.faint()));
        return;
    }
    let selected = picker.selected.min(items.len() - 1);
    let first = (selected + 1).saturating_sub(LIST_ROWS);
    let current = pane.current_model_id();
    for (i, item) in items.iter().enumerate().skip(first).take(LIST_ROWS) {
        let here = i == selected;
        let marker = if here {
            Span::styled("▌", theme.ink())
        } else {
            Span::raw(" ")
        };
        let name_style = if here { theme.strong() } else { theme.ink() };
        let mut spans = vec![marker];
        match item {
            PickItem::Chat(chat) => {
                let mut rest = format!(" · #{}", chat.number);
                if let Some(project) = &chat.project {
                    rest.push_str(&format!(" · {}", project.name));
                }
                let title = ellipsize(
                    &chat.title,
                    width.saturating_sub(rest.chars().count() + 2).max(8),
                );
                spans.push(Span::styled(title, name_style));
                spans.push(Span::styled(rest, theme.faint()));
            }
            PickItem::Model(model) => {
                let style = if model.selectable {
                    name_style
                } else {
                    theme.faint()
                };
                spans.push(Span::styled(model.name.clone(), style));
                if current == Some(model.id.as_str()) {
                    spans.push(Span::styled(
                        "  current",
                        theme.signal_ink(Signal::Positive),
                    ));
                }
                let detail = if model.selectable {
                    model.rate.clone().unwrap_or_else(|| model.provider.clone())
                } else {
                    model.reason.clone().unwrap_or_default()
                };
                let used: usize = spans.iter().map(Span::width).sum();
                let room = width.saturating_sub(used + 3);
                if room > 4 && !detail.is_empty() {
                    let style = if model.selectable {
                        theme.faint()
                    } else {
                        theme.signal_ink(Signal::Attention)
                    };
                    spans.push(Span::styled(
                        format!(" · {}", ellipsize(&detail, room)),
                        style,
                    ));
                }
            }
            PickItem::Project(project) => {
                let name = project.map_or("No project", |p| p.name.as_str());
                spans.push(Span::styled(
                    ellipsize(name, width.saturating_sub(2)),
                    name_style,
                ));
                if project.map(|p| p.id) == pane.project.as_ref().map(|p| p.id) {
                    spans.push(Span::styled(
                        "  current",
                        theme.signal_ink(Signal::Positive),
                    ));
                }
            }
        }
        let mut line = Line::from(spans);
        if here {
            line = line.patch_style(theme.selected());
        }
        view.rows.push((view.lines.len() as u16, Hit::Pick(i)));
        view.lines.push(line);
    }
}

/// The composer, with its edge saying what's happening: a dim outline at
/// rest and while it can't send, yellow while a change or a question waits
/// for the person, and the rainbow flowing in ANSI colours while an answer
/// is written. With `NO_COLOR`, bold while it works.
fn composer_box(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    now: OffsetDateTime,
    view: ComposerView,
    hits: &mut Hits,
) {
    let pane = &app.chat;
    let usable = pane.ready() && pane.locked_reason().is_none();
    let block = Block::bordered().border_type(BorderType::Rounded);
    let inner = pad(block.inner(area), 1, 0);
    frame.render_widget(block, area);
    let phase = if pane.working() {
        // One trip around every two seconds.
        (now.unix_timestamp_nanos() % 2_000_000_000) as f32 / 2_000_000_000.0
    } else {
        0.0
    };
    let edge = if !usable {
        Edge::Plain(theme.line())
    } else if pane.pending_decision().is_some() || pane.pending_question().is_some() {
        Edge::Plain(theme.edge(Signal::Attention))
    } else if phase > 0.0 {
        Edge::Rainbow(phase)
    } else {
        Edge::Plain(theme.line())
    };
    composer_edge(frame.buffer_mut(), area, theme, edge);
    // The model, on the bottom edge, where the desktop app shows it.
    if let Some(model) = pane.model_name().filter(|_| usable && area.height >= 3) {
        let label = format!(
            " {} ",
            ellipsize(model, (area.width as usize).saturating_sub(8))
        );
        let width = label.chars().count() as u16;
        if width + 4 <= area.width {
            let at = Rect::new(area.right() - 2 - width, area.bottom() - 1, width, 1);
            frame.render_widget(Paragraph::new(Line::styled(label, theme.faint())), at);
        }
    }
    hits.add(area, Hit::Composer);
    for (at, hit) in view.rows {
        if at < inner.height {
            hits.add(row(inner, at), hit);
        }
    }
    frame.render_widget(Paragraph::new(view.lines), inner);
    if let Some((x, y)) = view.cursor
        && y < inner.height
    {
        frame.set_cursor_position(Position::new(
            (inner.x + x).min(inner.right().saturating_sub(1)),
            inner.y + y,
        ));
    }
}

enum Edge {
    Plain(Style),
    /// Flowing, shifted by this much of the way round.
    Rainbow(f32),
}

/// Draws the composer's edge in `edge`'s colours, foreground only.
fn composer_edge(buf: &mut Buffer, area: Rect, theme: &Theme, edge: Edge) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let span = (area.width - 1) as f32;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let on_edge = y == area.top()
                || y == area.bottom() - 1
                || x == area.left()
                || x == area.right() - 1;
            if !on_edge {
                continue;
            }
            let style = match edge {
                Edge::Plain(style) => style,
                Edge::Rainbow(phase) => {
                    // The gradient runs there and back so it never jumps.
                    let along = (x - area.left()) as f32 / span;
                    let shifted = (along + phase) % 1.0;
                    theme.rainbow(1.0 - (2.0 * shifted - 1.0).abs())
                }
            };
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_style(style);
            }
        }
    }
}

// -------------------------------------------------------------- audit log

pub(super) fn log_view(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let mut status = format!("{} entries", app.audit.len());
    if app.log_scroll > 0 {
        status.push_str(" · end follows");
    }
    let header = spread(
        vec![Span::styled(
            format!("{:<10}{:<9}{:<12}{}", "TIME", "RESULT", "TOOL", "DETAIL"),
            theme.micro(),
        )],
        vec![Span::styled(status, theme.faint())],
        area.width,
    );
    frame.render_widget(Paragraph::new(header), row(area, 0));
    let body = below(area, 1);
    if app.audit.is_empty() {
        let text = wrap(
            "No entries yet. Every request Chat with Work makes shows up here, answered or \
             refused, as it happens.",
            body.width.min(72) as usize,
        );
        frame.render_widget(
            Paragraph::new(
                text.into_iter()
                    .map(|l| Line::styled(l, theme.faint()))
                    .collect::<Vec<_>>(),
            ),
            below(body, 1),
        );
        return;
    }
    // Newest at the bottom; an entry may take more than one line.
    let end = app.audit.len().saturating_sub(app.log_scroll);
    let mut lines: Vec<Line> = Vec::new();
    for entry in app.audit[..end].iter().rev() {
        let mut entry_lines = log_lines(entry, app, theme, body.width as usize);
        if lines.len() + entry_lines.len() > body.height as usize {
            break;
        }
        entry_lines.append(&mut lines);
        lines = entry_lines;
    }
    frame.render_widget(Paragraph::new(lines), body);
}

/// `text` padded to `width`, and always followed by at least one space, so
/// a long event name can't run into the next column.
fn column(text: &str, width: usize) -> String {
    if text.chars().count() < width {
        format!("{text:<width$}")
    } else {
        format!("{text} ")
    }
}

/// Where the DETAIL column starts.
const LOG_DETAIL: usize = 31;

/// One entry: a line, plus the reason under the detail for refusals.
fn log_lines(entry: &AuditEntry, app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let time = parse_ts(&entry.ts)
        .map(|t| clock_secs(t, app))
        .unwrap_or_else(|| "?".into());
    let mut spans = vec![Span::styled(format!("{time:<10}"), theme.faint())];
    let mut reason = None;
    if entry.event == "tool" {
        let signal = entry_signal(entry);
        let word = match entry.decision {
            Some(Decision::Allowed) => "allowed",
            Some(Decision::Denied) => "denied",
            Some(Decision::Error) => "error",
            None => "?",
        };
        spans.push(Span::styled(format!("{word:<9}"), theme.signal_ink(signal)));
        spans.push(Span::styled(
            column(entry.tool.as_deref().unwrap_or("?"), 12),
            theme.ink(),
        ));
        let target = target(entry);
        if !target.is_empty() {
            spans.push(Span::styled(target, theme.muted()));
        }
        match entry.decision {
            Some(Decision::Denied | Decision::Error) => {
                reason = entry.reason.clone().or_else(|| entry.code.clone());
            }
            _ => {
                spans.push(Span::styled(" → ", theme.faint()));
                spans.push(Span::styled(outcome(entry).0, theme.muted()));
                if let Some(ms) = entry.duration_ms {
                    spans.push(Span::styled(format!(" · {}", duration(ms)), theme.faint()));
                }
            }
        }
        if let Some(chat) = &entry.chat_id {
            spans.push(Span::styled(format!(" · chat {chat}"), theme.faint()));
        }
    } else {
        spans.push(Span::styled(format!("{:<9}", "·"), theme.faint()));
        spans.push(Span::styled(
            column(&entry.event.replace('_', " "), 12),
            theme.signal_ink(entry_signal(entry)),
        ));
        if let Some(detail) = &entry.detail {
            spans.push(Span::styled(detail.clone(), theme.muted()));
        }
    }
    let mut lines = vec![Line::from(spans)];
    if let Some(reason) = reason {
        let indent = " ".repeat(LOG_DETAIL);
        for text in wrap(&reason, width.saturating_sub(LOG_DETAIL)) {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(text, theme.signal_ink(Signal::Negative)),
            ]));
        }
    }
    lines
}

fn entry_signal(entry: &AuditEntry) -> Signal {
    if entry.event == "tool" {
        return match entry.decision {
            Some(Decision::Allowed) => Signal::Positive,
            Some(Decision::Denied | Decision::Error) => Signal::Negative,
            None => Signal::Idle,
        };
    }
    match entry.event.as_str() {
        "connected" | "resumed" => Signal::Positive,
        "revoked" => Signal::Negative,
        "disconnected" | "paused" | "stopped" | "shutdown_requested" => Signal::Attention,
        _ => Signal::Live,
    }
}

/// `0.4 ms`, `12 ms`, `1.2 s`.
fn duration(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

fn target(entry: &AuditEntry) -> String {
    match (&entry.query, &entry.path, &entry.to) {
        (Some(query), _, _) => format!("\"{query}\""),
        (None, Some(path), Some(to)) => format!("{path} to {to}"),
        (None, Some(path), None) => path.clone(),
        (None, None, _) => String::new(),
    }
}

/// What a change did, in words, for the activity log.
fn change_outcome(entry: &AuditEntry) -> Option<String> {
    if entry.dry_run == Some(true) {
        return Some("checked, nothing changed".into());
    }
    let words = match entry.effect.as_deref()? {
        "created" => "created",
        "replaced" => "replaced, old one in the trash",
        "appended" => "added to, old one in the trash",
        "edited" => "edited, old one in the trash",
        "created_folder" => "folder made",
        "moved" if entry.trash.is_some() => "moved, old one in the trash",
        "moved" => "moved",
        "trashed" => "in the trash",
        "unchanged" => "already there",
        // A command: how it ended and what it was, "exit 0 · make test".
        "ran" => return entry.detail.clone().or(Some("ran".into())),
        "stopped" => "stopped",
        other => other,
    };
    Some({
        match entry.written {
            Some(b) if b > 0 => format!("{words} · {}", bytes(b as usize)),
            _ => words.to_string(),
        }
    })
}

fn outcome(entry: &AuditEntry) -> (String, Signal) {
    let why = || {
        entry
            .reason
            .clone()
            .or_else(|| entry.code.clone())
            .unwrap_or_default()
    };
    match entry.decision {
        Some(Decision::Denied) => (format!("denied: {}", why()), Signal::Negative),
        Some(Decision::Error) => (format!("failed: {}", why()), Signal::Negative),
        _ if entry.effect.is_some() => (change_outcome(entry).unwrap_or_default(), Signal::Idle),
        _ => {
            let n = entry.results;
            let text = match (entry.tool.as_deref(), n, entry.bytes) {
                (Some("search"), Some(n), _) => plural(n, "hit", "hits"),
                (Some("list"), Some(n), _) => plural(n, "entry", "entries"),
                (Some("roots"), Some(n), _) => plural(n, "folder", "folders"),
                (_, _, Some(b)) => bytes(b),
                (_, Some(n), None) => plural(n, "result", "results"),
                _ => "done".into(),
            };
            (text, Signal::Idle)
        }
    }
}

// ----------------------------------------------------------------- modals

/// The keys an open dialog takes.
fn modal_hints(modal: &Modal) -> Vec<(&'static str, &'static str)> {
    match modal {
        Modal::AddRoot { .. } => vec![("enter", "share"), ("esc", "cancel")],
        Modal::RenameRoot { .. } => vec![("enter", "rename"), ("esc", "cancel")],
        Modal::ConfirmRemove { .. } => vec![("y", "stop sharing"), ("n", "keep it")],
        Modal::ConfirmChanges { .. } => vec![("y", "allow changes"), ("n", "keep it read-only")],
        Modal::ConfirmCommands { .. } => vec![("y", "allow commands"), ("n", "run none")],
        Modal::ConfirmBroad { .. } => vec![("y", "share anyway"), ("n", "cancel")],
        Modal::ConfirmDelete { .. } => vec![("y", "delete it"), ("n", "keep it")],
        Modal::ConfirmLogout => vec![("y", "disconnect"), ("n", "keep it")],
        Modal::CrashReports => vec![("y", "send crash reports"), ("n", "don't send")],
        Modal::Help { .. } => vec![("↑↓", "scroll"), ("any other key", "close")],
        Modal::Commands { .. } => vec![("↑↓", "choose"), ("s", "stop it"), ("esc", "close")],
    }
}

pub(super) fn key_hints(keys: &[(&'static str, &'static str)], theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(*key, theme.strong()));
        spans.push(Span::styled(format!(" {what}"), theme.faint()));
    }
    Line::from(spans)
}

fn modal_box(frame: &mut Frame, area: Rect, modal: &Modal, app: &App, theme: &Theme) {
    // What a command prints needs the room.
    let widest = if matches!(modal, Modal::Commands { .. }) {
        100
    } else {
        68
    };
    let width = area.width.saturating_sub(4).min(widest);
    let text_width = width.saturating_sub(4) as usize;
    let (title, lines) = match modal {
        Modal::AddRoot { input } => {
            let mut spans = vec![Span::styled(PROMPT, theme.strong()), Span::raw(" ")];
            spans.push(Span::styled(
                tail(input, text_width.saturating_sub(3)),
                theme.ink(),
            ));
            let mut lines = vec![
                Line::styled("The folder to share with Chat with Work:", theme.muted()),
                Line::raw(""),
                Line::from(spans),
                Line::raw(""),
            ];
            lines.extend(
                wrap(
                    "Only this folder becomes searchable. Keys, .env files and password \
                     databases inside it stay private.",
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.faint())),
            );
            ("SHARE A FOLDER", lines)
        }
        Modal::RenameRoot { id, input } => {
            let spans = vec![
                Span::styled(PROMPT, theme.strong()),
                Span::raw(" "),
                Span::styled(tail(input, text_width.saturating_sub(3)), theme.ink()),
            ];
            let mut lines = vec![
                Line::styled(
                    "The name Chat with Work sees this folder by:",
                    theme.muted(),
                ),
                Line::raw(""),
                Line::from(spans),
                Line::raw(""),
            ];
            lines.extend(
                wrap(
                    &format!(
                        "Its files keep their paths under {id}:, so links to them keep working."
                    ),
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.faint())),
            );
            ("RENAME A FOLDER", lines)
        }
        Modal::ConfirmRemove { label, path, .. } => {
            let mut lines = vec![Line::styled(
                format!("Stop sharing {label}?"),
                theme.strong(),
            )];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    &format!(
                        "Chat with Work can no longer search or read {path}, and its entries \
                         leave the index. The folder itself isn't touched."
                    ),
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("STOP SHARING", lines)
        }
        Modal::ConfirmChanges { label, .. } => {
            let mut lines = vec![Line::styled(
                format!("Allow changes in {label}?"),
                theme.strong(),
            )];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    "Chat with Work could then create, edit, move and delete files in this \
                     folder. It asks you before each change, and old versions and deleted \
                     files go to the trash on this computer.",
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("ALLOW CHANGES", lines)
        }
        Modal::ConfirmCommands { label, .. } => {
            let mut lines = vec![Line::styled(
                format!("Run commands in {label}?"),
                theme.strong(),
            )];
            lines.push(Line::raw(""));
            // The same words on every system, as its snapshot is.
            let network = "Its network is the folder's: by default this computer's own \
                           services, like a database (none on Windows), and the local network \
                           or the internet only if you allow them (i).";
            lines.extend(
                wrap(
                    &format!(
                        "Chat with Work could then run commands here, like tests or a build. \
                         Each one is approved in Chat with Work, or runs without asking in a \
                         chat where you chose Allow everything. A command runs in a sandbox \
                         that can change or delete anything in this folder and nothing outside \
                         it. {network}"
                    ),
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("RUN COMMANDS", lines)
        }
        Modal::ConfirmBroad { path, reason } => {
            let mut lines = vec![Line::styled(
                format!("Share {path} anyway?"),
                theme.strong(),
            )];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    &format!(
                        "{reason}. Secrets inside it stay private, but everything else in it \
                         becomes searchable by Chat with Work."
                    ),
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("A BROAD FOLDER", lines)
        }
        Modal::ConfirmDelete { title, .. } => {
            let mut lines = vec![Line::styled(format!("Delete {title}?"), theme.strong())];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    "It's gone from every list at once, its public link stops working, and \
                     Chat with Work purges it after 30 days.",
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("DELETE CHAT", lines)
        }
        Modal::CrashReports => {
            let mut lines = vec![Line::styled(
                "Send crash reports to the Chat with Work team?",
                theme.strong(),
            )];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(crate::crash_reports::ABOUT, text_width)
                    .into_iter()
                    .map(|l| Line::styled(l, theme.muted())),
            );
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    "Change it any time in settings, General, or with crash_reports in \
                     config.toml.",
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("CRASH REPORTS", lines)
        }
        Modal::ConfirmLogout => {
            let mut lines = vec![Line::styled("Disconnect this computer?", theme.strong())];
            lines.push(Line::raw(""));
            lines.extend(
                wrap(
                    "This forgets its pairing: Chat with Work can no longer reach your folders, \
                     and chats stop here, until you pair again. Remove it in Chat with Work as \
                     well, under Settings, Computers.",
                    text_width,
                )
                .into_iter()
                .map(|l| Line::styled(l, theme.muted())),
            );
            ("DISCONNECT", lines)
        }
        Modal::Commands { selected } => {
            // Room for the borders, the blank line and the hints.
            let room = (area.height as usize).saturating_sub(6).max(4);
            (
                "COMMANDS ON THIS COMPUTER",
                super::running::dialog_lines(app, selected.as_deref(), text_width, room, theme),
            )
        }
        Modal::Help { scroll } => {
            let all = help_lines(app, theme);
            // Room for the borders, the blank line and the hints.
            let room = (area.height as usize).saturating_sub(4).max(1);
            let scroll = (*scroll).min(all.len().saturating_sub(room));
            (
                "KEYS AND COMMANDS",
                all.into_iter().skip(scroll).take(room).collect(),
            )
        }
    };
    let mut lines = lines;
    lines.push(Line::raw(""));
    lines.push(key_hints(&modal_hints(modal), theme));
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.line())
        .title(Span::styled(format!(" {title} "), theme.micro()));
    let inner = pad(block.inner(rect), 1, 0);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
    if let Modal::AddRoot { input } | Modal::RenameRoot { input, .. } = modal {
        let x = inner.x + 2 + tail(input, text_width.saturating_sub(3)).chars().count() as u16;
        frame.set_cursor_position(Position::new(
            x.min(inner.right().saturating_sub(1)),
            inner.y + 2,
        ));
    }
}

fn help_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    // Grouped by where the keys work, so a key that means one thing in
    // Shared folders and another on Account says so under each.
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    if app.chat.ready() {
        groups.push((
            "SIDEBAR",
            vec![
                ("↑ ↓  j k", "move through the sidebar"),
                ("enter", "open a chat or a bot; open or close a project"),
                ("→ ←  l h", "open or close a project, or step up out of it"),
                ("alt-↑ alt-↓", "open the previous or next chat"),
                ("c  n", "new chat"),
                ("+", "new chat in the selected project"),
                ("/", "search chats and bots"),
                ("f", "show all, only chats, or only bots"),
                ("[", "hide or show the sidebar"),
                ("tab", "move between the sidebar and the chat"),
            ],
        ));
        groups.push((
            "GO TO",
            vec![
                ("g d", "Decisions"),
                ("g s", "settings"),
                ("g c  g b  g a", "show only chats, only bots, or all"),
                ("g l", "the activity log"),
            ],
        ));
        groups.push((
            "IN A CHAT",
            vec![
                ("i", "write a message"),
                ("pgup pgdn", "scroll the chat"),
                ("e", "show every tool step"),
                ("o", "open the chat in the browser"),
            ],
        ));
        groups.push((
            "ON DECISIONS",
            vec![
                ("↑ ↓  j k", "the next or previous decision"),
                ("a", "approve"),
                ("A", "approve for the rest of the chat"),
                ("d", "deny"),
                ("enter  o", "open its chat"),
                ("r", "read them again"),
            ],
        ));
        groups.push((
            "ON A BOT",
            vec![
                ("↑ ↓  j k", "select a run"),
                ("enter", "open the run"),
                ("f", "All, Waiting, You took part, Failed"),
                ("o", "open the bot in the browser"),
            ],
        ));
        groups.push((
            "COMPOSER",
            vec![
                ("enter", "send; while an answer runs, it joins the answer"),
                ("alt-enter", "while an answer runs, send after it"),
                ("alt-↑", "take back your latest queued message"),
                ("/", "commands"),
                (
                    "shift-enter",
                    "a new line (also ctrl-j, \\ then enter, or alt-enter when idle)",
                ),
                ("↑ ↓", "earlier questions"),
                ("ctrl-a ctrl-e", "start and end of the line"),
                ("ctrl-u ctrl-k", "delete to the start or end of the line"),
                ("esc", "stop an answer; back to the chats"),
            ],
        ));
    } else if matches!(app.chat.access, Access::NeedsApproval { .. }) {
        groups.push(("CHATS", vec![("o", "ask to use your chats here")]));
    }
    groups.extend([
        (
            "SETTINGS",
            vec![
                (",", "open settings"),
                ("tab ← →", "the next or previous page"),
                ("↑ ↓  j k", "select a folder or a web page, scroll a list"),
                ("esc", "back to the chats"),
            ],
        ),
        (
            "SHARED FOLDERS",
            vec![
                ("r", "rename the selected folder"),
                ("w", "allow or stop changes in the selected folder"),
                ("m", "allow or stop commands in the selected folder"),
                (
                    "i",
                    "its commands' network: none, local services, or the internet too",
                ),
                ("d", "stop sharing the selected folder"),
            ],
        ),
        ("ACCOUNT", vec![("d", "disconnect this computer")]),
        (
            "ANYWHERE ELSE",
            vec![("r", "look for the daemon and read the chats again")],
        ),
        (
            "ANYWHERE",
            vec![
                ("a", "share a folder"),
                ("p", "pause or resume answering Chat with Work"),
                ("s", "start the daemon, when it isn't running"),
                ("c", "pair this computer, when it isn't"),
                ("?", "this help"),
                ("ctrl-l", "draw the screen again"),
                ("ctrl-c", "clear what's typed; twice quits"),
                ("q", "quit"),
                ("mouse", "click to select and open; the wheel scrolls"),
                ("shift-drag", "select text while the mouse is on"),
            ],
        ),
    ]);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (heading, keys) in groups {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(heading, theme.micro()));
        lines.extend(keys.into_iter().map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!("{key:<14}"), theme.strong()),
                Span::styled(what.to_string(), theme.muted()),
            ])
        }));
    }
    if app.chat.ready() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("COMMANDS", theme.micro()));
        for command in COMMANDS {
            let mut name = format!("/{}", command.name);
            if !command.args.is_empty() {
                name.push(' ');
                name.push_str(command.args);
            }
            let mut about = command.about.to_string();
            if !command.aliases.is_empty() {
                let aliases: Vec<String> =
                    command.aliases.iter().map(|a| format!("/{a}")).collect();
                about.push_str(&format!(" (also {})", aliases.join(", ")));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{name:<22}"), theme.strong()),
                Span::styled(about, theme.muted()),
            ]));
        }
        lines.push(Line::styled(
            "Start with // to ask something that begins with a slash.",
            theme.faint(),
        ));
    }
    lines
}

// ---------------------------------------------------------------- helpers

pub(super) fn pad(area: Rect, x: u16, y: u16) -> Rect {
    let x = x.min(area.width / 2);
    let y = y.min(area.height / 2);
    Rect::new(area.x + x, area.y + y, area.width - 2 * x, area.height - y)
}

pub(super) fn row(area: Rect, offset: u16) -> Rect {
    Rect::new(
        area.x,
        area.y + offset.min(area.height),
        area.width,
        1.min(area.height - offset.min(area.height)),
    )
}

pub(super) fn below(area: Rect, offset: u16) -> Rect {
    let offset = offset.min(area.height);
    Rect::new(area.x, area.y + offset, area.width, area.height - offset)
}

/// `left` and `right` on one line, pushed apart.
pub(super) fn spread(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: u16,
) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let gap = (width as usize).saturating_sub(used).max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

pub(super) fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Keep the end of a path, which says the most.
pub(super) fn ellipsize_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count + 1 - max));
    out
}

/// The end of what's being typed, so the cursor stays in view.
pub(super) fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max)).collect()
}

/// Greedy word wrap by characters.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word.to_string();
            while word.chars().count() > width {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let rest = word.split_off(
                    word.char_indices()
                        .nth(width)
                        .map_or(word.len(), |(i, _)| i),
                );
                lines.push(word);
                word = rest;
            }
            let needed =
                line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if needed > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines
}

/// How far along its two-second sweep the shimmer is.
pub(super) fn sweep(now: OffsetDateTime) -> f32 {
    (now.unix_timestamp_nanos().rem_euclid(2_000_000_000)) as f32 / 2_000_000_000.0
}

/// The mark beside "Thinking": the spinner, in the blue that says it's
/// working.
fn working_mark(theme: &Theme, now: OffsetDateTime) -> Span<'static> {
    Span::styled(spinner(now), theme.signal(Signal::Live))
}

pub(super) fn spinner(now: OffsetDateTime) -> &'static str {
    let frame = (now.unix_timestamp_nanos() / 120_000_000).rem_euclid(SPINNER.len() as i128);
    SPINNER[frame as usize]
}

pub(super) fn local(at: OffsetDateTime, app: &App) -> OffsetDateTime {
    at.to_offset(app.utc_offset)
}

pub(super) fn clock(at: OffsetDateTime, app: &App) -> String {
    let t = local(at, app);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

pub(super) fn clock_secs(at: OffsetDateTime, app: &App) -> String {
    let t = local(at, app);
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
}

pub(super) fn host(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string()
}

pub(super) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(super) fn files(n: u64) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{} files", thousands(n))
    }
}

pub(super) fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", thousands(n as u64))
    }
}

/// A file's size, as the composer shows it.
fn size(n: u64) -> String {
    bytes(n as usize)
}

fn bytes(n: usize) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}
