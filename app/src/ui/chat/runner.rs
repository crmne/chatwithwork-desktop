//! Runs the chat page's commands against the daemon, each on its own
//! thread, as the terminal UI does, and wakes the window when an answer
//! comes back. Following a chat holds one connection open, blocked on the
//! daemon's socket; heartbeats on it don't wake the window.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cww::control::Closer;
use cww::tui::chat::Chats;
use cww::tui::dictation::{Recording, Update};

use super::state::{Acted, ChatAction, Command, Decision, Msg};

/// How often to check whether the owner allowed chats, and for how long.
const ACCESS_POLL: Duration = Duration::from_secs(5);
const ACCESS_WAIT: Duration = Duration::from_secs(15 * 60);

pub struct Runner {
    recording: Mutex<Option<Recording>>,
    chats: Chats,
    tx: Sender<ScopedMsg>,
    rx: Receiver<ScopedMsg>,
    generation: AtomicU64,
    ctx: Arc<Mutex<Option<egui::Context>>>,
    following: Mutex<Option<Follower>>,
    /// Keeps the list current.
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

/// Sends a message and wakes the window to read it.
#[derive(Clone)]
struct Wake {
    tx: Sender<ScopedMsg>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
    generation: u64,
}

struct ScopedMsg {
    generation: u64,
    msg: Msg,
}

impl Wake {
    fn send(&self, msg: Msg) -> bool {
        let sent = self
            .tx
            .send(ScopedMsg {
                generation: self.generation,
                msg,
            })
            .is_ok();
        if let Some(ctx) = &*self.ctx.lock().expect("ctx") {
            ctx.request_repaint();
        }
        sent
    }
}

impl Runner {
    pub fn new(socket: &Path) -> Self {
        let (tx, rx) = channel();
        Self {
            recording: Mutex::new(None),
            chats: Chats::new(socket),
            tx,
            rx,
            generation: AtomicU64::new(0),
            ctx: Arc::new(Mutex::new(None)),
            following: Mutex::new(None),
            list: Mutex::new(None),
            access_wait: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The window to wake when answers arrive.
    pub fn attach(&self, ctx: &egui::Context) {
        let mut slot = self.ctx.lock().expect("ctx");
        if slot.is_none() {
            *slot = Some(ctx.clone());
        }
    }

    /// Validate each result as it is consumed. A changed account can invalidate
    /// later results that were already queued in the very same frame.
    pub fn next(&self) -> Option<Msg> {
        while let Ok(message) = self.rx.try_recv() {
            if message.generation == self.generation.load(Ordering::SeqCst) {
                return Some(message.msg);
            }
        }
        None
    }

    fn wake(&self) -> Wake {
        Wake {
            tx: self.tx.clone(),
            ctx: Arc::clone(&self.ctx),
            generation: self.generation.load(Ordering::SeqCst),
        }
    }

    pub fn run(&self, command: Command) {
        let (chats, wake) = (self.chats.clone(), self.wake());
        let spawn = |name: &str, work: Box<dyn FnOnce() + Send>| {
            if let Err(e) = thread::Builder::new().name(name.into()).spawn(work) {
                log::error!("can't start a chat thread: {e}");
            }
        };
        match command {
            Command::ResetAccount => self.invalidate(),
            Command::Dictate(generation) => {
                let result = std::env::current_exe()
                    .map_err(|_| {
                        cww::tui::chat::Failure::new(
                            "unavailable",
                            "Could not find the microphone helper.",
                        )
                    })
                    .and_then(|helper| {
                        let wake = wake.clone();
                        Recording::start(chats, helper, move |update| {
                            wake.send(Msg::Dictation(generation, update));
                        })
                    });
                match result {
                    Ok(recording) => *self.recording.lock().expect("recording") = Some(recording),
                    Err(failure) => {
                        wake.send(Msg::Dictation(generation, Update::Finished(Err(failure))));
                    }
                }
            }
            Command::StopDictation => {
                if let Some(recording) = self.recording.lock().expect("recording").as_ref() {
                    recording.stop();
                }
            }
            Command::CancelDictation => {
                self.recording.lock().expect("recording").take();
            }
            Command::List => spawn(
                "cww-chats",
                Box::new(move || {
                    wake.send(Msg::Listed(chats.list()));
                }),
            ),
            Command::History(generation, query, before) => spawn(
                "cww-chat-history",
                Box::new(move || {
                    wake.send(Msg::History(generation, chats.history(query, before)));
                }),
            ),
            Command::Projects(generation, request) => spawn(
                "cww-projects",
                Box::new(move || {
                    wake.send(Msg::Projects(generation, chats.projects(request)));
                }),
            ),
            Command::Workspace(generation, project) => spawn(
                "cww-workspace",
                Box::new(move || {
                    wake.send(Msg::Workspace(generation, chats.workspace(project)));
                }),
            ),
            Command::CountInstruction(generation, layer, body) => spawn(
                "cww-instruction-count",
                Box::new(move || {
                    let result = chats.instruction_count(body.clone());
                    wake.send(Msg::InstructionCounted(generation, layer, body, result));
                }),
            ),
            Command::ComputerRemove(generation, id, project) => spawn(
                "cww-computer-remove",
                Box::new(move || {
                    let result = chats.computer_remove(id).and_then(|personal| {
                        project.map_or(Ok(personal), |id| chats.workspace(Some(id)))
                    });
                    wake.send(Msg::Workspace(generation, result));
                }),
            ),
            Command::Connectors {
                generation,
                query,
                category,
            } => spawn(
                "cww-connectors",
                Box::new(move || {
                    wake.send(Msg::Connectors {
                        generation,
                        result: chats.connectors(query, category),
                    });
                }),
            ),
            Command::ConnectorHandoff {
                generation,
                key,
                access,
                connection,
            } => spawn(
                "cww-connector-handoff",
                Box::new(move || {
                    wake.send(Msg::ConnectorHandoff {
                        generation,
                        result: chats.connector_handoff(key, access, connection),
                    });
                }),
            ),
            Command::ServicePause(generation, service, paused, project) => spawn(
                "cww-service-pause",
                Box::new(move || {
                    wake.send(Msg::Workspace(
                        generation,
                        chats.service_pause(&service, paused).and_then(|personal| {
                            project.map_or(Ok(personal), |id| chats.workspace(Some(id)))
                        }),
                    ));
                }),
            ),
            Command::MemoryPreference(generation, enabled) => spawn(
                "cww-memory-preference",
                Box::new(move || {
                    wake.send(Msg::Memory(generation, chats.memory_preference(enabled)));
                }),
            ),
            Command::MemoryForget(generation, id) => spawn(
                "cww-memory-forget",
                Box::new(move || {
                    wake.send(Msg::Memory(generation, chats.memory_forget(id)));
                }),
            ),
            Command::Instructions(generation, project) => spawn(
                "cww-instructions",
                Box::new(move || {
                    wake.send(Msg::Instructions(generation, chats.instructions(project)));
                }),
            ),
            Command::SaveInstruction(generation, layer, change, project) => spawn(
                "cww-instruction-save",
                Box::new(move || {
                    wake.send(Msg::InstructionSaved(
                        generation,
                        chats.instruction_update(project, layer, change),
                    ));
                }),
            ),
            Command::Open(chat) => spawn(
                "cww-chat",
                Box::new(move || {
                    wake.send(Msg::Shown {
                        chat,
                        result: chats.show(chat),
                    });
                }),
            ),
            Command::Send(ask) => spawn(
                "cww-chat-send",
                Box::new(move || {
                    wake.send(Msg::Sent {
                        chat: ask.chat,
                        result: chats.ask(&ask),
                    });
                }),
            ),
            Command::Schedules(generation, request) => spawn(
                "cww-schedules",
                Box::new(move || {
                    wake.send(Msg::Schedules(generation, chats.schedules(request)));
                }),
            ),
            Command::AccountSettings(generation, request) => spawn(
                "cww-account-settings",
                Box::new(move || {
                    wake.send(Msg::AccountSettings(
                        generation,
                        chats.account_settings(request),
                    ));
                }),
            ),
            Command::ModelPreferences(generation, update) => spawn(
                "cww-model-preferences",
                Box::new(move || {
                    let result = match update {
                        Some((account, model)) => chats.update_model_preferences(account, model),
                        None => chats.model_preferences(),
                    };
                    wake.send(Msg::ModelPreferences(generation, result));
                }),
            ),
            Command::Templates(generation, request) => spawn(
                "cww-templates",
                Box::new(move || {
                    wake.send(Msg::Templates(generation, chats.templates(request)));
                }),
            ),
            Command::UploadTemplate(generation, target, path) => spawn(
                "cww-template-upload",
                Box::new(move || {
                    wake.send(Msg::Templates(
                        generation,
                        chats.upload_template(target, &path),
                    ));
                }),
            ),
            Command::PickTemplate(generation, extensions) => {
                #[cfg(test)]
                {
                    let _ = extensions;
                    let path = super::tests::PICKED
                        .with(|p| p.borrow_mut().take())
                        .and_then(|p| p.into_iter().next());
                    wake.send(Msg::TemplatePicked(generation, Ok(path)));
                }
                #[cfg(not(test))]
                {
                    // AppKit requires construction on the UI thread. Only the result is
                    // awaited elsewhere; it must return through account/generation guards
                    // before any upload starts.
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title("Choose a template")
                        .add_filter("Office template", &extensions)
                        .pick_file();
                    let failed = wake.clone();
                    if let Err(error) = thread::Builder::new()
                        .name("cww-template-picker".into())
                        .spawn(move || {
                            wake.send(Msg::TemplatePicked(
                                generation,
                                Ok(pollster::block_on(dialog).map(|f| f.path().to_path_buf())),
                            ));
                        })
                    {
                        failed.send(Msg::TemplatePicked(
                            generation,
                            Err(cww::tui::chat::Failure::new(
                                "unavailable",
                                format!("Could not open the file picker: {error}"),
                            )),
                        ));
                    }
                }
            }
            Command::Models => spawn(
                "cww-chat-models",
                Box::new(move || {
                    wake.send(Msg::Models(chats.models()));
                }),
            ),
            Command::Upload { key, path } => spawn(
                "cww-chat-upload",
                Box::new(move || {
                    wake.send(Msg::Uploaded {
                        key,
                        result: chats.upload(&path),
                    });
                }),
            ),
            Command::Decide { chat, decision } => spawn(
                "cww-chat-decide",
                Box::new(move || {
                    let result = match decision {
                        Decision::Approve {
                            tool_call,
                            for_rest_of_chat,
                        } => chats.approve(chat, tool_call, for_rest_of_chat),
                        Decision::Deny { tool_call, reason } => {
                            chats.deny(chat, tool_call, reason.as_deref())
                        }
                        Decision::Answer { tool_call, input } => {
                            chats.answer(chat, tool_call, input)
                        }
                        Decision::Decline { tool_call } => chats.decline(chat, tool_call),
                    };
                    wake.send(Msg::Decided { chat, result });
                }),
            ),
            Command::Act { chat, action } => spawn(
                "cww-chat-act",
                Box::new(move || {
                    let result = match &action {
                        ChatAction::Retry(message) => {
                            chats.retry(chat, Some(*message)).map(Acted::Chat)
                        }
                        ChatAction::Branch(message) => {
                            chats.branch(chat, Some(*message)).map(Acted::Chat)
                        }
                        ChatAction::Rename(title) => chats.rename(chat, title).map(Acted::Chat),
                        ChatAction::Delete => chats.delete(chat).map(|()| Acted::Deleted),
                        ChatAction::Pin(pinned) => chats.pin(chat, *pinned).map(Acted::Pins),
                        ChatAction::Move(project) => chats.move_to(chat, *project).map(Acted::Chat),
                        ChatAction::Share => chats.share(chat).map(Acted::Shared),
                        ChatAction::Unshare => chats.unshare(chat).map(Acted::Chat),
                    };
                    wake.send(Msg::Acted {
                        chat,
                        action,
                        result,
                    });
                }),
            ),
            Command::Cancel(chat) => spawn(
                "cww-chat-cancel",
                Box::new(move || {
                    wake.send(Msg::Cancelled(chats.cancel(chat)));
                }),
            ),
            Command::RequestAccess | Command::WaitForAccess => {
                let ask = command == Command::RequestAccess;
                let generation = self.access_wait.fetch_add(1, Ordering::SeqCst) + 1;
                let current = Arc::clone(&self.access_wait);
                spawn(
                    "cww-chat-access",
                    Box::new(move || {
                        if ask {
                            let result = chats.request_access();
                            let waiting = result.as_ref().is_ok_and(|r| !r.granted);
                            wake.send(Msg::Access(result));
                            if !waiting {
                                return;
                            }
                        }
                        wait_for_access(&chats, &wake, || {
                            current.load(Ordering::SeqCst) == generation
                        });
                    }),
                );
            }
            Command::Follow(chat) => self.follow(chat),
            Command::FollowList(on) => self.follow_list(on),
        }
    }

    /// Keep the list current, starting over if it already is, or stop.
    /// The thread blocks on the daemon's socket; heartbeats don't wake the
    /// window.
    fn follow_list(&self, on: bool) {
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
        let (chats, wake) = (self.chats.clone(), self.wake());
        let spawned = thread::Builder::new()
            .name("cww-chats-follow".into())
            .spawn(move || {
                let result = chats.follow_list(
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
                            Some(live) => wake.send(Msg::ListLive(live)),
                            // A heartbeat: nothing to draw.
                            None => true,
                        }
                    },
                );
                if !stop.load(Ordering::SeqCst) {
                    wake.send(Msg::ListFollowEnded(result.err()));
                }
            });
        if let Err(e) = spawned {
            log::error!("can't follow the chat list: {e}");
        }
    }

