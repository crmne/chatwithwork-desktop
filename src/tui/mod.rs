//! `cww tui`: the terminal UI.
//!
//! Three kinds of input feed one channel: keys (a thread blocked in
//! crossterm's `event::read`), the daemon (a thread blocked on the control
//! subscription, see [`daemon`]), and chats (worker threads that ask the
//! daemon, and one that follows the open chat live). The loop blocks on that
//! channel and redraws only when something arrives. It sets a timeout only
//! while something on screen moves (a spinner, an answer being written), so
//! an idle TUI uses no CPU.

pub mod app;
pub mod chat;
pub mod commands;
pub mod composer;
mod daemon;
pub mod dictation;
mod history;
pub mod markdown;
mod newbot;
mod organizations;
mod pages;
mod running;
pub mod sidebar;
pub mod streaming;
pub mod theme;
pub mod ui;
mod views;

#[cfg(test)]
mod tests;

use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    MouseButton, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::DefaultTerminal;
use time::{OffsetDateTime, UtcOffset};

use self::app::{Acted, App, ChatAction, ChatCommand, ChatMsg, Effect, Msg};
use self::chat::Chats;
use self::theme::Theme;
use crate::commands::follow::Follow;
use crate::config::Config;
use crate::control::Closer;
use crate::notification_presence::Presence;
use crate::paths::Paths;

/// How often to check whether the owner allowed chats, after asking.
const ACCESS_POLL: Duration = Duration::from_secs(5);
/// How long to keep checking.
const ACCESS_WAIT: Duration = Duration::from_secs(15 * 60);

pub fn run(paths: Paths) -> Result<()> {
    // The organization chosen with /org last time, while its sign-in lasts.
    let remembered = remembered_organization(&paths)
        .filter(|id| crate::auth::organizations::resolve(&paths, id).is_ok());
    run_organization(paths, remembered.as_deref())
}

pub fn run_organization(paths: Paths, selector: Option<&str>) -> Result<()> {
    // A selector that names no sign-in fails before the screen changes.
    if let Some(selector) = selector {
        crate::auth::organizations::resolve(&paths, selector)?;
    }
    // Only safe to ask before any thread starts.
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let theme = Theme::detect();
    let config = Config::load(&paths).ok();
    let mouse = config.as_ref().is_none_or(|c| c.tui.mouse);

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste, EnableFocusChange);
    // Shift-Enter, told apart from Enter where the terminal can.
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
    if mouse {
        let _ = execute!(stdout(), EnableMouseCapture);
    }
    let input = Input::spawn();
    let mut selector = selector.map(str::to_string);
    let session = Session {
        paths: &paths,
        utc_offset,
        theme: &theme,
        config: config.as_ref(),
        input: &input,
    };
    let result = loop {
        match session.run(&mut terminal, selector.as_deref()) {
            // Another organization: everything of this one is dropped (its
            // chat runtime's streams, followers and watcher), and a new
            // session starts on a channel of its own, so nothing the old one
            // still sends reaches the new view.
            Ok(Ended::Switch(next)) => {
                remember_organization(&paths, next.as_deref());
                selector = next;
            }
            Ok(Ended::Quit) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    if mouse {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableBracketedPaste, DisableFocusChange);
    ratatui::restore();
    result
}

/// How a session ended.
enum Ended {
    Quit,
    /// Into this saved sign-in, or the computer's own (`None`).
    Switch(Option<String>),
}

/// What every session shares: the terminal's settings and its input.
struct Session<'a> {
    paths: &'a Paths,
    utc_offset: UtcOffset,
    theme: &'a Theme,
    config: Option<&'a Config>,
    input: &'a Input,
}

