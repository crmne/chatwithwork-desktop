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
mod organizations;
mod pages;
mod running;
pub mod streaming;
pub mod theme;
pub mod ui;

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
    run_organization(paths, None)
}

pub fn run_organization(paths: Paths, selector: Option<&str>) -> Result<()> {
    let profile = selector
        .map(|selector| crate::auth::organizations::resolve(&paths, selector))
        .transpose()?;
    let chat_paths = profile
        .as_ref()
        .map(|profile| profile.runtime_paths(&paths))
        .transpose()?
        .unwrap_or_else(|| paths.clone());
    // Only safe to ask before any thread starts.
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let theme = Theme::detect();
    let history_file = history::path(&chat_paths);
    let mut app = App::new(utc_offset).with_history(history::load(&history_file));
    if let Some(profile) = &profile {
        app = app.with_organization(profile.account.id.clone());
    }
    let mouse = Config::load(&paths).map_or(true, |c| c.tui.mouse);

    let (tx, rx) = mpsc::channel();
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
    spawn_input(tx.clone());
    let chats = ChatRunner::new(Chats::new(&chat_paths.socket_path()), tx.clone());
    let presence = Presence::new(chat_paths.socket_path());
    let _organization =
        profile.map(|profile| organizations::Watcher::spawn(paths.clone(), profile, tx.clone()));
    // Commands run in the computer's own daemon, whichever organization's
    // chats are open.
    let commands = {
        let tx = tx.clone();
        Arc::new(Follow::spawn(paths.socket_path(), move |snapshot| {
            let _ = tx.send(Msg::Commands(snapshot));
        }))
    };
    let link = daemon::Link::spawn(paths, tx.clone());
    let out = Outputs {
        link,
        chats,
        history: history_file,
        presence,
        commands,
        tx: tx.clone(),
    };

    let effects = app.start();
    let mut result = Ok(());
    if out.run(effects).is_some() {
        result = event_loop(&mut terminal, &mut app, &theme, &rx, &out);
    }
    out.chats.follow(None);
    out.chats.follow_list(false);
    out.chats.recording.lock().expect("recording").take();
    out.presence.set(None);
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

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    theme: &Theme,
    rx: &mpsc::Receiver<Msg>,
    out: &Outputs,
) -> Result<()> {
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
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => return Ok(()),
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
                None => return Ok(()),
                Some(again) => redraw |= again,
            }
        }
    }
}

/// Where effects go.
struct Outputs {
    link: daemon::Link,
    chats: ChatRunner,
    history: PathBuf,
    presence: Presence,
    commands: Arc<Follow>,
    tx: Sender<Msg>,
}

impl Outputs {
    /// Run effects: `None` when it's time to quit, else whether to draw
    /// the whole screen again.
    fn run(&self, effects: Vec<Effect>) -> Option<bool> {
        let mut redraw = false;
        for effect in effects {
            match effect {
                Effect::Quit => return None,
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
                Effect::Remember(entry) => {
                    if let Err(e) = history::append(&self.history, &entry) {
                        tracing::debug!("keeping the question: {e:#}");
                    }
                }
            }
        }
        Some(redraw)
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

fn spawn_input(tx: Sender<Msg>) {
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
            if tx.send(msg).is_err() {
                return;
            }
        }
    });
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
