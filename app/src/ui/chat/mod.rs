//! The Chat page: your Chat with Work chats, drawn as the web app draws
//! them (docs/chat-window.md), through the daemon's chat API, as `cww tui`
//! uses it (CONTROL.md, "Chats").
//!
//! The page asks for frames only while something on it moves: a
//! transition, a streaming answer, running work's rings and shimmer. New
//! text arrives from the follower thread, which wakes the window.

mod account_settings;
pub mod cards;
pub mod composer;
mod connectors;
pub mod controls;
pub mod conversation;
pub mod dialogs;
pub mod highlight;
mod history;
pub mod icons;
mod instructions;
pub mod logos;
pub mod markdown;
pub mod mcp_app;
mod model_preferences;
mod organizations;
pub mod paint;
mod people;
mod project_move;
mod projects;
pub mod prose;
pub mod runner;
mod schedules;
mod settings;
pub mod sidebar;
pub mod state;
mod templates;
pub mod tokens;
pub mod widgets;
pub mod workspace;

use egui::{Id, Key, Modifiers, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use crate::model::Status;
use composer::Composer;
use conversation::{Conversation, ConversationView};
use icons::{Icon, Images};
use runner::Runner;
use sidebar::Sidebar;
use state::{Access, ChatState, Command};
use tokens::{Palette, Type};

/// What the page asks of the window around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Go to the settings, on this page.
    Settings(super::Page),
    StartAgent,
    Organization(Option<String>),
    Notification(cww::notifications::navigation::Target),
}

/// What the page needs to know about the daemon and the window.
pub struct Env<'a> {
    pub status: Option<&'a Status>,
    /// The daemon answered (or didn't) at least once.
    pub known: bool,
    pub dark: bool,
    /// `Start Local Agent` is under way.
    pub starting: bool,
}

const GREETINGS: [&str; 8] = [
    "What's up, {}?",
    "Hey {}, what's next?",
    "What can I do for you, {}?",
    "Hi {}, what's the task?",
    "What are we working on, {}?",
    "Hey {}, what needs doing?",
    "What's the job, {}?",
    "Hi {}, let's get to it.",
];

pub struct ChatPage {
    state: ChatState,
    runner: Runner,
    images: Option<Images>,
    conversation: ConversationView,
    /// The docked sidebar is collapsed (wide windows).
    collapsed: bool,
    /// The sidebar is open as a drawer (narrow windows).
    drawer: bool,
    focus_composer: bool,
    focus_search: bool,
    /// Frames left to keep scrolling to the latest message: the scroll
    /// area learns how tall the chat is a frame late.
    scroll_to_bottom: u8,
    /// Frames left to scroll the latest question to near the top, as the
    /// web's scroll controller does on opening a chat and on each new
    /// question; smoothly for a new one.
    pin: u8,
    pin_smooth: bool,
    /// Where the latest question was pinned, from the top of the
    /// conversation: room is kept under the answer to stay there.
    pinned: Option<f32>,
    /// The latest question seen, to notice a new one.
    last_user: Option<String>,
    /// The chat shown last frame, to start fresh on another.
    shown: Option<Option<u64>>,
    greeting: usize,
    /// When the new-chat screen last appeared, for its rise.
    new_chat_since: Option<f64>,
    /// Hold every animation at one moment, for snapshots.
    still: bool,
    /// The file picker is open; what's picked arrives here.
    picking: Option<std::sync::mpsc::Receiver<Vec<std::path::PathBuf>>>,
    saving: Option<std::sync::mpsc::Receiver<Option<Result<String, cww::tui::chat::Failure>>>>,
    /// The window, to wake when the file picker closes.
    ctx: Option<egui::Context>,
    /// When the list was last read because a chat in it was running.
    relisted: f64,
    workspace_refresh: f64,
    reach_menu: Option<usize>,
    remove_service: Option<cww::workspace::Service>,
    pending_navigation: Option<sidebar::Event>,
    organizations: Vec<super::organizations::Choice>,
    /// The daemon's endpoint and cww's data folder, where the web's images
    /// come from and are kept.
    socket: std::path::PathBuf,
    data_dir: std::path::PathBuf,
}

/// How often to read the list again while a chat in it is being answered
/// elsewhere, so its dot and shimmer stop when it's done.
const RELIST: f64 = 4.0;

/// The moment a still page shows: the shimmer mid-sweep, the caret on.
const STILL_TIME: f64 = 0.35;

impl ChatPage {
    pub(super) fn set_organizations(&mut self, choices: Vec<super::organizations::Choice>) {
        self.organizations = choices;
    }

    pub(super) fn open_templates(&mut self, project: Option<u64>) {
        let commands = self.state.show_templates(project);
        self.run(commands);
    }

    pub(super) fn open_model_preferences(&mut self) {
        if self.state.model_preferences.is_none() {
            let commands = self.state.show_model_preferences();
            self.run(commands);
        }
    }

    pub(super) fn navigation_busy(&self) -> bool {
        if self.state.people.as_ref().is_some_and(|p| p.busy) {
            return true;
        }
        if self.state.schedules.as_ref().is_some_and(|p| p.busy) {
            return true;
        }
        self.state.account_settings.as_ref().is_some_and(|p| p.busy)
            || self.state.templates.as_ref().is_some_and(|p| p.busy)
            || self
                .state
                .model_preferences
                .as_ref()
                .is_some_and(|p| p.busy)
            || self
                .state
                .instructions
                .as_ref()
                .is_some_and(|p| p.working())
            || self.state.projects.as_ref().is_some_and(|p| p.busy)
    }