impl Session<'_> {
    /// The chats of one organization, until quitting or switching.
    fn run(&self, terminal: &mut DefaultTerminal, selector: Option<&str>) -> Result<Ended> {
        let paths = self.paths.clone();
        let profile = selector
            .map(|selector| crate::auth::organizations::resolve(&paths, selector))
            .transpose()?;
        let chat_paths = profile
            .as_ref()
            .map(|profile| profile.runtime_paths(&paths))
            .transpose()?
            .unwrap_or_else(|| paths.clone());
        let history_file = history::path(&chat_paths);
        let filter_file = sidebar::filter_path(&history_file);
        let hidden_file = sidebar::hidden_path(&history_file);
        let mut app = App::new(self.utc_offset)
            .with_history(history::load(&history_file))
            .with_filter(sidebar::load_filter(&filter_file))
            .with_organizations(
                organization_choices(&paths, self.config),
                profile.as_ref().map(|p| p.id.clone()),
            );
        if let Some(profile) = &profile {
            app = app.with_organization(profile.account.id.clone());
        }
        app = app.with_layout(
            sidebar::load_hidden(&hidden_file),
            self.config.and_then(|c| c.desktop.reduce_motion) == Some(true),
        );
        app = app.with_crash_reports(crate::crash_reports::Consent::read(self.config));
        // The terminal keeps its focus across a switch.
        app.window_focused = self.input.focused();

        let (tx, rx) = mpsc::channel();
        self.input.route(tx.clone());
        let mut chats = ChatRunner::new(Chats::new(&chat_paths.socket_path()), tx.clone());
        // Bots are read with the computer's own sign-in, as `cww bots` reads them.
        chats.bots = Some(BotsAccess {
            paths: paths.clone(),
            organization: selector.map(str::to_string),
            client: Arc::default(),
        });
        let presence = Presence::new(chat_paths.socket_path());
        let _organization = profile
            .clone()
            .map(|profile| organizations::Watcher::spawn(paths.clone(), profile, tx.clone()));
        // Commands run in the computer's own daemon, whichever organization's
        // chats are open.
        let commands = {
            let tx = tx.clone();
            Arc::new(Follow::spawn(paths.socket_path(), move |snapshot| {
                let _ = tx.send(Msg::Commands(snapshot));
            }))
        };
        let link = daemon::Link::spawn(paths.clone(), tx.clone());
        let out = Outputs {
            link,
            paths: paths.clone(),
            chats,
            history: history_file,
            filter: filter_file,
            hidden: hidden_file,
            presence,
            commands,
            tx: tx.clone(),
        };

        let effects = app.start();
        let ended = match out.run(effects) {
            Flow::Go(_) => event_loop(terminal, &mut app, self.theme, &rx, &out),
            Flow::Quit => Ok(Ended::Quit),
            Flow::Switch(next) => Ok(Ended::Switch(next)),
        };
        // The computer's own organization, by name, for /org from another.
        if profile.is_none() && !app.chat.list.account.name.is_empty() {
            let _ = crate::paths::write_private_file(
                &primary_name_path(&paths),
                app.chat.list.account.name.as_bytes(),
            );
        }
        out.chats.follow(None);
        out.chats.follow_list(false);
        out.chats.recording.lock().expect("recording").take();
        out.presence.set(None);
        ended
    }
}

/// The organizations `/org` offers: the computer's own pairing, then each
/// saved sign-in.
fn organization_choices(paths: &Paths, config: Option<&Config>) -> Vec<app::OrgChoice> {
    let mut choices = Vec::new();
    if config.is_some_and(|c| c.server.is_some()) {
        let name = std::fs::read_to_string(primary_name_path(paths))
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "This computer's organization".into());
        choices.push(app::OrgChoice { id: None, name });
    }
    for saved in crate::auth::organizations::list(paths).unwrap_or_default() {
        choices.push(app::OrgChoice {
            id: Some(saved.id.clone()),
            name: saved.account.name.clone(),
        });
    }
    choices
}

fn primary_name_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("tui-organization-name")
}

fn organization_path(paths: &Paths) -> PathBuf {
    paths.state_dir.join("tui-organization")
}