    /// Follow `chat` live, instead of whatever was followed before.
    fn follow(&self, chat: Option<u64>) {
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
        let (chats, wake) = (self.chats.clone(), self.wake());
        let spawned = thread::Builder::new()
            .name("cww-chat-follow".into())
            .spawn(move || {
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
                            Some(live) => wake.send(Msg::Live { chat, live }),
                            // A heartbeat: nothing to draw.
                            None => true,
                        }
                    },
                );
                if let Err(failure) = result
                    && !stop.load(Ordering::SeqCst)
                {
                    wake.send(Msg::FollowFailed { chat, failure });
                }
            });
        if let Err(e) = spawned {
            log::error!("can't follow chat {chat}: {e}");
        }
    }

    /// Stop every old account's observer and reject its buffered/in-flight work.
    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.recording.lock().expect("recording").take();
        if let Some(follower) = self.following.lock().expect("following").take() {
            follower.stop();
        }
        if let Some(follower) = self.list.lock().expect("list").take() {
            follower.stop();
        }
        self.access_wait.fetch_add(1, Ordering::SeqCst);
    }

    /// Stop following, as the window closes.
    pub fn stop(&self) {
        self.invalidate();
        *self.ctx.lock().expect("ctx") = None;
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Check back until the owner answers the request for chats. Stops early
/// when `current` says a newer wait took over.
fn wait_for_access(chats: &Chats, wake: &Wake, current: impl Fn() -> bool) {
    let started = Instant::now();
    while started.elapsed() < ACCESS_WAIT && current() {
        thread::sleep(ACCESS_POLL);
        if !current() {
            return;
        }
        match chats.list() {
            Err(failure) if failure.code == "chat_access_required" => {}
            result => {
                wake.send(Msg::Listed(result));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::ChatState;
    use super::*;
    use cww::tui::chat::{AccountUpdate, ChatList, ChatSummary, ListLive, Named, Transcript};

    #[test]
    fn account_reset_rejects_buffered_and_late_same_number_results() {
        let runner = Runner::new(Path::new("unused-test-socket"));
        let old = runner.wake();
        let mut state = ChatState::default();
        state.set_paired(true);
        state.update(Msg::Listed(Ok(ChatList {
            account: Named {
                id: "482139075".into(),
                ..Named::default()
            },
            ..ChatList::default()
        })));
        state.open_chat(16);
        let shown = |title: &str| Msg::Shown {
            chat: 16,
            result: Ok(Transcript {
                chat: ChatSummary {
                    number: 16,
                    title: title.into(),
                    ..ChatSummary::default()
                },
                ..Transcript::default()
            }),
        };
        old.send(Msg::ListLive(ListLive::Account(AccountUpdate {
            account: Named {
                id: "987654321".into(),
                ..Named::default()
            },
            ..AccountUpdate::default()
        })));
        old.send(shown("Old account's buffered transcript"));
        let commands = state.update(runner.next().unwrap());
        assert_eq!(commands, vec![Command::ResetAccount, Command::List]);
        runner.run(Command::ResetAccount);
        state.update(Msg::Listed(Ok(state.list.clone())));
        state.open_chat(16);
        old.send(shown("Old account's late transcript"));
        old.send(Msg::Listed(Ok(ChatList {
            account: Named {
                id: "482139075".into(),
                ..Named::default()
            },
            ..ChatList::default()
        })));
        assert!(
            runner.next().is_none(),
            "queued and late old account callbacks are discarded"
        );
        assert_eq!(state.list.account.id, "987654321");
        assert!(state.transcript.is_none());
        runner.wake().send(shown("New account's transcript"));
        state.update(runner.next().unwrap());
        assert_eq!(
            state.transcript.unwrap().chat.title,
            "New account's transcript"
        );
    }
}