    /// The same unsaved-instructions guard applies to notification navigation.
    pub(super) fn may_open_notification(
        &mut self,
        target: &cww::notifications::navigation::Target,
    ) -> bool {
        if self.state.templates.as_ref().is_some_and(|p| p.busy) {
            return false;
        }
        if let Some(page) = &mut self.state.schedules {
            if page.busy {
                return false;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        if let Some(page) = &mut self.state.people {
            if page.busy {
                return false;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        if let Some(page) = &mut self.state.account_settings {
            if page.busy {
                return false;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        if let Some(page) = &mut self.state.model_preferences {
            if page.busy {
                return false;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        if let Some(page) = &mut self.state.instructions {
            if page.working() {
                return false;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        if let Some(screen) = &mut self.state.projects {
            if screen.busy {
                return false;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Notification(target.clone()));
                return false;
            }
        }
        true
    }

    pub fn set_dictation_mode(&mut self, mode: cww::dictation::Mode) {
        if self.state.dictation.phase == cww::tui::dictation::Phase::Idle {
            self.state.dictation.mode = mode;
        }
    }
    pub fn new(socket: &std::path::Path, data_dir: std::path::PathBuf) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as usize);
        Self {
            state: ChatState::default(),
            runner: Runner::new(socket),
            images: None,
            conversation: ConversationView::default(),
            collapsed: false,
            drawer: false,
            focus_composer: false,
            focus_search: false,
            scroll_to_bottom: 0,
            pin: 3,
            pin_smooth: false,
            pinned: None,
            last_user: None,
            shown: None,
            greeting: seed % GREETINGS.len(),
            new_chat_since: None,
            still: false,
            picking: None,
            saving: None,
            ctx: None,
            relisted: 0.0,
            workspace_refresh: 0.0,
            reach_menu: None,
            remove_service: None,
            pending_navigation: None,
            organizations: Vec::new(),
            socket: socket.to_path_buf(),
            data_dir,
        }
    }

    /// Hold every animation still, for snapshots.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state(&self) -> &ChatState {
        &self.state
    }

    #[cfg(test)]
    pub fn conversation_mut(&mut self) -> &mut ConversationView {
        &mut self.conversation
    }

    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut ChatState {
        &mut self.state
    }

    /// The web's images still on their way.
    #[cfg(test)]
    pub fn logos_pending(&self) -> usize {
        self.images.as_ref().map_or(0, |i| i.logos.pending())
    }

    /// Pick the greeting (tests want the same one each time).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_greeting(&mut self, index: usize) {
        self.greeting = index % GREETINGS.len();
    }

    /// Open a chat, as choosing it in the sidebar does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open(&mut self, number: u64) {
        if self.state.templates.as_ref().is_some_and(|p| p.busy) {
            return;
        }
        if let Some(screen) = &mut self.state.schedules {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Open(number));
                return;
            }
        }
        if let Some(screen) = &mut self.state.people {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Open(number));
                return;
            }
        }
        if let Some(screen) = &mut self.state.account_settings {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Open(number));
                return;
            }
        }
        if let Some(screen) = &mut self.state.model_preferences {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Open(number));
                return;
            }
        }
        if let Some(screen) = &mut self.state.projects {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = Some(sidebar::Event::Open(number));
                return;
            }
        }
        if let Some(page) = &mut self.state.instructions
            && page.dirty()
        {
            page.confirm_close = true;
            self.pending_navigation = Some(sidebar::Event::Open(number));
            return;
        }
        let commands = self.state.open_chat(number);
        self.run(commands);
    }

    /// Discard account-local UI state, including egui's persistent undo buffers.
    pub(super) fn leave_organization(&mut self) {
        self.cancel_dictation();
        self.run(vec![Command::ResetAccount]);
        self.stop();
    }

    /// The window closes: stop following.
    pub fn stop(&self) {
        self.runner.stop();
    }

    pub fn cancel_dictation(&mut self) {
        let commands = self.state.cancel_dictation();
        self.run(commands);
    }

    fn run(&mut self, commands: Vec<Command>) {
        for mut command in commands {
            if let Command::ServicePause(_, _, _, project)
            | Command::ComputerRemove(_, _, project) = &mut command
            {
                *project = self.state.new_project.as_ref().map(|p| p.id);
            }
            if command == Command::ResetAccount {
                if let Some(ctx) = &self.ctx {
                    // Composer, instruction, rename and search undo buffers can
                    // otherwise bring the previous account's text back.
                    ctx.data_mut(|data| data.remove_by_type::<egui::text_edit::TextEditState>());
                    ctx.memory_mut(|memory| {
                        if let Some(focused) = memory.focused() {
                            memory.surrender_focus(focused);
                        }
                    });
                }
                self.focus_composer = false;
                self.focus_search = false;
                self.conversation = ConversationView::default();
                self.picking = None;
                self.saving = None;
                self.pending_navigation = None;
                self.remove_service = None;
                self.reach_menu = None;
                self.pinned = None;
                self.last_user = None;
                self.shown = None;
            }
            if let Command::Workspace(generation, _)
            | Command::ServicePause(generation, ..)
            | Command::ComputerRemove(generation, ..) = &command
            {
                self.state.start_loading = Some(*generation);
            }
            self.runner.run(command);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, env: &Env) -> Option<Action> {
        let ctx = ui.ctx().clone();
        self.runner.attach(&ctx);
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        let paired = env
            .status
            .is_some_and(|s| s.is_paired() && s.connection.connection != "revoked");
        let commands = self.state.set_paired(paired);
        self.run(commands);
        let mut account_changed = false;
        while let Some(msg) = self.runner.next() {
            if let state::Msg::ConnectorHandoff {
                generation,
                result: Ok(path),
            } = &msg
                && self.state.ready()
                && self
                    .state
                    .connectors
                    .as_ref()
                    .is_some_and(|dialog| dialog.handoff == Some(*generation))
                && let Some((account, endpoint)) =
                    path.strip_prefix('/').and_then(|path| path.split_once('/'))
                && account.len() == 9
                && account.bytes().all(|b| b.is_ascii_digit())
                && endpoint.starts_with("native/connector_handoff?")
                && !path.contains('\\')
                && let Some(server) = env.status.and_then(|status| status.server.as_deref())
            {
                self.open_on_server(env, &format!("{}{path}", server.trim_end_matches('/')));
            }
            let commands = self.update_state(&ctx, msg);
            account_changed |= commands.contains(&Command::ResetAccount);
            self.run(commands);
        }
        if !account_changed {
            if let Some(rx) = &self.saving
                && let Ok(result) = rx.try_recv()
            {
                self.saving = None;
                self.state.notice = result.map(|r| match r {
                    Ok(name) => format!("Saved {name}"),
                    Err(e) => e.message,
                });
            }
            // Files picked, or dropped on the window.
            if let Some(rx) = &self.picking
                && let Ok(paths) = rx.try_recv()
            {
                self.picking = None;
                self.attach(paths);
            }
            let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| p.is_absolute())
                    .collect()
            });
            if !dropped.is_empty() {
                self.attach(dropped);
            }
        }
        let now = ctx.input(|i| i.time);
        if self.state.ready()
            && self.state.open.is_none()
            && self.state.schedules.is_none()
            && self.state.people.is_none()
            && self.state.account_settings.is_none()
            && self.state.model_preferences.is_none()
            && self.state.templates.is_none()
            && self.state.instructions.is_none()
            && self.state.history.is_none()
            && self.state.projects.is_none()
        {
            if self.state.start_loading.is_none()
                && self.state.start_error.is_none()
                && (self.state.start.is_none() || now >= self.workspace_refresh)
            {
                self.workspace_refresh = now + 10.0;
                self.run(vec![Command::Workspace(
                    workspace::request_id(),
                    self.state.new_project.as_ref().map(|p| p.id),
                )]);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(10));
        }
        // A chat being answered elsewhere, while the server doesn't keep
        // the list current (an older daemon or server): read the list again
        // now and then, until it's done. Nothing runs, nothing is read.
        let running_elsewhere = self.state.ready()
            && !self.state.list_live
            && self
                .state
                .list
                .chats
                .iter()
                .any(|c| c.processing() && Some(c.number) != self.state.open);
        if running_elsewhere {
            let now = ctx.input(|i| i.time);
            if now - self.relisted >= RELIST {
                self.relisted = now;
                self.run(vec![Command::List]);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(RELIST));
        }
        if self.shown != Some(self.state.open) {
            self.shown = Some(self.state.open);
            self.conversation.reset();
            self.scroll_to_bottom = 0;
            self.pin = 3;
            self.pin_smooth = false;
            self.pinned = None;
            self.last_user = None;
            self.new_chat_since = None;
        }
        let images = self.images.take().unwrap_or_else(|| Images::load(&ctx));
        images.logos.set_source(
            &self.socket,
            env.status
                .filter(|_| paired)
                .and_then(|s| s.server.as_deref()),
            Some(&self.data_dir),
        );
        images.logos.poll(&ctx);
        let action = self.page(ui, env, &images);
        self.images = Some(images);
        action
    }

    fn update_state(&mut self, ctx: &egui::Context, msg: state::Msg) -> Vec<Command> {
        use egui::text::{CCursor, CCursorRange};
        let insertion = matches!(&msg, state::Msg::Dictation(_, cww::tui::dictation::Update::Finished(Ok(text))) if !text.is_empty());
        let id = Id::new(composer::ID);
        let mut editor =
            insertion.then(|| egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default());
        let before = insertion.then(|| self.state.input.clone());
        let previous = editor
            .as_ref()
            .and_then(|editor| editor.cursor.char_range());
        if insertion {
            self.state.dictation_cursor =
                previous.map(|range| (range.primary.index.0, range.secondary.index.0));
        }
        let commands = self.state.update(msg);
        if let (Some(editor), Some(before), Some((cursor, _))) =
            (editor.as_mut(), before, self.state.dictation_cursor)
            && before != self.state.input
        {
            let previous =
                previous.unwrap_or_else(|| CCursorRange::one(CCursor::new(before.chars().count())));
            let next = CCursorRange::one(CCursor::new(cursor));
            let mut undo = editor.undoer();
            undo.add_undo(&(previous, before));
            undo.add_undo(&(next, self.state.input.clone()));
            editor.set_undoer(undo);
            editor.cursor.set_char_range(Some(next));
            editor.clone().store(ctx, id);
            ctx.memory_mut(|memory| {
                if memory
                    .focused()
                    .is_none_or(|focused| focused == id || focused == id.with("dictate"))
                {
                    memory.request_focus(id);
                }
            });
            self.focus_composer = false;
        }
        commands
    }

    fn page(&mut self, ui: &mut Ui, env: &Env, images: &Images) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let p = Palette::new(env.dark);
        let time = if self.still {
            STILL_TIME
        } else {
            ctx.input(|i| i.time)
        };
        let full = ui.max_rect();
        if !self.state.ready() {
            self.cancel_dictation();
        }
        let mut action = None;
        let wide = full.width() >= tokens::WIDE;
        let docked = wide && !self.collapsed;
        if wide {
            self.drawer = false;
        }

        // Shortcuts, as on the web.
        let (search, new_chat, focus, escape) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::K)),
                i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::ALT, Key::N)),
                i.consume_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash)),
                i.key_pressed(Key::Escape),
            )
        });
        if search {
            self.focus_search = true;
            if !docked {
                self.drawer = true;
                self.collapsed = false;
            }
        }
        if new_chat && self.state.ready() {
            self.sidebar_event(Some(sidebar::Event::NewChat), env, &mut action);
        }
        if focus {
            self.focus_composer = true;
        }
        if escape && self.state.dictation.phase != cww::tui::dictation::Phase::Idle {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            let commands = self.state.dictate();
            self.run(commands);
        } else if escape && self.drawer {
            self.drawer = false;
        }

        ui.painter().rect_filled(full, 0.0, p.canvas);
        let main = if docked {
            Rect::from_min_max(
                pos2(full.left() + tokens::SIDEBAR_WIDTH, full.top()),
                full.max,
            )
        } else {
            full
        };

        let conversing = self.state.open.is_some() || self.state.pending_question.is_some();
        let mut animating = false;
        if let Some(screen) = &mut self.state.schedules {
            let outcome = schedules::show(
                ui,
                main,
                screen,
                &self.state.list.links.settings,
                &p,
                images,
            );
            if !screen.confirm_close && !outcome.discarded {
                self.pending_navigation = None;
            }
            if outcome.discarded {
                self.state.schedules = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            self.sidebar_event(outcome.event, env, &mut action);
        } else if let Some(screen) = &mut self.state.people {
            let outcome = people::show(
                ui,
                main,
                screen,
                &self.state.list.links.settings,
                &p,
                images,
            );
            if !screen.confirm_close && !outcome.discarded {
                self.pending_navigation = None;
            }
            if outcome.discarded {
                self.state.people = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            self.sidebar_event(outcome.event, env, &mut action);
        } else if let Some(screen) = &mut self.state.account_settings {
            let outcome = account_settings::show(
                ui,
                main,
                screen,
                &self.state.list.links.settings,
                &p,
                images,
            );
            if !screen.confirm_close && !outcome.discarded {
                self.pending_navigation = None;
            }
            if outcome.discarded {
                self.state.account_settings = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            self.sidebar_event(outcome.event, env, &mut action);
        } else if let Some(screen) = &mut self.state.model_preferences {
            let outcome = model_preferences::show(
                ui,
                main,
                screen,
                &self.state.list.links.settings,
                &p,
                images,
            );
            if !screen.confirm_close && !outcome.discarded {
                self.pending_navigation = None;
            }
            if outcome.discarded {
                self.state.model_preferences = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            self.sidebar_event(outcome.event, env, &mut action);
        } else if let Some(screen) = &mut self.state.templates {
            let outcome = templates::show(
                ui,
                main,
                screen,
                &self.state.list.links.settings,
                &p,
                images,
            );
            self.run(outcome.commands);
            if let Some(file) = outcome.download {
                self.save_file(cww::tui::chat::Attachment {
                    filename: file.name,
                    path: Some(file.download_path),
                    ..Default::default()
                });
            }
            self.sidebar_event(outcome.event, env, &mut action);
        } else if let Some(page) = &mut self.state.instructions {
            let outcome =
                instructions::show(ui, main, page, &self.state.list.links.settings, &p, images);
            if !page.confirm_close && !outcome.close {
                self.pending_navigation = None;
            }
            if outcome.close {
                self.state.instructions = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            if let Some(file) = outcome.download {
                self.save_file(cww::tui::chat::Attachment {
                    filename: file.name,
                    path: Some(file.download_path),
                    ..Default::default()
                });
            }
            self.sidebar_event(outcome.event, env, &mut action);
            if let Some(chat) = outcome.open_chat {
                self.sidebar_event(Some(sidebar::Event::Open(chat)), env, &mut action);
            }
        } else if let Some(screen) = &mut self.state.projects {
            let outcome =
                projects::show(ui, main, screen, &self.state.list.account.name, &p, images);
            if !screen.confirm_close && !outcome.discarded {
                self.pending_navigation = None;
            }
            if outcome.discarded {
                self.state.projects = None;
                let pending = self.pending_navigation.take();
                self.sidebar_event(pending, env, &mut action);
            }
            self.run(outcome.commands);
            self.sidebar_event(outcome.event, env, &mut action);
        } else if self.state.history.is_some() {
            let (event, load) = history::show(ui, main, &mut self.state, &p, images);
            if load {
                let commands = self.state.load_history();
                self.run(commands);
            }
            self.sidebar_event(event, env, &mut action);
        } else if self.state.moving.is_some() {
            let commands = project_move::show(ui, main, &mut self.state, &p, images);
            self.run(commands);
        } else if conversing {
            animating |= self.conversation_area(ui, main, &p, images, time, env, &mut action);
        } else {
            animating |= self.new_chat_area(ui, main, &p, images, time, env, &mut action);
        }

        if let Some(dialog) = &mut self.state.connectors {
            let (open, commands) = dialog.show(ui, self.state.start_loading.is_none(), &p, images);
            if !open {
                self.state.connectors = None;
                self.state.start = None;
            }
            self.run(commands);
        }

        // Moving things: running work, streaming, uploads, the drawer.
        if !self.still
            && (self.state.working()
                || self.state.uploading()
                || self.state.list.chats.iter().any(|c| c.processing()))
        {
            animating = true;
        }

        // The sidebar's own button, while it isn't docked.
        if !docked {
            let rect = Rect::from_min_size(main.min + vec2(8.0, 8.0), vec2(32.0, 32.0));
            egui::Area::new(Id::new("chat-open-sidebar"))
                .order(egui::Order::Middle)
                .fixed_pos(rect.min)
                .show(&ctx, |ui| {
                    let response = widgets::IconButton {
                        icon: Icon::SidebarSimple,
                        icon_size: 20.0,
                        label: "Open sidebar",
                        tooltip: None,
                        color: p.ink,
                        hover_color: p.ink,
                        wash: 7.0,
                        radius: tokens::RADIUS_CONTROL - 2.0,
                        enabled: true,
                    }
                    .show(
                        ui,
                        Id::new("chat-open-sidebar-button"),
                        rect,
                        images,
                        &p,
                    );
                    if response.clicked() {
                        if wide {
                            self.collapsed = false;
                        } else {
                            self.drawer = true;
                        }
                    }
                });
        }

        if self.state.schedules.is_none()
            && self.state.people.is_none()
            && self.state.account_settings.is_none()
            && self.state.model_preferences.is_none()
            && self.state.templates.is_none()
            && self.state.instructions.is_none()
            && self.state.history.is_none()
            && self.state.projects.is_none()
            && self.state.moving.is_none()
            && let Some(chat) = self
                .state
                .open_summary()
                .filter(|chat| chat.can(|can| can.pin))
        {
            let number = chat.number;
            let pinned = self.state.list.pins.chats.contains(&number);
            let mut clicked = false;
            egui::Area::new(Id::new("chat-pin-action"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(main.right() - 48.0, main.top() + 8.0))
                .show(&ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    if pinned {
                        ui.painter()
                            .rect_filled(rect, tokens::RADIUS_CONTROL, p.ink_wash(6.0));
                    }
                    clicked = widgets::IconButton {
                        icon: if pinned {
                            Icon::PushPinSlash
                        } else {
                            Icon::PushPin
                        },
                        icon_size: 16.0,
                        label: if pinned {
                            "Unpin from sidebar"
                        } else {
                            "Pin to sidebar"
                        },
                        tooltip: Some(if pinned {
                            "Unpin from sidebar"
                        } else {
                            "Pin to sidebar"
                        }),
                        color: p.ink_muted,
                        hover_color: p.ink,
                        wash: 7.0,
                        radius: tokens::RADIUS_CONTROL,
                        enabled: self.state.acting.is_none(),
                    }
                    .show(ui, Id::new("chat-pin-button"), rect, images, &p)
                    .clicked();
                });
            if clicked {
                let commands = self.state.act(number, state::ChatAction::Pin(!pinned));
                self.run(commands);
            }
        }

        let user = if self.state.list.user.name.is_empty() {
            "Settings".to_string()
        } else {
            self.state.list.user.name.clone()
        };
        let drawer_t = ctx.animate_bool_with_time_and_easing(
            Id::new("chat-drawer"),
            self.drawer,
            tokens::DRAWER,
            tokens::ease_out,
        );
        if docked {
            let rect = Rect::from_min_size(full.min, vec2(tokens::SIDEBAR_WIDTH, full.height()));
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
            let event = Sidebar {
                images,
                organizations: &self.organizations,
                palette: &p,
                time,
                drawer: false,
                user: &user,
                focus_search: std::mem::take(&mut self.focus_search),
            }
            .show(&mut child, rect, &mut self.state);
            self.sidebar_event(event, env, &mut action);
        } else if drawer_t > 0.0 {
            animating |= drawer_t < 1.0;
            let width = tokens::SIDEBAR_WIDTH.min(full.width() * 0.85);
            egui::Area::new(Id::new("chat-drawer-area"))
                .order(egui::Order::Foreground)
                .fixed_pos(full.min)
                .show(&ctx, |ui| {
                    // The backdrop dims the conversation and closes on a click.
                    let backdrop =
                        ui.interact(full, Id::new("chat-drawer-backdrop"), Sense::click());
                    ui.painter().rect_filled(
                        full,
                        0.0,
                        egui::Color32::from_black_alpha((0.4 * 255.0 * drawer_t) as u8),
                    );
                    if backdrop.clicked() {
                        self.drawer = false;
                    }
                    let rect = Rect::from_min_size(
                        pos2(full.left() - width * (1.0 - drawer_t), full.top()),
                        vec2(width, full.height()),
                    );
                    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
                    // Swallow clicks under the drawer.
                    child.interact(rect, Id::new("chat-drawer-body"), Sense::click());
                    let event = Sidebar {
                        images,
                        organizations: &self.organizations,
                        palette: &p,
                        time,
                        drawer: true,
                        user: &user,
                        focus_search: std::mem::take(&mut self.focus_search),
                    }
                    .show(&mut child, rect, &mut self.state);
                    if matches!(
                        event,
                        Some(
                            sidebar::Event::Open(_)
                                | sidebar::Event::NewChat
                                | sidebar::Event::Page(_)
                                | sidebar::Event::Organization(_)
                                | sidebar::Event::Notification(_)
                                | sidebar::Event::Move(_)
                                | sidebar::Event::History
                                | sidebar::Event::Projects(_)
                                | sidebar::Event::ProjectChat(_)
                                | sidebar::Event::ProjectInstructions(_)
                                | sidebar::Event::Instructions(_)
                                | sidebar::Event::Schedules(_)
                                | sidebar::Event::People
                                | sidebar::Event::AccountSettings
                                | sidebar::Event::ModelPreferences
                                | sidebar::Event::Templates(_)
                                | sidebar::Event::Toggle
                        )
                    ) {
                        self.drawer = false;
                    }
                    self.sidebar_event(event, env, &mut action);
                });
        }

        // The dialog over it all, and the clipboard.
        for outcome in dialogs::show(&ctx, &mut self.state, &p, images) {
            match outcome {
                dialogs::Outcome::Commands(commands) => self.run(commands),
                dialogs::Outcome::Copy(text) => ctx.copy_text(text),
            }
        }
        if let Some(text) = self.state.clipboard.take() {
            ctx.copy_text(text);
        }

        if animating {
            ctx.request_repaint();
        }
        action
    }

    fn sidebar_event(
        &mut self,
        event: Option<sidebar::Event>,
        env: &Env,
        action: &mut Option<Action>,
    ) {
        let leaving_models = matches!(
            &event,
            Some(
                sidebar::Event::NewChat
                    | sidebar::Event::Open(_)
                    | sidebar::Event::Page(_)
                    | sidebar::Event::Organization(_)
                    | sidebar::Event::Notification(_)
                    | sidebar::Event::Move(_)
                    | sidebar::Event::History
                    | sidebar::Event::Projects(_)
                    | sidebar::Event::ProjectChat(_)
                    | sidebar::Event::ProjectInstructions(_)
                    | sidebar::Event::Instructions(_)
                    | sidebar::Event::Schedules(_)
                    | sidebar::Event::People
                    | sidebar::Event::AccountSettings
                    | sidebar::Event::ModelPreferences
                    | sidebar::Event::Templates(_)
                    | sidebar::Event::OpenUrl(_)
            )
        );
        if leaving_models && let Some(screen) = &mut self.state.schedules {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
            self.state.schedules = None;
        }
        if leaving_models && let Some(screen) = &mut self.state.people {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
            self.state.people = None;
        }
        if leaving_models && let Some(screen) = &mut self.state.account_settings {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
            self.state.account_settings = None;
        }
        if leaving_models && let Some(screen) = &self.state.templates {
            if screen.busy {
                return;
            }
            self.state.templates = None;
        }
        if leaving_models && let Some(screen) = &mut self.state.model_preferences {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
            self.state.model_preferences = None;
        }
        if matches!(
            event,
            Some(
                sidebar::Event::NewChat
                    | sidebar::Event::Open(_)
                    | sidebar::Event::Page(_)
                    | sidebar::Event::Organization(_)
                    | sidebar::Event::Notification(_)
                    | sidebar::Event::Move(_)
                    | sidebar::Event::History
                    | sidebar::Event::Projects(_)
                    | sidebar::Event::ProjectChat(_)
                    | sidebar::Event::ProjectInstructions(_)
                    | sidebar::Event::Instructions(_)
                    | sidebar::Event::Schedules(_)
                    | sidebar::Event::People
                    | sidebar::Event::AccountSettings
                    | sidebar::Event::ModelPreferences
                    | sidebar::Event::Templates(_)
            )
        ) && let Some(page) = &mut self.state.instructions
        {
            if page.working() {
                return;
            }
            if page.dirty() {
                page.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
            self.state.instructions = None;
        }
        let leaving_project = matches!(
            &event,
            Some(
                sidebar::Event::NewChat
                    | sidebar::Event::Open(_)
                    | sidebar::Event::Page(_)
                    | sidebar::Event::Organization(_)
                    | sidebar::Event::Notification(_)
                    | sidebar::Event::Move(_)
                    | sidebar::Event::History
                    | sidebar::Event::ProjectChat(_)
                    | sidebar::Event::ProjectInstructions(_)
                    | sidebar::Event::Instructions(_)
                    | sidebar::Event::Schedules(_)
                    | sidebar::Event::People
                    | sidebar::Event::AccountSettings
                    | sidebar::Event::ModelPreferences
                    | sidebar::Event::Templates(_)
            )
        ) || matches!(
            &event,
            Some(sidebar::Event::Projects(
                cww::projects::Request::List
                    | cww::projects::Request::New
                    | cww::projects::Request::Edit { .. }
                    | cww::projects::Request::Show { .. }
            ))
        );
        if leaving_project && let Some(screen) = &mut self.state.projects {
            if screen.busy {
                return;
            }
            if screen.dirty() {
                screen.confirm_close = true;
                self.pending_navigation = event;
                return;
            }
        }
        match event {
            Some(sidebar::Event::Schedules(request)) => {
                let commands = self.state.schedule_request(request);
                self.run(commands);
            }
            Some(sidebar::Event::People) => {
                let commands = self.state.show_people();
                self.run(commands);
            }
            Some(sidebar::Event::AccountSettings) => {
                let commands = self.state.show_account_settings();
                self.run(commands);
            }
            Some(sidebar::Event::ModelPreferences) => {
                let commands = self.state.show_model_preferences();
                self.run(commands);
            }
            Some(sidebar::Event::Templates(project)) => self.open_templates(project),
            Some(sidebar::Event::Projects(request)) => {
                let commands = self.state.project_request(request);
                self.run(commands);
                self.drawer = false;
            }
            Some(sidebar::Event::ProjectChat(project)) => {
                let commands = self.state.new_chat();
                self.state.new_project = Some(project);
                self.run(commands);
                self.focus_composer = true;
            }
            Some(sidebar::Event::Instructions(layer)) => {
                let mut page = workspace::Instructions::new(layer);
                page.account = Some(self.state.list.account.id.clone());
                let generation = page.generation;
                self.state.instructions = Some(page);
                self.drawer = false;
                self.run(vec![Command::Instructions(generation, None)]);
            }
            Some(sidebar::Event::ProjectInstructions(project)) => {
                let mut page = workspace::Instructions::new(cww::workspace::Layer::Project);
                page.account = Some(self.state.list.account.id.clone());
                page.project = Some(project);
                page.back_to_project = true;
                let generation = page.generation;
                self.state.instructions = Some(page);
                self.run(vec![Command::Instructions(generation, Some(project))]);
            }
            Some(sidebar::Event::NewChat) => {
                let commands = self.state.new_chat();
                self.run(commands);
                self.focus_composer = true;
            }
            Some(sidebar::Event::History) => {
                let commands = self.state.show_history();
                self.run(commands);
                self.drawer = false;
            }
            Some(sidebar::Event::OpenUrl(url)) => self.open_on_server(env, &url),
            Some(sidebar::Event::Page(page)) => *action = Some(Action::Settings(page)),
            Some(sidebar::Event::Organization(id)) => *action = Some(Action::Organization(id)),
            Some(sidebar::Event::Notification(target)) => {
                *action = Some(Action::Notification(target))
            }
            Some(sidebar::Event::Open(number)) => {
                let commands = self.state.open_chat(number);
                self.run(commands);
                self.focus_composer = true;
            }
            Some(sidebar::Event::Toggle) => {
                if self.drawer {
                    self.drawer = false;
                } else {
                    self.collapsed = true;
                }
            }
            Some(sidebar::Event::Rename(chat, title)) => {
                let commands = self.state.act(chat, state::ChatAction::Rename(title));
                self.run(commands);
            }
            Some(sidebar::Event::Delete(chat)) => {
                self.state.dialog = Some(state::Dialog::Delete { chat });
            }
            Some(sidebar::Event::Pin(chat, pinned)) => {
                let commands = self.state.act(chat, state::ChatAction::Pin(pinned));
                self.run(commands);
            }
            Some(sidebar::Event::Move(chat)) => {
                let commands = self.state.move_chat(chat);
                self.run(commands);
                self.drawer = false;
            }
            None => {}
        }
    }

    /// The project the open chat is in, as the list knows it.
    fn open_project(&self) -> Option<cww::tui::chat::Project> {
        let project = self.state.open_summary()?.project.as_ref()?;
        Some(
            self.state
                .project(project.id)
                .cloned()
                .unwrap_or_else(|| project.clone()),
        )
    }

    /// Why the composer is locked right now, and the way out.
    fn lock(&self, env: &Env) -> Option<(String, Option<&'static str>)> {
        if !env.known {
            return Some(("Looking for the Local Agent…".into(), None));
        }
        let Some(status) = env.status else {
            let action = if env.starting { None } else { Some("Start it") };
            let reason = if env.starting {
                "Starting the Local Agent…"
            } else {
                "The Local Agent isn't running."
            };
            return Some((reason.into(), action));
        };
        if status.connection.connection == "revoked" {
            return Some((
                "This computer's pairing was revoked.".into(),
                Some("Pair again"),
            ));
        }
        if !status.is_paired() {
            return Some((
                "This computer isn't paired with Chat with Work.".into(),
                Some("Pair it"),
            ));
        }
        match &self.state.access {
            Access::Unknown | Access::Loading => Some(("Loading your chats…".into(), None)),
            Access::NeedsApproval {
                requested: false, ..
            } => Some((
                "Chat with Work hasn't allowed this computer to use your chats.".into(),
                Some("Ask for access"),
            )),
            Access::NeedsApproval {
                requested: true,
                url,
            } => Some((
                "Asked. Allow it in Chat with Work: Settings, Computers.".into(),
                url.as_ref().map(|_| "Open Settings"),
            )),
            Access::Unavailable(failure) => Some((failure.message.clone(), Some("Try again"))),
            Access::Ready => self.state.locked_reason().map(|r| (r.to_string(), None)),
        }
    }

    fn lock_action(&mut self, env: &Env, action: &mut Option<Action>) {
        if env.status.is_none() {
            *action = Some(Action::StartAgent);
            return;
        }
        if env
            .status
            .is_some_and(|s| !s.is_paired() || s.connection.connection == "revoked")
        {
            *action = Some(Action::Settings(super::Page::Account));
            return;
        }
        match &self.state.access {
            Access::NeedsApproval {
                requested: false, ..
            } => {
                let commands = self.state.request_access();
                self.run(commands);
            }
            Access::NeedsApproval { url: Some(url), .. } => {
                let url = url.clone();
                self.open_on_server(env, &url);
            }
            Access::Unavailable(_) => {
                let commands = self.state.retry();
                self.run(commands);
            }
            _ => {}
        }
    }

    /// Open a page of the paired server, the only kind of page the app
    /// opens on the server's say-so.
    fn open_on_server(&self, env: &Env, url: &str) {
        let on_server = env
            .status
            .and_then(|s| s.server.as_deref())
            .is_some_and(|server| url.starts_with(&format!("{}/", server.trim_end_matches('/'))));
        if on_server {
            super::open_url(url);
        }
    }

    /// The open chat in the browser, or the server's new chat.
    fn open_chat_in_browser(&self, env: &Env) {
        match self.state.open_summary().map(|c| c.url.clone()) {
            Some(url) if !url.is_empty() => self.open_on_server(env, &url),
            _ => {
                if let Some(server) = env.status.and_then(|s| s.server.as_deref()) {
                    super::open_url(server);
                }
            }
        }
    }

    /// The native file picker, for files to attach. It's built on this
    /// thread (AppKit wants that) and awaited on another; what's picked
    /// comes back through `picked`.
    fn pick_files(&mut self) {
        if self.picking.is_some() || !self.state.ready() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.picking = Some(rx);
        #[cfg(test)]
        {
            // Tests never open a window: they say what was picked.
            let _ = tx.send(
                tests::PICKED
                    .with(|p| p.borrow_mut().take())
                    .unwrap_or_default(),
            );
        }
        #[cfg(not(test))]
        {
            let mut dialog = rfd::AsyncFileDialog::new().set_title("Attach Files");
            if let Some(home) = crate::paths::home_dir() {
                dialog = dialog.set_directory(home);
            }
            let picked = dialog.pick_files();
            let ctx = self.ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("cww-chat-pick".into())
                .spawn(move || {
                    let files = pollster::block_on(picked)
                        .map(|files| files.iter().map(|f| f.path().to_path_buf()).collect())
                        .unwrap_or_default();
                    let _ = tx.send(files);
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                });
            if let Err(e) = spawned {
                log::error!("can't open the file picker: {e}");
                self.picking = None;
            }
        }
    }

    fn save_file(&mut self, file: cww::tui::chat::Attachment) {
        if self.saving.is_some() {
            return;
        }
        let Some(path) = file.path.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.saving = Some(rx);
        let chats = cww::tui::chat::Chats::new(&self.socket);
        let name = file.save_name().to_string();
        #[cfg(test)]
        let picked = {
            let destination = tests::SAVED.with(|p| p.borrow_mut().take());
            async move { destination }
        };
        #[cfg(not(test))]
        let picked = {
            let dialog = rfd::AsyncFileDialog::new()
                .set_title("Save File")
                .set_file_name(&name)
                .save_file();
            async move { dialog.await.map(|f| f.path().to_path_buf()) }
        };
        let ctx = self.ctx.clone();
        self.state.notice = Some(format!("Saving {name}…"));
        if let Err(e) = std::thread::Builder::new()
            .name("cww-chat-save".into())
            .spawn(move || {
                let result = pollster::block_on(picked).map(|destination| {
                    chats
                        .save_download(&path, &destination, true)
                        .map(|()| name)
                });
                let _ = tx.send(result);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            })
        {
            self.saving = None;
            self.state.notice = Some(format!("Could not open the save dialog: {e}"));
        }
    }

    /// Files to attach: picked, or dropped on the window.
    pub fn attach(&mut self, paths: Vec<std::path::PathBuf>) {
        let commands = self.state.attach(paths);
        self.run(commands);
    }

    fn composer_event(
        &mut self,
        event: Option<composer::Event>,
        env: &Env,
        action: &mut Option<Action>,
    ) {
        match event {
            Some(composer::Event::Send) => {
                // The question shows, and is pinned near the top as it does.
                let commands = self.state.send();
                self.run(commands);
            }
            Some(composer::Event::Stop) => {
                let commands = self.state.stop();
                self.run(commands);
            }
            Some(composer::Event::Dictate) => {
                let commands = self.state.dictate();
                self.run(commands);
                self.focus_composer = true;
            }
            Some(composer::Event::Action) => self.lock_action(env, action),
            Some(composer::Event::Attach) => self.pick_files(),
            Some(composer::Event::Detach(key)) => self.state.detach(key),
            Some(composer::Event::PickModel(id)) => self.state.pick_model(&id),
            Some(composer::Event::ToggleWebSearch) => self.state.toggle_web_search(),
            None => {}
        }
    }

    fn composer<'a>(
        &self,
        lock: &'a Option<(String, Option<&'static str>)>,
        time: f64,
    ) -> Composer<'a> {
        let notice_free = lock
            .as_ref()
            .map(|(reason, action)| (reason.as_str(), *action));
        Composer {
            dictation: self.state.dictation.clone(),
            web_search: self.state.web_search_control(),
            placeholder: if self.state.open.is_some() {
                "Reply to Chat with Work"
            } else {
                "Ask anything…"
            },
            locked: notice_free.map(|(r, _)| r),
            action: notice_free.and_then(|(_, a)| a),
            working: self.state.working(),
            stopping: self.state.stopping,
            sending: self.state.sending,
            can_send: self.state.can_send().is_ok(),
            time,
            attachments: self.state.attachments.clone(),
            models: match &self.state.models {
                state::ModelList::Ready(models) => models.models.clone(),
                _ => Vec::new(),
            },
            model: self.state.current_model(),
            model_logo: self
                .state
                .open_summary()
                .and_then(|c| c.model.as_ref())
                .and_then(|m| m.logo.clone()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn conversation_area(
        &mut self,
        ui: &mut Ui,
        main: Rect,
        p: &Palette,
        images: &Images,
        time: f64,
        env: &Env,
        action: &mut Option<Action>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let column = (main.width() - 32.0).min(tokens::COLUMN);
        let mut animating = false;
        let mut events = Vec::new();
        let mut project_url = None;
        let mut change_project = false;
        let mut child = ui.new_child(UiBuilder::new().max_rect(main));
        // Back to the latest message, from the scroll button.
        let scroll_to_bottom = self.scroll_to_bottom > 0 && !self.state.loading;
        if scroll_to_bottom {
            self.scroll_to_bottom -= 1;
            self.pinned = None;
            ctx.request_repaint();
        }
        let output = egui::ScrollArea::vertical()
            .id_salt(("chat-conversation", self.state.open))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let origin = ui.cursor().top();
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(24.0);
                let left = main.center().x - column / 2.0;
                let mut col = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(
                            pos2(left, ui.cursor().top()),
                            vec2(column, f32::INFINITY),
                        ))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                col.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if let Some(project) = self.open_project() {
                    (project_url, change_project) = project_line(
                        &mut col,
                        &project,
                        &self.state.list.account.name,
                        self.state
                            .open_summary()
                            .is_some_and(|chat| chat.can(|can| can.r#move)),
                        images,
                        p,
                    );
                    col.add_space(12.0);
                }
                let (found, moving) = Conversation {
                    palette: p,
                    images,
                    time,
                    still: self.still,
                }
                .show(&mut col, &self.state, &mut self.conversation);
                events = found;
                animating |= moving;
                if let Some(error) = &self.state.error {
                    col.add_space(12.0);
                    empty_state(
                        &mut col,
                        images,
                        p,
                        Icon::WarningCircle,
                        "This chat isn't available",
                        error,
                    );
                }
                let height = col.min_rect().height();
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                ui.add_space(176.0);
                // The web's scroll controller: a new question (asked here or
                // elsewhere), and the latest one when a chat opens, scrolls
                // to 24 under the top, and the answer writes itself in under
                // it; the page doesn't follow it down.
                let latest = self.conversation.latest_user.clone();
                if let Some((key, _)) = &latest
                    && self.last_user.as_ref() != Some(key)
                {
                    if self.last_user.is_some() {
                        self.pin = 3;
                        self.pin_smooth = true;
                    }
                    self.last_user = Some(key.clone());
                }
                let pinning = self.pin > 0 && !self.state.loading;
                if pinning {
                    self.pin -= 1;
                    ui.ctx().request_repaint();
                    self.pinned = latest.as_ref().map(|(_, y)| (y - origin - 24.0).max(0.0));
                }
                // Room under the answer, so the question can stay pinned
                // while the answer is still short.
                if let Some(target) = self.pinned {
                    let content = ui.cursor().top() - origin;
                    ui.add_space((target - (content - main.height())).max(0.0));
                }
                match (pinning, self.pinned) {
                    (true, Some(target)) => {
                        let animation = if self.pin_smooth && !self.still {
                            egui::style::ScrollAnimation::default()
                        } else {
                            egui::style::ScrollAnimation::none()
                        };
                        ui.scroll_to_rect_animation(
                            Rect::from_min_size(pos2(main.left(), origin + target), vec2(1.0, 1.0)),
                            Some(egui::Align::TOP),
                            animation,
                        );
                    }
                    // Nothing asked yet: the end.
                    (true, None) => ui.scroll_to_cursor_animation(
                        Some(egui::Align::BOTTOM),
                        egui::style::ScrollAnimation::none(),
                    ),
                    _ => {}
                }
                if scroll_to_bottom {
                    ui.scroll_to_cursor_animation(
                        Some(egui::Align::BOTTOM),
                        egui::style::ScrollAnimation::none(),
                    );
                }
            });
        if change_project && let Some(chat) = self.state.open {
            self.sidebar_event(Some(sidebar::Event::Move(chat)), env, action);
        }
        if let Some(url) = project_url {
            self.open_on_server(env, &url);
        }
        for event in events {
            match event {
                conversation::Event::OpenUrl(url) => super::open_url(&url),
                conversation::Event::OpenChat => self.open_chat_in_browser(env),
                conversation::Event::Copy(text) => ctx.copy_text(text),
                conversation::Event::CopyAnswer(text) => {
                    crate::clipboard::answer(&ctx, text, self.state.list.copy_answers_as)
                }
                conversation::Event::Download(file) => self.save_file(file),
                conversation::Event::Retry { message, user } => {
                    if let Some(chat) = self.state.open {
                        self.state.dialog = Some(state::Dialog::Retry {
                            chat,
                            message,
                            user,
                        });
                    }
                }
                conversation::Event::Branch(message) => {
                    if let Some(chat) = self.state.open {
                        let commands = self.state.act(chat, state::ChatAction::Branch(message));
                        self.run(commands);
                    }
                }
                conversation::Event::Share => {
                    if let Some(chat) = self.state.open {
                        self.state.dialog = Some(state::Dialog::Share {
                            chat,
                            copied: false,
                        });
                    }
                }
                conversation::Event::Decide(decision) => {
                    let commands = self.state.decide(decision);
                    self.run(commands);
                }
            }
        }
        let behind = output.content_size.y - output.inner_rect.height() - output.state.offset.y;
        let near_bottom = behind < 96.0;

        // The dock: the composer over the bottom of the conversation.
        let lock = self.lock(env);
        let composer = self.composer(&lock, time);
        let width = main.width().min(tokens::COLUMN + 16.0) - 16.0;
        let height = composer.height(ui, &self.state.input, width);
        let rect = Rect::from_min_size(
            pos2(main.center().x - width / 2.0, main.bottom() - 12.0 - height),
            vec2(width, height),
        );
        let focus = std::mem::take(&mut self.focus_composer);
        let mut event = None;
        let mut jump = false;
        // Not constrained to the window: a constrained area keeps last
        // frame's size and creeps up from the bottom edge, covering the
        // conversation above the composer.
        egui::Area::new(Id::new("chat-dock"))
            .order(egui::Order::Middle)
            .constrain(false)
            .fixed_pos(rect.min)
            .show(&ctx, |ui| {
                ui.allocate_rect(rect, Sense::hover());
                event = composer.show(ui, rect, &mut self.state.input, images, p, focus);
                jump = self.scroll_button(ui, rect, near_bottom, p, images, time);
            });
        if jump {
            self.scroll_to_bottom = 3;
            self.pinned = None;
        }
        if let Some(notice) = self.state.notice.clone() {
            notice_toast(&ctx, rect, &notice, p, images);
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.state.notice = None;
            }
        }
        self.composer_event(event, env, action);
        animating
    }

    /// `.scroll-button`: back to the latest message, with a live dot while
    /// more streams in. True when clicked.
    fn scroll_button(
        &self,
        ui: &mut Ui,
        composer: Rect,
        near_bottom: bool,
        p: &Palette,
        images: &Images,
        time: f64,
    ) -> bool {
        let id = Id::new("chat-scroll-button");
        let t = ui.ctx().animate_bool_with_time_and_easing(
            id,
            !near_bottom,
            tokens::BASE,
            tokens::ease_snap,
        );
        if t <= 0.0 {
            return false;
        }
        let center = pos2(
            composer.center().x,
            composer.top() - 48.0 + 18.0 + 8.0 * (1.0 - t),
        );
        let rect = Rect::from_center_size(center, vec2(36.0, 36.0));
        let response = ui.interact(
            rect,
            id.with("button"),
            if t > 0.5 {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                "Scroll to the latest message",
            )
        });
        let hover = widgets::hover(ui, id, response.hovered(), tokens::FAST);
        let painter = ui.painter();
        paint::float_shadow(
            painter,
            rect,
            18.0,
            &Palette {
                shadow: p.shadow.gamma_multiply(t),
                ..*p
            },
        );
        painter.circle_filled(center, 18.0, p.surface_raised.gamma_multiply(t));
        painter.circle_stroke(
            center,
            18.5,
            egui::Stroke::new(1.0, p.line_strong.gamma_multiply(t)),
        );
        let color = tokens::lerp_rgb(p.ink_muted, p.ink, hover).gamma_multiply(t);
        images.icon_at(painter, center, 16.0, Icon::ArrowDown, color);
        if self.state.working() {
            let dot = pos2(rect.right() - 2.0 - 4.0, rect.top() + 2.0 + 4.0);
            painter.circle_filled(dot, 6.0, p.surface_raised.gamma_multiply(t));
            painter.circle_filled(dot, 4.0, p.live().gamma_multiply(t));
        }
        let _ = time;
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.clicked()
    }

    #[allow(clippy::too_many_arguments)]
    fn new_chat_area(
        &mut self,
        ui: &mut Ui,
        main: Rect,
        p: &Palette,
        images: &Images,
        time: f64,
        env: &Env,
        action: &mut Option<Action>,
    ) -> bool {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(main));
        child.set_clip_rect(main);
        egui::ScrollArea::vertical()
            .id_salt("new-chat-page")
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_min_size(main.size());
                let content = Rect::from_min_size(ui.cursor().min, main.size());
                self.new_chat_content(ui, content, p, images, time, env, action)
            })
            .inner
    }

    #[allow(clippy::too_many_arguments)]
    fn new_chat_content(
        &mut self,
        ui: &mut Ui,
        main: Rect,
        p: &Palette,
        images: &Images,
        time: f64,
        env: &Env,
        action: &mut Option<Action>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let full_width = ctx.content_rect().width();
        let since = *self.new_chat_since.get_or_insert(time);
        let rise = if self.still {
            1.0
        } else {
            ((time - since) as f32 / tokens::SLOW).clamp(0.0, 1.0)
        };
        let eased = tokens::ease_snap(rise);

        // The faded dot grid behind it all.
        let backdrop = Rect::from_min_max(
            pos2(main.left() + 16.0 - 0.3 * full_width, main.top() - 32.0),
            pos2(main.right() - 16.0 + 0.3 * full_width, main.bottom() + 32.0),
        );
        paint::dot_grid(
            &ui.painter().with_clip_rect(main),
            backdrop,
            22.0,
            p.dot,
            true,
        );

        let column = (main.width() - 32.0).min(672.0);
        let lock = self.lock(env);
        let composer = self.composer(&lock, time);
        let composer_h = composer.height(ui, &self.state.input, column);
        let size = (0.036 * full_width).clamp(28.0, 40.0);
        let ty = Type::sans(size, size * 1.1).weight(650.0).tracking(-0.045);
        let name = self
            .state
            .list
            .user
            .name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let greeting = if name.is_empty() {
            "What are we working on?".to_string()
        } else {
            GREETINGS[self.greeting].replace("{}", &name)
        };
        let galley = paint::layout(ui.painter(), paint::job(&greeting, ty, p.ink, column));
        let start = self
            .state
            .start
            .as_ref()
            .map(|data| workspace::StartLayout::new(ui, column, data, p));
        let project_context = self.state.new_project.as_ref().map(|project| {
            let label = format!(
                "{} · shared with {}",
                project.name,
                project.audience.replacen("Everyone", "everyone", 1)
            );
            (
                project.number,
                paint::layout(
                    ui.painter(),
                    paint::job(&label, Type::sans(13.0, 19.5), p.ink_muted, column - 48.0),
                ),
                label,
            )
        });
        let context_height = project_context
            .as_ref()
            .map_or(0.0, |(_, galley, _)| galley.size().y + 12.0);
        let content = galley.size().y
            + context_height
            + 28.0
            + composer_h
            + start.as_ref().map_or(0.0, |layout| layout.height);
        ui.set_min_height((content + 168.0).max(main.height()));
        // The column's padding, and the page's 8 at the bottom.
        let available = main.height() - 64.0 - 96.0 - 8.0;
        let top = main.top() + 64.0 + ((available - content) / 2.0).max(0.0);
        if std::env::var_os("CWW_DEBUG_LAYOUT").is_some() {
            eprintln!(
                "new chat: main {main:?} greeting {:?} composer {composer_h} top {top}",
                galley.size()
            );
        }
        let left = main.center().x - column / 2.0;
        let greeting_pos = pos2(
            main.center().x - galley.size().x / 2.0,
            top + 8.0 * (1.0 - eased),
        );
        let greeting_rect = Rect::from_min_size(greeting_pos, galley.size());
        widgets::label(ui, greeting_rect, &greeting);
        let greeting_height = galley.size().y;
        ui.painter().galley_with_override_text_color(
            greeting_pos,
            galley,
            p.ink.gamma_multiply(eased),
        );
        if let Some((number, galley, label)) = project_context {
            let context = Rect::from_min_size(
                pos2(
                    main.center().x - galley.size().x / 2.0,
                    top + greeting_height + 28.0,
                ),
                galley.size(),
            );
            let response = ui.interact(context, Id::new("new-project-context"), Sense::click());
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, &label));
            ui.painter().galley(context.min, galley, p.ink_muted);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                self.sidebar_event(
                    Some(sidebar::Event::Projects(cww::projects::Request::Show {
                        project: number,
                        before: None,
                    })),
                    env,
                    action,
                );
            }
        }
        let rect = Rect::from_min_size(
            pos2(left, top + greeting_height + 28.0 + context_height),
            vec2(column, composer_h),
        );
        let focus = std::mem::take(&mut self.focus_composer);
        let event = composer.show(ui, rect, &mut self.state.input, images, p, focus);
        if let Some(notice) = self.state.notice.clone() {
            notice_toast(&ctx, rect, &notice, p, images);
        }
        self.composer_event(event, env, action);
        if let (Some(start), Some(data)) = (start, self.state.start.as_ref()) {
            let start_rect =
                Rect::from_min_size(pos2(left, rect.bottom()), vec2(column, start.height));
            let event = start.show(
                ui,
                start_rect,
                data,
                &mut self.reach_menu,
                self.state.can_send().is_ok(),
                self.state.start_loading.is_some(),
                p,
                images,
            );
            match event {
                Some(workspace::Event::Ask(prompt)) => {
                    self.state.input = prompt;
                    let commands = self.state.send();
                    self.run(commands);
                }
                Some(workspace::Event::Pause(service, paused)) => {
                    self.run(vec![Command::ServicePause(
                        workspace::request_id(),
                        service,
                        paused,
                        self.state.new_project.as_ref().map(|p| p.id),
                    )]);
                }
                Some(workspace::Event::Instructions(layer)) => {
                    let mut page = workspace::Instructions::new(layer);
                    page.account = Some(self.state.list.account.id.clone());
                    page.project = if layer == cww::workspace::Layer::Project {
                        self.state.new_project.as_ref().map(|p| p.id)
                    } else {
                        None
                    };
                    let generation = page.generation;
                    let project = page.project;
                    self.state.instructions = Some(page);
                    self.run(vec![Command::Instructions(generation, project)]);
                }
                Some(workspace::Event::Connect) => {
                    self.state.connectors = Some(connectors::ConnectDialog::default())
                }
                Some(workspace::Event::Remove(service)) => self.remove_service = Some(service),
                None => {}
            }
        }
        if let Some(error) = self.state.start_error.clone() {
            let y = rect.bottom()
                + self.state.start.as_ref().map_or(20.0, |data| {
                    workspace::StartLayout::new(ui, column, data, p).height + 16.0
                });
            let error_text = paint::layout(
                ui.painter(),
                paint::job(
                    &error,
                    tokens::scale::SMALLER,
                    p.negative_ink(),
                    (column - 80.0).max(1.0),
                ),
            );
            ui.painter()
                .galley(pos2(left, y), error_text, p.negative_ink());
            let retry =
                controls::Button::new(controls::Kind::Subtle, controls::Size::Small, "Try again");
            if retry
                .show_at(
                    ui,
                    Id::new("workspace-retry"),
                    pos2(left + column - retry.width(ui), y),
                    p,
                    images,
                )
                .clicked()
            {
                self.state.start_error = None;
                self.run(vec![Command::Workspace(
                    workspace::request_id(),
                    self.state.new_project.as_ref().map(|p| p.id),
                )]);
            }
        } else if self.state.start.is_none() {
            let label = paint::layout(
                ui.painter(),
                paint::job(
                    "Loading your workspace…",
                    tokens::scale::SMALLER,
                    p.ink_muted,
                    column,
                ),
            );
            ui.painter().galley(
                pos2(main.center().x - label.size().x / 2.0, rect.bottom() + 24.0),
                label,
                p.ink_muted,
            );
        }
        if let Some(service) = self.remove_service.clone() {
            let mut remove = false;
            let mut cancel = false;
            let open = controls::modal(
                &ctx,
                Id::new("remove-computer"),
                480.0,
                235.0,
                p,
                |ui, rect| {
                    controls::dialog_header(
                        ui,
                        rect.min,
                        rect.width(),
                        &format!("Remove {}?", service.name),
                        &[(
                            "This disconnects the computer and removes its cached files from Chat with Work. Pair it again to reconnect.",
                            false,
                        )],
                        p,
                    );
                    let y = rect.bottom() - 32.0;
                    cancel = controls::Button::new(
                        controls::Kind::Subtle,
                        controls::Size::Small,
                        "Cancel",
                    )
                    .show_at(
                        ui,
                        Id::new("remove-computer-cancel"),
                        pos2(rect.left(), y),
                        p,
                        images,
                    )
                    .clicked();
                    remove = controls::Button::new(
                        controls::Kind::Error,
                        controls::Size::Small,
                        "Remove computer",
                    )
                    .show_at(
                        ui,
                        Id::new("remove-computer-confirm"),
                        pos2(rect.left() + 86.0, y),
                        p,
                        images,
                    )
                    .clicked();
                },
            );
            if remove && let Ok(id) = service.key.parse() {
                self.run(vec![Command::ComputerRemove(
                    workspace::request_id(),
                    id,
                    self.state.new_project.as_ref().map(|p| p.id),
                )]);
            }
            if remove || cancel || !open {
                self.remove_service = None;
            }
        }
        rise < 1.0
    }
}