/// The saved sign-in `/org` switched to last, if any.
fn remembered_organization(paths: &Paths) -> Option<String> {
    std::fs::read_to_string(organization_path(paths))
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

fn remember_organization(paths: &Paths, id: Option<&str>) {
    let path = organization_path(paths);
    let result = match id {
        Some(id) => crate::paths::write_private_file(&path, id.as_bytes()),
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        },
    };
    if let Err(e) = result {
        tracing::debug!("remembering the organization: {e:#}");
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    theme: &Theme,
    rx: &mpsc::Receiver<Msg>,
    out: &Outputs,
) -> Result<Ended> {
    let mut redraw = false;
    loop {
        let now = OffsetDateTime::now_utc();
        if std::mem::take(&mut redraw) {
            terminal.clear()?;
        }
        let mut hits = app::Hits::default();
        terminal.draw(|frame| hits = ui::render_hits(frame, app, theme, now))?;
        app.hits = hits;
        out.presence.set(app.notification_view());
        let first = match app.next_wakeup(now) {
            Some(wait) => match rx.recv_timeout(wait) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(Ended::Quit),
            },
            None => match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => return Ok(Ended::Quit),
            },
        };
        // Apply a burst of messages before drawing again.
        for msg in first.into_iter().chain(rx.try_iter()) {
            let effects = app.update(msg);
            out.chats
                .generation
                .store(app.chat_generation, Ordering::SeqCst);
            // Blur and navigation clear immediately; a new visible view waits
            // until the next draw has actually put its transcript on screen.
            if app.notification_view().is_none() {
                out.presence.set(None);
            }
            match out.run(effects) {
                Flow::Quit => return Ok(Ended::Quit),
                Flow::Switch(next) => return Ok(Ended::Switch(next)),
                Flow::Go(again) => redraw |= again,
            }
        }
    }
}

/// Where effects go.
struct Outputs {
    link: daemon::Link,
    chats: ChatRunner,
    history: PathBuf,
    filter: PathBuf,
    hidden: PathBuf,
    presence: Presence,
    commands: Arc<Follow>,
    paths: Paths,
    tx: Sender<Msg>,
}

/// What running effects says next.
enum Flow {
    /// Carry on, drawing the whole screen again when true.
    Go(bool),
    Quit,
    Switch(Option<String>),
}

impl Outputs {
    fn run(&self, effects: Vec<Effect>) -> Flow {
        let mut redraw = false;
        for effect in effects {
            match effect {
                Effect::Quit => return Flow::Quit,
                Effect::SwitchOrganization(next) => return Flow::Switch(next),
                Effect::Daemon(command) => self.link.send(command),
                Effect::Chat(command) => self.chats.run(command),
                Effect::Copy(text) => copy(&text),
                Effect::Redraw => redraw = true,
                Effect::FollowCommand(run) => self.commands.focus(run),
                Effect::StopCommand(run) => {
                    let (commands, tx) = (self.commands.clone(), self.tx.clone());
                    thread::spawn(move || {
                        let _ = tx.send(Msg::CommandStopped(commands.stop_run(&run)));
                    });
                }
                Effect::CrashReports(on) => {
                    if let Err(e) = crate::crash_reports::set(&self.paths, on) {
                        let _ = self.tx.send(Msg::CrashReportsNotSaved(format!("{e:#}")));
                    }
                }
                Effect::RememberSidebar(hidden) => {
                    if let Err(e) = sidebar::save_hidden(&self.hidden, hidden) {
                        tracing::debug!("keeping the sidebar hidden or shown: {e:#}");
                    }
                }
                Effect::RememberFilter(filter) => {
                    if let Err(e) = sidebar::save_filter(&self.filter, filter) {
                        tracing::debug!("keeping the sidebar's filter: {e:#}");
                    }
                }
                Effect::Remember(entry) => {
                    if let Err(e) = history::append(&self.history, &entry) {
                        tracing::debug!("keeping the question: {e:#}");
                    }
                }
            }
        }
        Flow::Go(redraw)
    }
}

/// Put text on the clipboard with OSC 52, which terminals pass on to the
/// system clipboard (over SSH too); those that don't ignore it.
fn copy(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

/// Keys, the mouse and pastes, read by one thread for the whole run and
/// sent to the session open now, and whether the terminal has focus, which
/// a new session starts from.
struct Input(Arc<Mutex<Option<Sender<Msg>>>>, Arc<AtomicBool>);

impl Input {
    fn spawn() -> Self {
        let route: Arc<Mutex<Option<Sender<Msg>>>> = Arc::default();
        let to = Arc::clone(&route);
        let focused = Arc::new(AtomicBool::new(false));
        let focus = Arc::clone(&focused);
        thread::spawn(move || {
            loop {
                let msg = match event::read() {
                    // Windows also reports releases.
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => Msg::Key(key),
                    // Clicks and the wheel; moves and drags would only wake it.
                    Ok(Event::Mouse(mouse))
                        if matches!(
                            mouse.kind,
                            MouseEventKind::Down(MouseButton::Left)
                                | MouseEventKind::ScrollUp
                                | MouseEventKind::ScrollDown
                        ) =>
                    {
                        Msg::Mouse(mouse)
                    }
                    Ok(Event::Paste(text)) => Msg::Paste(text),
                    Ok(Event::Resize(..)) => Msg::Resize,
                    Ok(Event::FocusGained) => Msg::WindowFocus(true),
                    Ok(Event::FocusLost) => Msg::WindowFocus(false),
                    Ok(_) => continue,
                    Err(_) => return,
                };
                if let Msg::WindowFocus(on) = msg {
                    focus.store(on, Ordering::SeqCst);
                }
                if let Some(tx) = to.lock().expect("input route").as_ref() {
                    let _ = tx.send(msg);
                }
            }
        });
        Self(route, focused)
    }

    fn focused(&self) -> bool {
        self.1.load(Ordering::SeqCst)
    }

    /// From now on, to this session.
    fn route(&self, tx: Sender<Msg>) {
        *self.0.lock().expect("input route") = Some(tx);
    }
}

/// The chat pane's calls to the daemon. Each blocks, so each gets a thread.
struct ChatRunner {
    generation: AtomicU64,
    recording: Mutex<Option<dictation::Recording>>,
    chats: Chats,
    tx: Sender<Msg>,
    /// The chat followed now: set its flag to stop it, and close its
    /// connection to stop it at once.
    following: Mutex<Option<Follower>>,
    /// Keeping the chat list current, while it is.
    list: Mutex<Option<Follower>>,
    /// Bumped to stop an earlier wait for chat access.
    access_wait: Arc<AtomicU64>,
    /// Where to sign in for bots, which the daemon doesn't relay.
    bots: Option<BotsAccess>,
}

/// The sign-in `cww bots` uses: this computer's own, or a saved
/// organization's. One client for the run, so its token is refreshed one
/// call at a time, never by two at once with the same refresh credential.
#[derive(Clone)]
struct BotsAccess {
    paths: Paths,
    organization: Option<String>,
    client: Arc<Mutex<Option<Arc<crate::bots::Bots>>>>,
}

impl BotsAccess {
    fn connect(&self) -> Result<Arc<crate::bots::Bots>, chat::Failure> {
        let mut client = self.client.lock().expect("bots client");
        if let Some(bots) = client.as_ref() {
            return Ok(Arc::clone(bots));
        }
        let bots = Arc::new(
            crate::bots::Bots::connect(&self.paths, self.organization.as_deref())
                .map_err(bots_failure)?,
        );
        *client = Some(Arc::clone(&bots));
        Ok(bots)
    }
}

/// How often a try is read again while it runs, and for how long.
const TRIAL_POLL: Duration = Duration::from_secs(2);
const TRIAL_WAIT: Duration = Duration::from_secs(5 * 60);

fn no_bots() -> chat::Failure {
    chat::Failure::new("unsupported", "Bots aren't available here.")
}

/// One call to the bots API, with the computer's own sign-in.
fn with_bots(
    bots: Option<BotsAccess>,
    call: impl FnOnce(&crate::bots::Bots) -> anyhow::Result<serde_json::Value>,
) -> Result<serde_json::Value, chat::Failure> {
    let bots = bots.ok_or_else(no_bots)?.connect()?;
    call(&bots).map_err(bots_failure)
}

/// A refusal from the bots API, as the chat pane shows failures.
fn bots_failure(error: anyhow::Error) -> chat::Failure {
    let json = crate::bots::error_json(&error);
    let json = &json["error"];
    chat::Failure::new(
        json["code"].as_str().unwrap_or("failed"),
        json["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{error:#}")),
    )
}