/// The project's audience and the creator's Change link, wrapping on phones.
fn project_line(
    ui: &mut Ui,
    project: &cww::tui::chat::Project,
    account: &str,
    movable: bool,
    images: &Images,
    p: &Palette,
) -> (Option<String>, bool) {
    let mut url = None;
    let mut change = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        let (icon, _) = ui.allocate_exact_size(vec2(16.0, 19.5), Sense::hover());
        images.icon_at(
            ui.painter(),
            icon.center(),
            16.0,
            sidebar::project_icon(project),
            p.ink_faint,
        );
        let name = ui.add(
            egui::Label::new(paint::job(
                &project.name,
                tokens::scale::SMALLER.weight(550.0),
                p.ink,
                ui.available_width(),
            ))
            .sense(Sense::click()),
        );
        name.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, project.url.is_some(), &project.name)
        });
        if name.hovered() && project.url.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if name.clicked() {
            url = project.url.clone();
        }
        let audience = if project.hq {
            format!(" · shared with everyone in {}", account.trim())
        } else {
            project.people_count.map_or_else(
                || " · shared with the people on it".into(),
                |n| {
                    format!(
                        " · shared with {n} {}",
                        if n == 1 { "person" } else { "people" }
                    )
                },
            )
        };
        ui.add(
            egui::Label::new(paint::job(
                &audience,
                tokens::scale::SMALLER,
                p.ink_muted,
                ui.available_width(),
            ))
            .wrap(),
        );
        if movable {
            let response = ui.add(
                egui::Label::new(paint::job(
                    "Change",
                    tokens::scale::SMALLER.weight(550.0),
                    p.ink_muted,
                    f32::INFINITY,
                ))
                .sense(Sense::click()),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Link, true, "Change project")
            });
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            change = response.clicked();
        }
    });
    (url, change)
}