struct Follower {
    stop: Arc<AtomicBool>,
    closer: Arc<Mutex<Option<Closer>>>,
}

impl Follower {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(closer) = self.closer.lock().expect("closer").take() {
            closer.close();
        }
    }
}

impl ChatRunner {
    fn new(chats: Chats, tx: Sender<Msg>) -> Self {
        Self {
            generation: AtomicU64::new(0),
            recording: Mutex::new(None),
            chats,
            tx,
            following: Mutex::new(None),
            list: Mutex::new(None),
            access_wait: Arc::new(AtomicU64::new(0)),
            bots: None,
        }
    }

    fn run(&self, command: ChatCommand) {
        let (chats, tx) = (self.chats.clone(), self.tx.clone());
        let scope_generation = self.generation.load(Ordering::SeqCst);
        let send = move |message: ChatMsg| {
            let _ = tx.send(Msg::ScopedChat {
                generation: scope_generation,
                message,
            });
        };
        match command {
            ChatCommand::Dictate(generation) => {
                let tx = self.tx.clone();
                let result = dictation::installed_helper().and_then(|helper| {
                    dictation::Recording::start(chats, helper, move |update| {
                        let _ = tx.send(Msg::ScopedChat {
                            generation: scope_generation,
                            message: ChatMsg::Dictation(generation, update),
                        });
                    })
                });
                match result {
                    Ok(recording) => *self.recording.lock().expect("recording") = Some(recording),
                    Err(failure) => send(ChatMsg::Dictation(
                        generation,
                        dictation::Update::Finished(Err(failure)),
                    )),
                }
            }
            ChatCommand::StopDictation => {
                if let Some(recording) = self.recording.lock().expect("recording").as_ref() {
                    recording.stop();
                }
            }
            ChatCommand::CancelDictation => {
                self.recording.lock().expect("recording").take();
            }
            ChatCommand::List => {
                thread::spawn(move || send(ChatMsg::Listed(chats.list())));
            }
            ChatCommand::Open(chat) => {
                thread::spawn(move || {
                    send(ChatMsg::Shown {
                        chat,
                        result: chats.show(chat),
                    });
                });
            }
            ChatCommand::Send {
                chat,
                text,
                model,
                web_search,
                project,
                attachments,
            } => {
                thread::spawn(move || {
                    let ask = chat::Ask {
                        chat,
                        text,
                        project,
                        model,
                        web_search,
                        attachments,
                        steer: None,
                    };
                    send(ChatMsg::Sent {
                        chat,
                        result: chats.ask(&ask),
                    });
                });
            }
            ChatCommand::Models => {
                thread::spawn(move || send(ChatMsg::Models(chats.models())));
            }
            ChatCommand::Approve {
                chat,
                tool_call,
                for_rest_of_chat,
            } => {
                thread::spawn(move || {
                    let result = chats.approve(chat, tool_call, for_rest_of_chat);
                    send(ChatMsg::Decided { chat, result });
                });
            }
            ChatCommand::Deny {
                chat,
                tool_call,
                reason,
            } => {
                thread::spawn(move || {
                    let result = chats.deny(chat, tool_call, reason.as_deref());
                    send(ChatMsg::Decided { chat, result });
                });
            }
            ChatCommand::Answer {
                chat,
                tool_call,
                input,
            } => {
                thread::spawn(move || {
                    let result = chats.answer(chat, tool_call, input);
                    send(ChatMsg::Decided { chat, result });
                });
            }
            ChatCommand::Decline { chat, tool_call } => {
                thread::spawn(move || {
                    let result = chats.decline(chat, tool_call);
                    send(ChatMsg::Decided { chat, result });
                });
            }
            ChatCommand::Download { path, destination } => {
                thread::spawn(move || {
                    send(ChatMsg::Downloaded(chats.save_download(
                        &path,
                        &expand_home(&destination),
                        false,
                    )))
                });
            }
            ChatCommand::Upload(path) => {
                thread::spawn(move || {
                    let result = chats.upload(&expand_home(&path));
                    send(ChatMsg::Uploaded { path, result });
                });
            }
            ChatCommand::Act { chat, action } => {
                thread::spawn(move || {
                    let result = match &action {
                        ChatAction::Pin(pinned) => chats.pin(chat, *pinned).map(Acted::Pins),
                        ChatAction::Retry => chats.retry(chat, None).map(Acted::Chat),
                        ChatAction::Branch => chats.branch(chat, None).map(Acted::Chat),
                        ChatAction::Rename(title) => chats.rename(chat, title).map(Acted::Chat),
                        ChatAction::Delete => chats.delete(chat).map(|()| Acted::Deleted),
                        ChatAction::Share => chats.share(chat).map(Acted::Shared),
                        ChatAction::Unshare => chats.unshare(chat).map(Acted::Chat),
                    };
                    send(ChatMsg::Acted {
                        chat,
                        action,
                        result,
                    });
                });
            }
            ChatCommand::Cancel(chat) => {
                thread::spawn(move || send(ChatMsg::Cancelled(chats.cancel(chat))));
            }
            ChatCommand::Queue {
                chat,
                text,
                steer,
                attachments,
            } => {
                thread::spawn(move || {
                    let ask = chat::Ask {
                        chat: Some(chat),
                        text: text.clone(),
                        attachments,
                        steer: Some(steer),
                        ..chat::Ask::default()
                    };
                    let result = chats.ask(&ask);
                    send(ChatMsg::Queued { chat, text, result });
                });
            }
            ChatCommand::Unqueue { chat, queued } => {
                thread::spawn(move || {
                    let result = chats.unqueue(chat, queued.id);
                    send(ChatMsg::Unqueued {
                        chat,
                        queued,
                        result,
                    });
                });
            }
            ChatCommand::RequestAccess => {
                let generation = self.access_wait.fetch_add(1, Ordering::SeqCst) + 1;
                let current = Arc::clone(&self.access_wait);
                thread::spawn(move || {
                    let result = chats.request_access();
                    let waiting = result.as_ref().is_ok_and(|r| !r.granted);
                    send(ChatMsg::Access(result));
                    if waiting {
                        wait_for_access(&chats, &send, || {
                            current.load(Ordering::SeqCst) == generation
                        });
                    }
                });
            }
            ChatCommand::WaitForAccess => {
                let generation = self.access_wait.fetch_add(1, Ordering::SeqCst) + 1;
                let current = Arc::clone(&self.access_wait);
                thread::spawn(move || {
                    wait_for_access(&chats, &send, || {
                        current.load(Ordering::SeqCst) == generation
                    });
                });
            }
            ChatCommand::Follow(chat) => self.follow(chat),
            ChatCommand::FollowList(on) => self.follow_list(on),
            ChatCommand::OpenUrl(url) => {
                crate::browser::open(&url);
            }
            ChatCommand::Decisions => {
                thread::spawn(move || send(ChatMsg::Decisions(chats.decisions())));
            }
            ChatCommand::Bot { id, filter } => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let result = match bots {
                        Some(bots) => bots.connect().and_then(|bots| {
                            bots.show_runs(&id.to_string(), filter.as_deref())
                                .map_err(bots_failure)
                        }),
                        None => Err(chat::Failure::new(
                            "unsupported",
                            "Bots aren't available here.",
                        )),
                    };
                    send(ChatMsg::Bot { id, filter, result });
                });
            }
            ChatCommand::Recipes => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let result = with_bots(bots, |bots| bots.recipes()).and_then(|answer| {
                        serde_json::from_value(answer["recipes"].clone())
                            .map_err(|e| chat::Failure::new("unsupported", e.to_string()))
                    });
                    send(ChatMsg::Recipes(result));
                });
            }
            ChatCommand::Recipe { key, tools } => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let result = with_bots(bots, |bots| bots.recipe(&key, &tools));
                    send(ChatMsg::Recipe { key, result });
                });
            }
            ChatCommand::StartBotBuilder { sentence, project } => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let mut body = serde_json::json!({ "sentence": sentence });
                    if let Some(project) = project {
                        body["project_id"] = serde_json::json!(project);
                    }
                    let result =
                        with_bots(bots, |bots| bots.start_builder(&body)).and_then(|answer| {
                            serde_json::from_value(answer["chat"].clone())
                                .map_err(|e| chat::Failure::new("unsupported", e.to_string()))
                        });
                    send(ChatMsg::BotBuilderStarted(result));
                });
            }
            ChatCommand::TryBot { body, origin } => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let bots = match bots.ok_or_else(no_bots).and_then(|b| b.connect()) {
                        Ok(bots) => bots,
                        Err(failure) => {
                            return send(ChatMsg::Trial {
                                origin,
                                result: Err(failure),
                            });
                        }
                    };
                    let started = bots.start_trial(&body).map_err(bots_failure);
                    let id = started
                        .as_ref()
                        .ok()
                        .and_then(|a| a["trial"]["id"].as_u64());
                    let mut running = started
                        .as_ref()
                        .is_ok_and(|a| a["trial"]["state"] == "running");
                    send(ChatMsg::Trial {
                        origin,
                        result: started,
                    });
                    let begun = Instant::now();
                    while running && let Some(id) = id {
                        if begun.elapsed() > TRIAL_WAIT {
                            // Never leave Try it waiting: say how to see it.
                            return send(ChatMsg::Trial {
                                origin,
                                result: Err(chat::Failure::new(
                                    "timeout",
                                    format!(
                                        "The try is still running. Follow it with cww bots run --follow {id}."
                                    ),
                                )),
                            });
                        }
                        thread::sleep(TRIAL_POLL);
                        let answer = bots.trial(id).map_err(bots_failure);
                        running = answer
                            .as_ref()
                            .is_ok_and(|a| a["trial"]["state"] == "running");
                        send(ChatMsg::Trial {
                            origin,
                            result: answer,
                        });
                    }
                });
            }
            ChatCommand::MakeBot { body, origin } => {
                let bots = self.bots.clone();
                thread::spawn(move || {
                    let result = with_bots(bots, |bots| bots.create(&body));
                    send(ChatMsg::BotMade { origin, result });
                });
            }
            ChatCommand::Connect { key, server } => {
                thread::spawn(move || {
                    let result = chats
                        .connector_handoff(key, "read".into(), None)
                        .map(|path| {
                            let url = format!("{}{path}", server.trim_end_matches('/'));
                            crate::browser::open(&url);
                        });
                    send(ChatMsg::Connected(result));
                });
            }
            ChatCommand::ProjectChats {
                project,
                number,
                before,
            } => {
                thread::spawn(move || {
                    let result = chats
                        .projects(crate::projects::Request::Show {
                            project: number,
                            before,
                        })
                        .and_then(|reply| match reply {
                            crate::projects::Reply::Page(page) => Ok((
                                page.chats.into_iter().map(|c| c.chat).collect(),
                                page.next_cursor,
                            )),
                            _ => Err(chat::Failure::new("unsupported", "No chats came back.")),
                        });
                    send(ChatMsg::ProjectChats { project, result });
                });
            }
        }
    }

    /// Keep the chat list current, starting over if it already is, or stop.
    fn follow_list(&self, on: bool) {
        let generation = self.generation.load(Ordering::SeqCst);
        let mut list = self.list.lock().expect("list");
        if let Some(previous) = list.take() {
            previous.stop();
        }
        if !on {
            return;
        }
        let follower = Follower {
            stop: Arc::new(AtomicBool::new(false)),
            closer: Arc::new(Mutex::new(None)),
        };
        let (stop, closer) = (Arc::clone(&follower.stop), Arc::clone(&follower.closer));
        *list = Some(follower);
        let (chats, tx) = (self.chats.clone(), self.tx.clone());
        thread::spawn(move || {
            let result = chats.follow_list(
                |c| {
                    *closer.lock().expect("closer") = c;
                    if stop.load(Ordering::SeqCst)
                        && let Some(c) = closer.lock().expect("closer").take()
                    {
                        c.close();
                    }
                },
                |live| {
                    if stop.load(Ordering::SeqCst) {
                        return false;
                    }
                    match live {
                        Some(live) => tx
                            .send(Msg::ScopedChat {
                                generation,
                                message: ChatMsg::ListLive(live),
                            })
                            .is_ok(),
                        None => true,
                    }
                },
            );
            if let Err(failure) = result
                && !stop.load(Ordering::SeqCst)
            {
                let _ = tx.send(Msg::ScopedChat {
                    generation,
                    message: ChatMsg::ListFollowFailed(failure),
                });
            }
        });
    }

    /// Follow `chat` live, instead of whatever was followed before.
    fn follow(&self, chat: Option<u64>) {
        let generation = self.generation.load(Ordering::SeqCst);
        let mut following = self.following.lock().expect("following");
        if let Some(previous) = following.take() {
            previous.stop();
        }
        let Some(chat) = chat else { return };
        let follower = Follower {
            stop: Arc::new(AtomicBool::new(false)),
            closer: Arc::new(Mutex::new(None)),
        };
        let (stop, closer) = (Arc::clone(&follower.stop), Arc::clone(&follower.closer));
        *following = Some(follower);
        let (chats, tx) = (self.chats.clone(), self.tx.clone());
        thread::spawn(move || {
            let result = chats.follow(
                chat,
                |c| {
                    *closer.lock().expect("closer") = c;
                    // Stopped while connecting.
                    if stop.load(Ordering::SeqCst)
                        && let Some(c) = closer.lock().expect("closer").take()
                    {
                        c.close();
                    }
                },
                |live| {
                    if stop.load(Ordering::SeqCst) {
                        return false;
                    }
                    match live {
                        Some(live) => tx
                            .send(Msg::ScopedChat {
                                generation,
                                message: ChatMsg::Live { chat, live },
                            })
                            .is_ok(),
                        None => true,
                    }
                },
            );
            if let Err(failure) = result
                && !stop.load(Ordering::SeqCst)
            {
                let _ = tx.send(Msg::ScopedChat {
                    generation,
                    message: ChatMsg::FollowFailed { chat, failure },
                });
            }
        });
    }
}

/// `~/notes.pdf` as the shell would read it; a relative path is from where
/// `cww` was started.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").or(path.strip_prefix("~\\")) {
        Some(rest) => crate::paths::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|_| PathBuf::from(path)),
        None if path == "~" => crate::paths::home_dir().unwrap_or_else(|_| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// Check back until the owner answers the request for chats, then tell the
/// pane. Stops early when `current` says a newer wait took over.
fn wait_for_access(chats: &Chats, send: &impl Fn(ChatMsg), current: impl Fn() -> bool) {
    let started = Instant::now();
    while started.elapsed() < ACCESS_WAIT && current() {
        thread::sleep(ACCESS_POLL);
        if !current() {
            return;
        }
        match chats.list() {
            Err(failure) if failure.code == "chat_access_required" => {}
            result => return send(ChatMsg::Listed(result)),
        }
    }
}