/// A failure over the composer: a question that didn't go, a file or a
/// model refused.
fn notice_toast(ctx: &egui::Context, composer: Rect, text: &str, p: &Palette, images: &Images) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    let galley = paint::layout(
        &painter,
        paint::job(text, tokens::scale::SMALL, p.ink, composer.width() - 64.0),
    );
    let height = galley.size().y + 20.0;
    let rect = Rect::from_min_size(
        pos2(composer.left(), composer.top() - 12.0 - height),
        vec2(composer.width(), height),
    );
    egui::Area::new(Id::new("chat-notice"))
        .order(egui::Order::Foreground)
        .constrain(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(rect, Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
            let painter = ui.painter();
            paint::tinted_card(
                painter,
                rect,
                p.negative(),
                p.negative_wash(),
                p.negative_edge(),
            );
            images.icon_at(
                painter,
                pos2(rect.left() + 16.0 + 9.0, rect.top() + 10.0 + 10.0),
                18.0,
                Icon::WarningCircle,
                p.negative(),
            );
            painter.galley(pos2(rect.left() + 46.0, rect.top() + 10.0), galley, p.ink);
        });
}

/// `.empty-state`: a dashed card on the dot grid with an icon, one bold
/// line and one sentence.
pub fn empty_state(ui: &mut Ui, images: &Images, p: &Palette, icon: Icon, title: &str, text: &str) {
    let width = ui.available_width();
    let painter = ui.painter().clone();
    let title_galley = paint::layout(
        &painter,
        paint::job(
            title,
            tokens::scale::SMALL.weight(550.0),
            p.ink,
            width - 48.0,
        ),
    );
    let text_galley = paint::layout(
        &painter,
        paint::job(text, tokens::scale::SMALL, p.ink_muted, width - 48.0),
    );
    let height = 40.0 + 24.0 + 8.0 + title_galley.size().y + 8.0 + text_galley.size().y + 40.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{title} {text}"))
    });
    paint::dot_grid(
        &painter.with_clip_rect(rect.shrink(1.0)),
        rect,
        18.0,
        p.dot,
        false,
    );
    // A dashed hairline.
    let outline = paint::rounded_outline(rect.shrink(0.5), [tokens::RADIUS_CARD; 4], 8);
    let mut closed = outline.clone();
    closed.push(outline[0]);
    painter.add(egui::Shape::dashed_line(
        &closed,
        egui::Stroke::new(1.0, p.line_strong),
        4.0,
        3.0,
    ));
    let mut y = rect.top() + 40.0;
    images.icon_at(
        &painter,
        pos2(rect.center().x, y + 12.0),
        24.0,
        icon,
        p.ink_faint,
    );
    y += 24.0 + 8.0;
    painter.galley(
        pos2(rect.center().x - title_galley.size().x / 2.0, y),
        title_galley.clone(),
        p.ink,
    );
    y += title_galley.size().y + 8.0;
    painter.galley(
        pos2(rect.center().x - text_galley.size().x / 2.0, y),
        text_galley,
        p.ink_muted,
    );
}

/// The window's style on the Chat page: the platform's text rendering,
/// with the web's selection, caret and thin scroll bars.
pub fn style(mut style: egui::Style, dark: bool) -> egui::Style {
    let p = Palette::new(dark);
    let v = &mut style.visuals;
    v.override_text_color = None;
    v.panel_fill = p.canvas;
    v.window_fill = p.surface_raised;
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.selection.bg_fill = tokens::alpha(p.blue, 0.3);
    v.selection.stroke = egui::Stroke::new(1.0, p.ink);
    v.text_cursor.stroke = egui::Stroke::new(1.5, p.ink);
    // A blinking caret redraws twice a second; a steady one costs nothing.
    v.text_cursor.blink = false;
    v.hyperlink_color = p.ink;
    for (state, fill) in [
        (&mut v.widgets.inactive, p.line_strong),
        (&mut v.widgets.hovered, p.ink_faint),
        (&mut v.widgets.active, p.ink_faint),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = egui::Stroke::new(1.0, p.ink);
    }
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.ink);
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        floating_width: 3.0,
        floating_allocated_width: 0.0,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.8,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::floating()
    };
    style.spacing.item_spacing = vec2(0.0, 0.0);
    style
}

#[cfg(test)]
mod tests;
