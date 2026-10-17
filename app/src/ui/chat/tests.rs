//! The Chat page, driven through its accessibility tree against the
//! stand-in daemon, which records every request it gets and serves a
//! synthetic organization's chats. The snapshot test renders the page
//! offscreen with wgpu and keeps PNGs in `app/tests/snapshots`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::Value;

use crate::control::Client;
use crate::demo::{self, Scenario};
use crate::model::Platform;
use crate::ui::tests::OPENED;
use crate::ui::theme::Theme;
use crate::ui::{AppState, Page, SettingsApp, Shared};

thread_local! {
    /// What the next file picker "picks": tests never open a window.
    pub static SAVED: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    pub static PICKED: std::cell::RefCell<Option<Vec<std::path::PathBuf>>> =
        const { std::cell::RefCell::new(None) };
}

struct Fixture {
    server: demo::Server,
    harness: Harness<'static, SettingsApp>,
}

struct Options {
    scenario: Scenario,
    size: [f32; 2],
    dark: bool,
    held: bool,
    /// The daemon refuses to keep the list current, as one from before
    /// live lists does.
    old_daemon: bool,
    web_search: bool,
    renderer: Option<egui_kittest::wgpu::WgpuTestRenderer>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            scenario: Scenario::Sample,
            size: [1280.0, 820.0],
            dark: false,
            held: true,
            old_daemon: false,
            web_search: false,
            renderer: None,
        }
    }
}

fn fixture(options: Options) -> Fixture {
    let server = demo::start(options.scenario).unwrap();
    server.hold_streams(options.held);
    if options.old_daemon {
        server.act_old();
    }
    if options.web_search {
        server.enable_web_search();
    }
    if options.scenario == Scenario::Sample {
        AppState {
            onboarded: true,
            ..Default::default()
        }
        .save(&server.paths);
    }
    let shared = Shared::new(
        Client::new(server.paths.socket_path()),
        server.paths.clone(),
        server.account(),
    );
    let watcher = Arc::clone(&shared);
    shared
        .client
        .spawn_watch(move |status| watcher.set_status(status));
    let mut app = SettingsApp::new(shared, Theme::new(Platform::current(), options.dark, None));
    app.set_page(Page::Chat);
    let mut builder = Harness::builder().with_size(options.size);
    if let Some(renderer) = options.renderer {
        builder = builder.renderer(renderer);
    }
    // The whole window, without the harness's own margin.
    let harness = builder.build_ui_state(
        |ui, app: &mut SettingsApp| {
            let screen = ui.ctx().content_rect();
            let mut window = ui.new_child(egui::UiBuilder::new().max_rect(screen));
            window.set_clip_rect(screen);
            app.show(&mut window);
        },
        app,
    );
    let mut fixture = Fixture { server, harness };
    fixture.wait("the first status", |h| h.state().shared().is_known());
    fixture
}

impl Fixture {
    fn wait(&mut self, what: &str, done: impl Fn(&Harness<'static, SettingsApp>) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.harness.step();
            if done(&self.harness) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_label(&mut self, label: &str) {
        let label = label.to_string();
        self.wait(&format!("“{label}”"), move |h| {
            h.query_all_by_label_contains(&label).next().is_some()
        });
    }

    fn wait_for_request(&mut self, cmd: &str, check: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(found) = self
                .server
                .requests()
                .into_iter()
                .find(|r| r["cmd"] == cmd && check(r))
            {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "no {cmd} request: {:?}",
                self.server.requests()
            );
            self.harness.step();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn chat(&mut self) -> &mut super::ChatPage {
        self.harness.state_mut().chat().expect("the chat page")
    }

    fn open(&mut self, number: u64) {
        let title = self.title(number);
        // The first daemon status can arrive before the chat list. Wait for
        // the actual sidebar row, just as a person must before clicking it.
        self.wait(&format!("the chat row {title}"), |h| {
            h.query_by_label(&title).is_some()
        });
        self.harness.get_by_label(&title).click();
        self.harness.run_steps(2);
    }

    fn title(&self, number: u64) -> String {
        match number {
            12 => "Q3 budget review",
            11 => "Vendor contract renewal",
            _ => "Weekly sync summary",
        }
        .into()
    }
}

#[test]
fn chats_are_listed_by_day_and_open_with_their_answers() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    // One Recent list, as the web's sidebar has.
    fx.harness.get_by_label("Recent");
    assert!(fx.harness.query_by_label("Yesterday").is_none());
    fx.chat().set_greeting(4);
    fx.harness.run_steps(1);
    fx.harness
        .get_by_label_contains("What are we working on, Alex?");
    fx.harness.get_by_label("Vendor contract renewal").click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    fx.wait_for_request("subscribe", |r| r["chat"] == "11");
    fx.wait_for_label("When does the Acme contract renew");
    fx.harness
        .get_by_label_contains("The Acme contract renews on March 1, 2027");
    fx.harness
        .get_by_label_contains("Annual fee · €40,000 · €42,000");
    fx.harness.get_by_label("Source 1: Acme renewal 2026.pdf");
    // A Drive link of another shape still finds its source.
    fx.harness.get_by_label("Source 2: Acme MSA 2024.pdf");
    fx.harness.get_by_label_contains("\"auto_renew\": true");
    fx.harness.get_by_label_contains("Acme renews on March 1.");
    // The open chat is the selected one.
    let row = fx.harness.get_by_label("Vendor contract renewal");
    let node = row.accesskit_node();
    assert!(
        node.is_selected() == Some(true) || node.toggled() == Some(egui::accesskit::Toggled::True),
        "the open chat is selected"
    );
}

#[test]
fn new_chat_workspace_pauses_resumes_and_asks_suggested_questions() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Google Drive, connected");
    fx.harness.get_by_label("Google Drive, connected").click();
    fx.wait_for_label("Pause");
    fx.harness.get_by_label("Pause").click();
    fx.wait_for_request("service_pause", |r| {
        r["key"] == "google" && r["paused"] == true
    });
    fx.wait_for_label("Google Drive, paused");
    fx.harness.get_by_label("Google Drive, paused").click();
    fx.wait_for_label("Resume");
    fx.harness.get_by_label("Resume").click();
    fx.wait_for_label("Google Drive, connected");
    fx.harness.get_by_label("Find a document").click();
    let asked = fx.wait_for_request("chat_send", |_| true);
    assert_eq!(
        asked["text"],
        "Find the latest Acme contract in Google Drive."
    );
}

#[test]
fn new_chat_workspace_connects_in_the_browser_with_the_chosen_access() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Connect a service");
    fx.harness.get_by_label("Connect a service").click();
    fx.wait_for_label("Search services");
    fx.harness.get_by_label("Search services").click();
    fx.harness
        .get_by_label("Search services")
        .type_text("Linear");
    fx.wait_for_request("connectors", |r| r["query"] == "Linear");
    fx.wait("the filtered catalog", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .connectors
            .as_ref()
            .unwrap()
            .catalog
            .as_ref()
            .is_some_and(|c| c.connectors.len() == 1)
    });
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Choose access").click();
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Read and change").click();
    fx.harness.run_steps(3);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "connector_handoff")
    );
    OPENED.with(|opened| opened.borrow_mut().clear());
    fx.harness.get_by_label("Read").click();
    fx.wait_for_request("connector_handoff", |r| {
        r["key"] == "linear" && r["access"] == "read"
    });
    fx.wait_for_label("Finish signing in in your browser, then return here.");
    OPENED.with(|opened| assert_eq!(opened.borrow().last().map(String::as_str), Some("https://chatwithwork.com/482139075/native/connector_handoff?key=linear&access=read&token=synthetic-demo")));
}

#[test]
fn new_chat_workspace_confirms_computer_removal() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Alex's laptop, connected");
    fx.harness.get_by_label("Alex's laptop, connected").click();
    fx.wait_for_label("Remove…");
    fx.harness.get_by_label("Remove…").click();
    fx.wait_for_label("Remove computer");
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "computer_remove")
    );
    fx.harness.get_by_label("Remove computer").click();
    fx.wait_for_request("computer_remove", |r| r["id"] == 42);
    fx.wait("the computer to disappear", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .start
            .as_ref()
            .is_some_and(|s| s.services.len() == 2)
    });
}

#[test]
fn new_chat_workspace_edits_instructions_and_brings_a_version_back() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("write your instructions");
    fx.harness
        .query_all_by_label("write your instructions")
        .next()
        .unwrap()
        .click();
    fx.wait_for_request("instructions", |_| true);
    fx.wait_for_label("Use a template");
    fx.harness.run_steps(8);
    fx.harness.get_by_label("Use a template").click();
    fx.harness.run_steps(15);
    fx.harness.get_by_label("Save").click();
    let first = fx.wait_for_request("instruction_update", |r| r["change"]["kind"] == "edit");
    assert_eq!(first["layer"], "personal");
    assert!(
        first["change"]["body"]
            .as_str()
            .unwrap()
            .starts_with("## About me")
    );
    fx.wait_for_label("Saved your instructions.");
    {
        let page = fx.chat().state.instructions.as_mut().unwrap();
        let editor = page
            .editors
            .iter_mut()
            .find(|e| e.instruction.layer == cww::workspace::Layer::Personal)
            .unwrap();
        editor.body = "- Prefer tables for numbers.".into();
    }
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Save").click();
    fx.wait_for_request("instruction_update", |r| {
        r["change"]["body"] == "- Prefer tables for numbers."
    });
    fx.wait_for_label("Earlier versions (1)");
    fx.harness
        .event(egui::Event::PointerMoved(egui::pos2(900.0, 600.0)));
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        phase: egui::TouchPhase::Move,
        delta: egui::vec2(0.0, -1000.0),
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(15);
    fx.harness.get_by_label("Earlier versions (1)").click();
    fx.harness.run_steps(15);
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        phase: egui::TouchPhase::Move,
        delta: egui::vec2(0.0, -1000.0),
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(15);
    fx.harness.get_by_label("Bring back").click();
    fx.wait_for_request("instruction_update", |r| r["change"]["revision_id"] == 1);
    fx.wait_for_label("Brought back that version of your instructions.");
    assert!(
        fx.chat()
            .state
            .instructions
            .as_ref()
            .unwrap()
            .editors
            .iter()
            .find(|e| e.instruction.layer == cww::workspace::Layer::Personal)
            .unwrap()
            .body
            .starts_with("## About me")
    );
}

#[test]
fn new_chat_workspace_keeps_unsaved_edits_and_controls_memory() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("write your instructions");
    fx.harness
        .query_all_by_label("write your instructions")
        .next()
        .unwrap()
        .click();
    fx.wait_for_label("Use a template");
    fx.harness.run_steps(8);
    fx.harness.get_by_label("Use a template").click();
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Vendor contract renewal").click();
    fx.wait_for_label("Discard unsaved instructions?");
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(3);
    assert!(fx.chat().state.instructions.as_ref().unwrap().dirty());
    assert!(fx.chat().state.open.is_none());
    fx.harness
        .event(egui::Event::PointerMoved(egui::pos2(900.0, 600.0)));
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        phase: egui::TouchPhase::Move,
        delta: egui::vec2(0.0, -2000.0),
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(15);
    fx.harness
        .get_by_role_and_label(
            egui::accesskit::Role::CheckBox,
            "Let the assistant remember things",
        )
        .click();
    fx.wait_for_request("memory_preference", |r| r["enabled"] == false);
    fx.wait("memory is disabled", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .instructions
            .as_ref()
            .unwrap()
            .memory
            .as_ref()
            .is_some_and(|m| !m.enabled)
    });
    fx.harness.get_by_label("Prefers short answers");
    fx.harness.get_by_label("Forget").click();
    fx.wait_for_request("memory_forget", |r| r["id"] == 1);
    fx.wait_for_label("Nothing remembered yet");
    assert!(fx.chat().state.instructions.as_ref().unwrap().dirty());
    fx.harness.get_by_label("Vendor contract renewal").click();
    fx.wait_for_label("Discard unsaved instructions?");
    fx.harness.get_by_label("Discard changes").click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    assert!(fx.chat().state.instructions.is_none());
}

fn notification_target(
    server: &str,
    account: &str,
    chat: u64,
) -> cww::notifications::navigation::Target {
    cww::notifications::navigation::Target::new(
        cww::auth::client::ServerUrl::parse(server).unwrap(),
        account.into(),
        chat,
    )
    .unwrap()
}

#[test]
fn notification_click_opens_only_the_chat_in_the_current_account_and_server() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.harness.state_mut().set_page(Page::Folders);
    fx.harness.state().shared().open_chat(notification_target(
        "https://chatwithwork.com",
        "482139075",
        11,
    ));
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    fx.wait("notification chat loaded", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.open == Some(11) && !s.loading)
    });
    assert_eq!(fx.harness.state().page(), Page::Chat);
    for (server, account) in [
        ("https://another.example", "482139075"),
        ("https://chatwithwork.com", "999999999"),
    ] {
        fx.harness
            .state()
            .shared()
            .open_chat(notification_target(server, account, 12));
        fx.harness.run_steps(5);
        assert_eq!(fx.chat().state.open, Some(11));
        assert!(
            !fx.server
                .requests()
                .iter()
                .any(|r| r["cmd"] == "chat" && r["chat"] == "12")
        );
    }
    // An older server without the account header must not guess.
    fx.chat().state.list.account.id.clear();
    fx.harness.state().shared().open_chat(notification_target(
        "https://chatwithwork.com",
        "482139075",
        12,
    ));
    fx.harness.run_steps(5);
    assert_eq!(fx.chat().state.open, Some(11));
}

#[test]
fn notification_click_preserves_the_unsaved_instructions_confirmation() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("write your instructions");
    fx.harness
        .query_all_by_label("write your instructions")
        .next()
        .unwrap()
        .click();
    fx.wait_for_label("Use a template");
    fx.harness.run_steps(8);
    fx.harness.get_by_label("Use a template").click();
    fx.harness.run_steps(3);
    fx.harness.state().shared().open_chat(notification_target(
        "https://chatwithwork.com",
        "482139075",
        11,
    ));
    fx.wait_for_label("Discard unsaved instructions?");
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "chat" && r["chat"] == "11")
    );
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(3);
    assert!(fx.chat().state.instructions.as_ref().unwrap().dirty());
    fx.harness.state().shared().open_chat(notification_target(
        "https://chatwithwork.com",
        "482139075",
        11,
    ));
    fx.wait_for_label("Discard unsaved instructions?");
    fx.harness.get_by_label("Discard changes").click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    assert!(fx.chat().state.instructions.is_none());
}

#[test]
fn notification_click_waits_for_the_new_pairings_account_header() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.chat().state.list.account.id = "999999999".into();
    let mut status = fx.harness.state().shared().status().unwrap();
    status.device_id = Some("new-device".into());
    fx.harness.state().shared().set_status(Some(status));
    fx.harness.state().shared().open_chat(notification_target(
        "https://chatwithwork.com",
        "999999999",
        11,
    ));
    fx.harness.run_steps(1);
    fx.wait("fresh account header", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.ready() && s.list.account.id == "482139075")
    });
    fx.harness.run_steps(4);
    assert!(fx.chat().state.open.is_none());
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "chat" && r["chat"] == "11")
    );
}

#[test]
fn account_header_reset_clears_editor_undo_and_pending_file_choices() {
    use cww::tui::chat::{AccountUpdate, ListLive, Named};
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    let field = fx
        .harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("Private draft from the old account");
    fx.harness.run_steps(3);
    assert!(!fx.chat().state.input.is_empty());
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join("old-private-file.txt");
    std::fs::write(&file, "Synthetic old-account attachment").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(vec![file]).unwrap();
    fx.chat().picking = Some(rx);
    let commands = fx
        .chat()
        .state
        .update(super::state::Msg::ListLive(ListLive::Account(
            AccountUpdate {
                account: Named {
                    id: "987654321".into(),
                    ..Named::default()
                },
                ..AccountUpdate::default()
            },
        )));
    fx.chat().run(commands);
    assert!(fx.chat().picking.is_none());
    assert!(fx.chat().state.input.is_empty());
    assert!(fx.harness.ctx.memory(|memory| memory.focused()).is_none());
    assert!(
        egui::text_edit::TextEditState::load(&fx.harness.ctx, egui::Id::new(super::composer::ID))
            .is_none()
    );
    fx.harness.run_steps(8);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "chat_upload")
    );
}

#[test]
fn notification_navigation_screenshots() {
    let Ok(directory) = std::env::var("CWW_NOTIFICATION_NAVIGATION_SCREENSHOTS") else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [false, true] {
        for (name, size) in [("wide", [1280.0, 820.0]), ("narrow", [760.0, 820.0])] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(renderer),
                ..Default::default()
            });
            fx.wait("chat list loaded", |h| {
                h.state().chat_state().is_some_and(|s| s.ready())
            });
            fx.chat().set_still(true);
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(5);
            let theme = if dark { "dark" } else { "light" };
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!("before-{name}-{theme}.png")))
                .unwrap();
            fx.harness.state().shared().open_chat(notification_target(
                "https://chatwithwork.com",
                "482139075",
                11,
            ));
            fx.wait("notification chat loaded", |h| {
                h.state()
                    .chat_state()
                    .is_some_and(|s| s.open == Some(11) && !s.loading)
            });
            fx.caught_up();
            fx.logos();
            fx.harness.run_steps(5);
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!("after-{name}-{theme}.png")))
                .unwrap();
        }
    }
}

#[test]
fn renders_new_chat_workspace() {
    for dark in [false, true] {
        for (name, size) in [
            ("workspace", [1280.0, 820.0]),
            ("workspace-narrow", [760.0, 820.0]),
        ] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                return;
            };
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(renderer),
                ..Default::default()
            });
            fx.wait_for_label("Google Drive, connected");
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(15);
            let theme = if dark { "dark" } else { "light" };
            let directory = std::env::var_os("CWW_WORKSPACE_EVIDENCE");
            if let Some(directory) = &directory {
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(std::path::Path::new(directory).join(format!("{name}-{theme}.png")))
                    .unwrap();
            }
            fx.harness
                .query_all_by_label("write your instructions")
                .next()
                .unwrap()
                .click();
            fx.wait_for_label("Use a template");
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(15);
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(
                    &fx.harness.render().expect("GPU rendering"),
                    format!("{name}-instructions-{theme}"),
                    &egui_kittest::SnapshotOptions::new()
                        .threshold(2.0)
                        .max_failed_pixels(((size[0] * size[1]) * 0.01) as usize),
                );
            }
            if let Some(directory) = &directory {
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(
                        std::path::Path::new(directory)
                            .join(format!("{name}-instructions-{theme}.png")),
                    )
                    .unwrap();
            }
            fx.chat().state.instructions = None;
            fx.harness.run_steps(3);
            fx.harness.get_by_label("Connect a service").click();
            fx.wait_for_request("connectors", |_| true);
            fx.wait_for_label("Search services");
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(15);
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(
                    &fx.harness.render().expect("GPU rendering"),
                    format!("{name}-connectors-{theme}"),
                    &egui_kittest::SnapshotOptions::new()
                        .threshold(2.0)
                        .max_failed_pixels(((size[0] * size[1]) * 0.01) as usize),
                );
            }
            if let Some(directory) = &directory {
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(
                        std::path::Path::new(directory)
                            .join(format!("{name}-connectors-{theme}.png")),
                    )
                    .unwrap();
            }
        }
    }
}

#[test]
fn the_activity_line_opens_into_its_steps() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.open(11);
    fx.wait_for_label("Searched Drive and Slack · 3 searches, read 2 files");
    assert!(
        fx.harness
            .query_by_label("Searched Drive for “Acme contract”")
            .is_none()
    );
    // Above the fold: clicked as a screen reader would.
    fx.harness
        .get_by_label("Searched Drive and Slack · 3 searches, read 2 files")
        .click_accesskit();
    fx.wait_for_label("Searched Drive for “Acme contract”");
    fx.harness
        .get_by_label("Acme renewal 2026.pdf, Acme MSA 2024.pdf");
}

#[test]
fn sources_open_in_the_browser() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.open(11);
    fx.wait_for_label("2 sources");
    // Let it scroll to the latest question first: this answer is above it,
    // so it's clicked as a screen reader would.
    fx.harness.run_steps(5);
    fx.harness.get_by_label("2 sources").click_accesskit();
    fx.wait_for_label("1. Acme renewal 2026.pdf");
    fx.harness
        .get_by_label("2. Acme MSA 2024.pdf")
        .click_accesskit();
    fx.harness.run_steps(2);
    // The chip is above the fold now: clicked as a screen reader would.
    fx.harness
        .get_by_label("Source 1: Acme renewal 2026.pdf")
        .click_accesskit();
    fx.harness.run_steps(2);
    let opened = OPENED.with(|o| o.borrow().clone());
    assert_eq!(
        opened,
        [
            "https://drive.google.com/file/d/1AcmeMsa2024Signed/view",
            "https://drive.google.com/file/d/1AcmeRenewal2026Final/view",
        ]
    );
}

#[test]
fn a_question_streams_its_answer_in() {
    let mut fx = fixture(Options {
        held: false,
        ..Options::default()
    });
    fx.wait_for_label("Vendor contract renewal");
    let field = fx
        .harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("Who owns the budget?");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Send message").click();
    let sent = fx.wait_for_request("chat_send", |_| true);
    assert_eq!(sent["text"], "Who owns the budget?");
    assert!(sent.get("chat").is_none(), "a new chat: {sent}");
    // The new chat follows its answer, which streams in and settles.
    fx.wait_for_request("subscribe", |r| r["chat"] == "13");
    fx.wait("the question in the chat", |h| {
        h.query_all_by_label("Who owns the budget?").count() >= 2
    });
    fx.wait_for_label("Stop response");
    fx.wait_for_label("Here's what I found in Drive and Slack");
    fx.wait_for_label("Want me to draft the next review's agenda?");
    fx.wait_for_label("Send message");
    fx.harness.get_by_label_contains("Searched Drive and Slack");
    assert!(!fx.chat().state().working());
}

#[test]
fn stop_cancels_the_answer_being_written() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Q3 budget review");
    fx.open(12);
    fx.wait_for_label("Q3 spending came in at €1.28M");
    // What's written so far can arrive before the chat's read, and the
    // activity with it.
    fx.wait_for_label("Searching Drive and Slack · Reading Q3 actuals.xlsx");
    fx.harness.get_by_label("Stop response").click();
    fx.wait_for_request("chat_cancel", |r| r["chat"] == "12");
    fx.wait_for_label("Send message");
}

#[test]
fn the_composer_says_why_it_is_locked_and_the_way_out() {
    let mut fx = fixture(Options {
        scenario: Scenario::Fresh,
        ..Options::default()
    });
    fx.wait_for_label("This computer isn't paired with Chat with Work.");
    let send = fx.harness.get_by_label("Send message");
    assert!(send.accesskit_node().is_disabled());
    fx.harness.get_by_label("Pair it").click();
    fx.harness.run_steps(2);
    assert_eq!(fx.harness.state().page(), Page::Account);
    assert!(
        !fx.server.requests().iter().any(|r| r["cmd"] == "chats"),
        "no chats asked for while unpaired"
    );
}

#[test]
fn a_tool_with_its_own_view_gets_a_place_for_it() {
    let mut fx = fixture(Options::default());
    // A project's chat isn't under Recent, as on the web: open it as its
    // pin or its link would.
    fx.wait_for_label("Vendor contract renewal");
    fx.chat().open(10);
    fx.harness.run_steps(2);
    fx.wait_for_label("Linear's view");
    fx.harness.get_by_label("Open in browser");
}

#[test]
fn a_settled_chat_asks_for_no_more_frames() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    // Chat 12's answer runs: its shimmer and dot keep the window drawing.
    assert!(fx.harness.try_run().is_err(), "a running chat animates");
    fx.server.finish(12, 123, "Done.");
    fx.wait("the list to show it settled", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.chats.iter().all(|c| !c.processing()))
    });
    fx.open(11);
    fx.wait_for_label("The Acme contract renews");
    // Each logo's fetch asks for one frame when it lands; on a slower
    // channel (Windows' pipe) they can still be on their way here.
    fx.wait("the logos", |h| {
        h.state().chat_ref().is_some_and(|c| c.logos_pending() == 0)
    });
    // Hovers and rises settle, then nothing more is asked for.
    fx.harness.try_run().expect("a settled page stops drawing");
}

impl Fixture {
    /// How many times the page has read the list so far.
    fn lists(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r["cmd"] == "chats")
            .count()
    }

    /// Wait until the server keeps the list current, and the catch-up
    /// read is in.
    fn live(&mut self) {
        self.wait_for_request("subscribe", |r| r["topics"] == serde_json::json!(["chats"]));
        self.wait("the list to be live", |h| {
            h.state().chat_state().is_some_and(|s| s.list_live)
        });
        self.harness.run_steps(4);
    }
}

#[test]
fn the_list_stays_current_as_chats_change_elsewhere() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.live();
    let listed = fx.lists();
    // Started on the web: it shows up, being answered, then settles.
    fx.server.start_elsewhere(
        30,
        "Draft the Q4 hiring plan",
        "Draft the Q4 hiring plan from the headcount sheet.",
    );
    fx.wait_for_label("Draft the Q4 hiring plan");
    assert!(
        fx.harness
            .state()
            .chat_state()
            .is_some_and(|s| s.list.chats[0].number == 30 && s.list.chats[0].processing()),
        "newest first, being answered"
    );
    fx.server.finish(30, 302, "Here's a first draft.");
    fx.wait("chat 30 to settle", |h| {
        h.state().chat_state().is_some_and(|s| {
            s.list
                .chats
                .iter()
                .any(|c| c.number == 30 && !c.processing())
        })
    });
    // Renamed elsewhere: the row says so.
    fx.server
        .edit_elsewhere(30, |c| c["title"] = serde_json::json!("Q4 hiring plan"));
    fx.wait("the new title", |h| {
        h.query_by_label("Q4 hiring plan").is_some()
    });
    assert!(
        fx.harness
            .query_by_label("Draft the Q4 hiring plan")
            .is_none()
    );
    // The open chat deleted elsewhere closes, saying so.
    fx.open(11);
    fx.wait_for_label("When does the Acme contract renew");
    fx.server.delete_elsewhere(11);
    fx.wait_for_label(super::state::GONE);
    assert!(
        fx.harness
            .query_by_label("Vendor contract renewal")
            .is_none()
    );
    assert_eq!(fx.chat().state().open, None);
    // None of it needed the list read again.
    assert_eq!(fx.lists(), listed, "no reads while the list is live");
}

#[test]
fn an_older_daemon_still_gets_the_list_read_again() {
    let mut fx = fixture(Options {
        old_daemon: true,
        ..Options::default()
    });
    fx.wait_for_label("Vendor contract renewal");
    fx.wait_for_request("subscribe", |r| r["topics"] == serde_json::json!(["chats"]));
    let listed = fx.lists();
    // Chat 12 is answered elsewhere: the list is read again until it's done.
    fx.server.finish(12, 123, "Done.");
    fx.wait("the list to show it settled", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.chats.iter().all(|c| !c.processing()))
    });
    assert!(fx.lists() > listed);
    assert!(!fx.chat().state().list_live);
}

#[test]
fn the_webs_logos_come_from_the_server_once_and_are_kept() {
    let mut fx = fixture(Options::default());
    fx.ready();
    // Nothing runs, so nothing but the logos could ask for frames.
    fx.server.finish(12, 123, "Done.");
    fx.open_waiting(11, "Want me to add it to the Q4 checklist?");
    fx.harness
        .get_by_label("Searched Drive and Slack · 3 searches, read 2 files")
        .click_accesskit();
    fx.logos();
    let asked: Vec<String> = fx
        .server
        .requests()
        .iter()
        .filter(|r| r["cmd"] == "asset")
        .map(|r| r["path"].as_str().unwrap_or_default().to_string())
        .collect();
    for path in [
        "/assets/providers/azure-6a1b2c3d.svg",
        "/assets/providers/drive-7a3c91e0.svg",
        "/assets/providers/slack-2b8d4f61.svg",
        "/assets/mimetypes/application-pdf-1e2d3c4b.svg",
        "/assets/mimetypes/x-office-spreadsheet-5a6b7c8d.svg",
    ] {
        assert_eq!(
            asked.iter().filter(|p| *p == path).count(),
            1,
            "{path} once: {asked:?}"
        );
    }
    let kept = fx
        .server
        .paths
        .daemon
        .data_dir
        .join("chat-assets/chatwithwork.com/providers__drive-7a3c91e0.svg");
    assert!(kept.is_file(), "kept at {}", kept.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&kept).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Drawn from memory from now on: nothing more is asked, nothing moves.
    let before = fx.server.requests().len();
    fx.harness.run_steps(4);
    assert_eq!(fx.server.requests().len(), before);
    fx.harness
        .try_run()
        .expect("loaded logos ask for no frames");
}

#[test]
fn the_sidebar_shows_whats_pinned_as_the_web_does() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Pinned");
    // The web shows the organization only to someone in more than one,
    // which the chat API doesn't say: none, as for someone in one.
    assert!(fx.harness.query_by_label("Northwind").is_none());
    for label in ["Recent", "Pinned", "All projects", "Falcon"] {
        fx.harness.get_by_label(label);
    }
    // A pinned chat stays in Recent too; the pin opens it.
    let pinned: Vec<_> = fx
        .harness
        .get_all_by_label("Weekly report for leadership")
        .collect();
    assert_eq!(pinned.len(), 2);
    pinned[1].click();
    fx.wait_for_request("chat", |r| r["chat"] == "4");
    // Chats opens the native page of every chat.
    fx.harness.get_by_label("Chats").click();
    fx.wait_for_label("Search your chats");
    assert!(OPENED.with(|o| o.borrow().is_empty()));
}

#[test]
fn pinned_projects_and_the_project_list_open_natively() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Falcon");
    fx.harness.get_by_label("Falcon").click();
    fx.wait_for_request("projects", |r| {
        r["request"]["action"] == "show" && r["request"]["project"] == 7
    });
    fx.wait_for_label("New chat here");
    fx.harness
        .query_all_by_label("All projects")
        .next()
        .unwrap()
        .click();
    fx.wait_for_label("Projects you're not on");
    assert!(fx.chat().state.projects.is_some());
    assert!(fx.chat().state.open.is_none());
    OPENED.with(|o| assert!(o.borrow().is_empty()));
}

#[test]
fn a_pinned_project_is_current_while_one_of_its_chats_is_open() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Falcon");
    // The pin, not the project's line over the open chat, which links too.
    let current = |fx: &Fixture| {
        let pin = fx
            .harness
            .get_all_by_label("Falcon")
            .find(|n| n.accesskit_node().role() != egui::accesskit::Role::Link)
            .expect("the pin");
        let node = pin.accesskit_node();
        node.is_selected() == Some(true) || node.toggled() == Some(egui::accesskit::Toggled::True)
    };
    assert!(!current(&fx));
    fx.chat().open(10);
    fx.harness.run_steps(2);
    assert!(current(&fx), "a chat in Falcon is open");
}

#[test]
fn your_name_opens_native_account_settings_and_this_computer_its_own() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    fx.ready();
    // As on the web, your name links to Settings: no menu.
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    assert!(OPENED.with(|o| o.borrow().is_empty()));
    assert!(fx.chat().state.account_settings.is_some());
    assert_eq!(fx.harness.state().page(), Page::Chat);
    // This computer's settings are one link of their own.
    fx.harness.get_by_label("This computer").click();
    fx.harness.run_steps(2);
    assert_eq!(fx.harness.state().page(), Page::Folders);
}

#[test]
fn the_logotype_starts_a_new_chat_as_on_the_web() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.open(11);
    fx.wait("the chat", |h| {
        h.state().chat_state().is_some_and(|s| s.open == Some(11))
    });
    fx.harness.get_by_label("Chat with Work").click();
    fx.harness.run_steps(2);
    assert_eq!(fx.harness.state().chat_state().unwrap().open, None);
}

#[test]
fn credits_and_projects_change_in_place() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.live();
    assert!(fx.harness.query_by_label_contains("credits left").is_none());
    // Running low: the meter shows under the name, as on the web.
    fx.server.spend(410);
    fx.wait_for_label("410 of 5,000 credits left");
    // A project joined elsewhere is in the list.
    let mut projects = crate::demo::projects();
    projects.push(crate::demo::project(9, "Board", "folder-simple"));
    fx.server.set_projects(projects);
    fx.wait("the new project", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.projects.iter().any(|p| p.id == 9))
    });
}

#[test]
fn a_question_sent_is_pinned_near_the_top_with_its_answer_under_it() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.open_waiting(11, "Want me to add it to the Q4 checklist?");
    // The conversation keeps its padding below the integrated window header.
    let pinned_top = if fx.harness.state().custom_titlebar {
        crate::ui::chrome::HEIGHT
    } else {
        0.0
    } + 34.0;
    let top = fx
        .harness
        .get_by_label("Draft a short reminder for the team.")
        .rect()
        .top();
    assert!((top - pinned_top).abs() < 4.0, "pinned at {top}");
    let field = fx
        .harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("Who signs the renewal?");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Send message").click();
    fx.wait_for_request("chat_send", |_| true);
    // The new question scrolls up to the same place, smoothly, with room
    // under it for the answer to come.
    fx.wait("the question pinned", |h| {
        h.query_all_by_label("Who signs the renewal?")
            .last()
            .is_some_and(|q| (q.rect().top() - pinned_top).abs() < 4.0)
    });
    assert!(
        fx.harness
            .query_by_label("Draft a short reminder for the team.")
            .is_none_or(|q| q.rect().bottom() < pinned_top),
        "the earlier one scrolled up"
    );
}

#[test]
fn a_narrow_window_keeps_the_sidebar_in_a_drawer() {
    let mut fx = fixture(Options {
        size: [760.0, 820.0],
        ..Options::default()
    });
    fx.wait("the chats", |h| {
        h.state().chat_state().is_some_and(|s| s.ready())
    });
    assert!(
        fx.harness
            .query_by_label("Vendor contract renewal")
            .is_none()
    );
    fx.harness.get_by_label("Open sidebar").click();
    fx.wait_for_label("Vendor contract renewal");
    fx.harness.get_by_label("Vendor contract renewal").click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    // Choosing a chat closes the drawer.
    fx.wait("the drawer to close", |h| {
        h.query_by_label("Close sidebar").is_none()
    });
}

/// The page rendered offscreen, light and dark, wide and narrow, against
/// the snapshots in `app/tests/snapshots` (compared on Linux, where they
/// were made; `UPDATE_SNAPSHOTS=1` writes them). Skipped where wgpu finds
/// no adapter at all.
#[test]
fn renders_like_the_web() {
    let screens: [(&str, Option<u64>, [f32; 2]); 7] = [
        ("chat", Some(11), [1280.0, 820.0]),
        ("chat-narrow", Some(11), [760.0, 820.0]),
        ("chat-streaming", Some(12), [1280.0, 820.0]),
        ("chat-new", None, [1280.0, 820.0]),
        ("chat-new-narrow", None, [760.0, 820.0]),
        ("chat-approval", Some(5), [1280.0, 820.0]),
        ("chat-question", Some(4), [1280.0, 820.0]),
    ];
    for dark in [false, true] {
        for (name, chat, size) in screens {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                eprintln!("no wgpu adapter here: skipping the snapshots");
                return;
            };
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            if let Some(number) = chat {
                fx.chat().open(number);
                fx.wait("the transcript", |h| {
                    h.state()
                        .chat_state()
                        .is_some_and(|s| s.transcript.is_some())
                });
            }
            if chat == Some(12) {
                fx.wait_for_label("Q3 spending came in at €1.28M");
                fx.harness
                    .get_by_label("Searching Drive and Slack · Reading Q3 actuals.xlsx")
                    .click();
            }
            fx.logos();
            // kittest paints a pointer wherever one hovers: take it away.
            fx.harness.event(egui::Event::PointerGone);
            // Let fonts and textures settle.
            fx.harness.run_steps(12);
            let theme = if dark { "dark" } else { "light" };
            let file = format!("{name}-{theme}");
            let image = fx.harness.render().expect("rendering");
            let options = egui_kittest::SnapshotOptions::new()
                .threshold(2.0)
                .max_failed_pixels(((size[0] * size[1]) * 0.01) as usize);
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(&image, &file, &options);
            } else {
                // Other platforms render text their own way: render only.
                assert!(image.width() > 0);
            }
        }
    }
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

impl Fixture {
    /// Draw a few frames, so the page asks for the images it shows, and
    /// wait until they're all in.
    fn logos(&mut self) {
        self.harness.run_steps(2);
        self.wait("the logos", |h| {
            h.state().chat_ref().is_some_and(|c| c.logos_pending() == 0)
        });
    }

    /// Wait until the open chat is followed and its catch-up read is in.
    fn caught_up(&mut self) {
        self.wait("the chat to catch up", |h| {
            h.state().chat_state().is_some_and(|s| {
                s.following == super::state::Following::Live && !s.loading && !s.stale
            })
        });
    }

    /// Wait for the chats and the models.
    fn ready(&mut self) {
        self.wait("the models", |h| {
            h.state()
                .chat_state()
                .is_some_and(|s| matches!(s.models, super::state::ModelList::Ready(_)))
        });
    }

    fn open_waiting(&mut self, number: u64, label: &str) {
        self.ready();
        self.chat().open(number);
        self.wait_for_label(label);
        self.harness.run_steps(4);
    }
}

#[test]
fn a_model_picked_goes_with_the_question() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Model: GPT-6 Luna");
    fx.harness.get_by_label("Model: GPT-6 Luna").click();
    fx.wait_for_label("Mistral Large 3");
    // One that can't be asked now says why, and stays unpicked.
    fx.harness.get_by_label("GPT-6.1 Sol").click();
    fx.wait_for_label("You're out of credits. Add credits in Settings to keep going.");
    fx.harness.get_by_label("Model: GPT-6 Luna").click();
    fx.wait_for_label("Gemini 4 Argon");
    fx.harness.get_by_label("Gemini 4 Argon").click();
    fx.wait_for_label("Model: Gemini 4 Argon");
    let field = fx
        .harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("Summarize the Acme contract.");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Send message").click();
    let sent = fx.wait_for_request("chat_send", |_| true);
    assert_eq!(sent["model"], "14");
}

#[test]
fn files_attach_from_the_picker_and_by_dropping() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("Q3 plan.pdf");
    std::fs::write(&pdf, b"%PDF-1.7 synthetic").unwrap();
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, b"Synthetic notes").unwrap();
    let svg = dir.path().join("logo.svg");
    std::fs::write(&svg, b"<svg/>").unwrap();
    let mut fx = fixture(Options::default());
    fx.ready();
    PICKED.with(|p| *p.borrow_mut() = Some(vec![pdf]));
    fx.harness.get_by_label("Attach files").click();
    let upload = fx.wait_for_request("chat_upload", |_| true);
    assert_eq!(upload["filename"], "Q3 plan.pdf");
    assert_eq!(upload["content_type"], "application/pdf");
    fx.wait_for_label("Q3 plan.pdf, 18.00 B");
    // Dropped on the window: the composer takes the text file, and says
    // it can't take the SVG, as the web does.
    fx.harness.input_mut().dropped_files = vec![
        std::sync::Arc::new(Dropped(notes)),
        std::sync::Arc::new(Dropped(svg)),
    ];
    fx.harness.run_steps(2);
    fx.wait_for_label("That file can't be attached");
    fx.harness
        .get_by_label_contains("isn't a supported file type");
    fx.harness.get_by_label("OK").click();
    fx.wait_for_request("chat_upload", |r| r["filename"] == "notes.txt");
    fx.wait_for_label("notes.txt, 15.00 B");
    // Taken off again.
    fx.harness.get_by_label("Remove notes.txt").click();
    fx.harness.run_steps(2);
    assert!(fx.harness.query_by_label("notes.txt, 15.00 B").is_none());
    // Files alone can be sent.
    fx.harness.get_by_label("Send message").click();
    let sent = fx.wait_for_request("chat_send", |_| true);
    assert_eq!(sent["attachments"], serde_json::json!(["demo-upload-1"]));
    // The question shows its file.
    fx.wait_for_label("Q3 plan.pdf, 18 Bytes");
}

#[test]
fn answers_offer_what_the_chat_allows() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Vendor contract renewal");
    fx.open(11);
    fx.wait_for_label("Want me to add it to the Q4 checklist?");
    fx.harness.run_steps(5);
    // The latest answer keeps its actions in view.
    for label in [
        "Retry this answer",
        "Branch into a new chat",
        "Share a public link to this chat",
    ] {
        fx.harness.get_by_label(label);
    }
    // Retry asks first.
    fx.harness.get_by_label("Retry this answer").click();
    fx.wait_for_label("Retry this answer?");
    fx.harness.get_by_label("Retry").click();
    let retried = fx.wait_for_request("chat_retry", |_| true);
    assert_eq!(
        (retried["chat"].as_str(), retried["message"].as_str()),
        (Some("11"), Some("115"))
    );
    // The server records the request before the UI receives its reply.
    // Finish the retry before testing a second action in another chat.
    fx.wait("the retry to finish", |h| {
        h.state().chat_state().is_some_and(|s| s.acting.is_none())
    });
    // A project chat started by someone else: no retry, and no share.
    fx.chat().open(6);
    fx.wait_for_label("Interviews for the third role finish on Thursday.");
    // The read that catches up once the chat is followed can land under
    // a click and move the buttons: let it in first.
    fx.caught_up();
    fx.harness.run_steps(5);
    fx.harness.get_by_label("Branch into a new chat");
    assert!(fx.harness.query_by_label("Retry this answer").is_none());
    assert!(
        fx.harness
            .query_by_label("Share a public link to this chat")
            .is_none()
    );
    // Branching opens the new chat.
    fx.harness.get_by_label("Branch into a new chat").click();
    let branched = fx.wait_for_request("chat_branch", |_| true);
    assert_eq!(branched["message"], "62");
    fx.wait_for_label("Branch of Hiring pipeline status");
    fx.wait("the branch to open", |h| {
        h.state().chat_state().is_some_and(|s| s.open == Some(13))
    });
}

#[test]
fn a_chat_is_shared_and_stops_being_shared_from_its_dialog() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.chat().set_still(true);
    fx.open_waiting(11, "Want me to add it to the Q4 checklist?");
    fx.caught_up();
    // Loaded logos and the initial scroll can move the answer's footer.
    // Click its final position, including on slower macOS/Nix runners.
    fx.logos();
    fx.harness.run_steps(4);
    fx.harness
        .get_by_label("Share a public link to this chat")
        .click();
    fx.wait_for_label("Public");
    fx.harness
        .get_by_label("https://chatwithwork.com/shared/9aXk2pQ7mN4rT8vW1yZ3bC5d");
    fx.harness.get_by_label("Stop sharing").click();
    fx.wait_for_request("chat_unshare", |r| r["chat"] == "11");
    fx.wait_for_label("Private");
    fx.harness.get_by_label("Create public link").click();
    fx.wait_for_request("chat_share", |r| r["chat"] == "11");
    // The link is copied as it's made.
    fx.wait_for_label("Copied");
    fx.harness.get_by_label("Done").click();
    fx.harness.run_steps(2);
    assert!(fx.harness.query_by_label("Share this chat").is_none());
}

#[test]
fn a_change_is_approved_for_the_rest_of_the_chat() {
    let mut fx = fixture(Options::default());
    fx.open_waiting(5, "Waiting for your approval.");
    fx.harness
        .get_by_label_contains("Needs your approval · Slack: Post a message to #launch");
    fx.harness.get_by_label("Message: Falcon is live! Thanks, everyone: the release notes are in Drive, and the status page is up.");
    fx.harness
        .get_by_label("Allow this in Slack for the rest of this chat")
        .click();
    fx.harness.run_steps(1);
    fx.harness.get_by_label("Approve").click();
    let approved = fx.wait_for_request("chat_approve", |_| true);
    assert_eq!(approved["tool_call"], "531");
    assert_eq!(approved["for_rest_of_chat"], true);
    // The card goes once nothing waits, and the answer carries on.
    fx.wait("the card to go", |h| h.query_by_label("Approve").is_none());
    assert!(
        fx.harness
            .query_by_label("Waiting for your approval.")
            .is_none()
    );
}

#[test]
fn a_change_is_denied_with_a_reason() {
    let mut fx = fixture(Options::default());
    fx.open_waiting(5, "Waiting for your approval.");
    fx.harness.get_by_label("Deny with a reason").click();
    fx.harness.run_steps(2);
    let field = fx.harness.get_by_label("Why you're denying it");
    field.focus();
    field.type_text("Wait for the status page");
    fx.harness.run_steps(1);
    fx.harness.get_by_label("Deny").click();
    let denied = fx.wait_for_request("chat_deny", |_| true);
    assert_eq!(denied["reason"], "Wait for the status page");
}

#[test]
fn a_tools_question_is_answered_with_its_form() {
    let mut fx = fixture(Options::default());
    fx.open_waiting(4, "Waiting for your answer.");
    fx.harness
        .get_by_label_contains("Only Notion sees your answer.");
    fx.harness.get_by_label("Environment").click();
    fx.wait_for_label("production");
    fx.harness.get_by_label("production").click();
    fx.harness.run_steps(1);
    fx.harness.get_by_label("Hiring").click();
    fx.harness.get_by_label("Include drafts").click();
    let days = fx.harness.get_by_label("Days (optional)");
    days.focus();
    days.type_text("0");
    fx.harness.run_steps(1);
    // The card's foot is below the fold, under the question pinned at top.
    fx.harness.get_by_label("Send").click_accesskit();
    let answered = fx.wait_for_request("chat_answer", |_| true);
    assert_eq!(answered["tool_call"], "432");
    assert_eq!(
        answered["input"],
        serde_json::json!({
            "environment": "production",
            "days": "70",
            "sections": ["Revenue", "Hiring"],
            "include_drafts": true,
        })
    );
}

#[test]
fn a_page_to_visit_opens_in_the_browser_then_is_answered() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    fx.open_waiting(3, "Waiting for your answer.");
    fx.harness
        .get_by_label("It asks you to open a page on linear.app, then come back.");
    fx.harness.get_by_label("Open linear.app").click();
    fx.harness.run_steps(2);
    assert_eq!(
        OPENED.with(|o| o.borrow().clone()),
        ["https://linear.app/oauth/authorize?client_id=northwind-demo"]
    );
    fx.harness.get_by_label("I've done it").click();
    let answered = fx.wait_for_request("chat_answer", |_| true);
    assert!(answered.get("input").is_none(), "{answered}");
}

#[test]
fn a_question_can_be_declined() {
    let mut fx = fixture(Options::default());
    fx.open_waiting(3, "Waiting for your answer.");
    fx.harness.get_by_label("Decline").click();
    let declined = fx.wait_for_request("chat_decline", |_| true);
    assert_eq!(declined["tool_call"], "332");
}

#[test]
fn chats_are_renamed_and_deleted_from_their_row_menu() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.wait_for_label("Vendor contract renewal");
    // Chat 11's row is the list's second.
    fx.harness
        .get_all_by_label("Chat options")
        .nth(1)
        .unwrap()
        .click();
    fx.wait_for_label("Rename");
    fx.harness.get_by_label("Rename").click();
    fx.harness.run_steps(2);
    let field = fx.harness.get_by_label("Chat title");
    field.focus();
    field.type_text(" 2027");
    fx.harness.run_steps(1);
    fx.harness.get_by_label("Save title").click();
    let renamed = fx.wait_for_request("chat_rename", |_| true);
    assert_eq!(renamed["title"], "Vendor contract renewal 2027");
    fx.wait_for_label("Vendor contract renewal 2027");
    fx.harness
        .get_all_by_label("Chat options")
        .nth(1)
        .unwrap()
        .click();
    fx.wait_for_label("Delete");
    fx.harness.get_by_label("Delete").click();
    fx.wait_for_label("Are you sure?");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Delete").click();
    fx.wait_for_request("chat_delete", |r| r["chat"] == "11");
    fx.wait("the chat to go", |h| {
        h.query_by_label("Vendor contract renewal 2027").is_none()
    });
}

#[test]
fn answer_text_is_selected_with_the_mouse_and_copied() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.open_waiting(11, "Want me to add it to the Q4 checklist?");
    let rect = fx
        .harness
        .get_by_label("Want me to add it to the Q4 checklist?")
        .rect();
    let (from, to) = (
        rect.left_center() + egui::vec2(1.0, 0.0),
        rect.right_center() - egui::vec2(1.0, 0.0),
    );
    fx.harness.event(egui::Event::PointerMoved(from));
    fx.harness.step();
    fx.harness.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.step();
    fx.harness.event(egui::Event::PointerMoved(to));
    fx.harness.step();
    fx.harness.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.step();
    fx.harness.event(egui::Event::Copy);
    fx.harness.step();
    let copied: Vec<String> = fx
        .harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(copied, ["Want me to add it to the Q4 checklist?"]);
}

/// One screen for the review images: what to open and set up.
struct Screen {
    name: &'static str,
    chat: Option<u64>,
    setup: fn(&mut Fixture),
}

fn review_screens() -> Vec<Screen> {
    fn nothing(_: &mut Fixture) {}
    vec![
        Screen {
            name: "picker",
            chat: None,
            setup: |fx| {
                fx.wait_for_label("Model: GPT-6 Luna");
                fx.harness.get_by_label("Model: GPT-6 Luna").click();
            },
        },
        Screen {
            name: "files",
            chat: None,
            setup: |fx| {
                let state = fx.chat().state_mut();
                state.input = "What changed between these two?".into();
                state.attachments = vec![
                    super::state::Attached {
                        key: 900,
                        path: "/tmp/Q3 plan.pdf".into(),
                        name: "Q3 plan.pdf".into(),
                        size: 48_210,
                        content_type: "application/pdf".into(),
                        signed_id: Some("demo".into()),
                        icon: None,
                    },
                    super::state::Attached {
                        key: 901,
                        path: "/tmp/Vendor list.xlsx".into(),
                        name: "Vendor list.xlsx".into(),
                        size: 18_432,
                        content_type:
                            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                                .into(),
                        signed_id: None,
                        icon: None,
                    },
                ];
            },
        },
        Screen {
            name: "chat",
            chat: Some(11),
            setup: nothing,
        },
        Screen {
            name: "approval",
            chat: Some(5),
            setup: nothing,
        },
        Screen {
            name: "steps",
            chat: Some(11),
            setup: |fx| {
                fx.harness
                    .get_by_label("Searched Drive and Slack · 3 searches, read 2 files")
                    .click_accesskit();
            },
        },
        Screen {
            name: "computer",
            chat: Some(8),
            setup: |fx| {
                fx.harness
                    .get_by_label("Searched Alex's MacBook · 1 search, read 1 file")
                    .click_accesskit();
            },
        },
        Screen {
            name: "plan",
            chat: Some(7),
            setup: |fx| {
                fx.harness
                    .get_by_label("Made a plan · 2 steps")
                    .click_accesskit();
            },
        },
        Screen {
            name: "sent",
            chat: Some(11),
            setup: |fx| {
                fx.chat().state_mut().input = "Who signs the renewal?".into();
                fx.harness.run_steps(2);
                fx.harness.get_by_label("Send message").click();
                fx.wait_for_request("chat_send", |_| true);
                fx.harness.run_steps(20);
            },
        },
        Screen {
            name: "notice",
            chat: Some(9),
            setup: |fx| {
                fx.chat().state_mut().notice =
                    Some("An answer is being written in this chat.".into())
            },
        },
        Screen {
            name: "account",
            chat: Some(11),
            setup: nothing,
        },
        Screen {
            name: "project-chat",
            chat: Some(10),
            setup: nothing,
        },
        Screen {
            name: "credits",
            chat: Some(11),
            setup: |fx| {
                fx.server.spend(410);
                fx.wait("the meter", |h| {
                    h.query_by_label("410 of 5,000 credits left").is_some()
                });
            },
        },
        Screen {
            name: "sources",
            chat: Some(11),
            setup: |fx| fx.chat().conversation_mut().sources_open = Some(113),
        },
        Screen {
            name: "question",
            chat: Some(4),
            setup: nothing,
        },
        Screen {
            name: "url",
            chat: Some(3),
            setup: nothing,
        },
        Screen {
            name: "menu",
            chat: Some(11),
            setup: |fx| fx.chat().state_mut().menu = Some(11),
        },
        Screen {
            name: "rename",
            chat: Some(11),
            setup: |fx| {
                fx.chat().state_mut().renaming = Some((11, "Vendor contract renewal".into()))
            },
        },
        Screen {
            name: "share",
            chat: Some(11),
            setup: |fx| {
                fx.chat().state_mut().dialog = Some(super::state::Dialog::Share {
                    chat: 11,
                    copied: false,
                })
            },
        },
        Screen {
            name: "share-private",
            chat: Some(5),
            setup: |fx| {
                fx.chat().state_mut().dialog = Some(super::state::Dialog::Share {
                    chat: 5,
                    copied: false,
                })
            },
        },
        Screen {
            name: "retry",
            chat: Some(11),
            setup: |fx| {
                fx.chat().state_mut().dialog = Some(super::state::Dialog::Retry {
                    chat: 11,
                    message: 115,
                    user: false,
                })
            },
        },
        Screen {
            name: "delete",
            chat: Some(11),
            setup: |fx| {
                fx.chat().state_mut().dialog = Some(super::state::Dialog::Delete { chat: 11 })
            },
        },
        Screen {
            name: "unsupported",
            chat: None,
            setup: |fx| {
                fx.chat().state_mut().dialog = Some(super::state::Dialog::Unsupported {
                    name: "logo.svg".into(),
                })
            },
        },
    ]
}

/// Renders the new controls offscreen for review beside the web's
/// references, into `$CWW_PARITY_DIR` (skipped without it, or without a
/// wgpu adapter).
#[test]
fn renders_review_screens() {
    let Some(dir) = std::env::var_os("CWW_PARITY_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let only = std::env::var("CWW_PARITY_ONLY").ok();
    std::fs::create_dir_all(&dir).unwrap();
    for dark in [false, true] {
        for screen in review_screens() {
            if only
                .as_deref()
                .is_some_and(|o| !o.split(',').any(|n| n == screen.name))
            {
                continue;
            }
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                eprintln!("no wgpu adapter here");
                return;
            };
            let mut fx = fixture(Options {
                dark,
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait("the chats", |h| {
                h.state().chat_state().is_some_and(|s| s.ready())
            });
            fx.wait("the models", |h| {
                h.state()
                    .chat_state()
                    .is_some_and(|s| matches!(s.models, super::state::ModelList::Ready(_)))
            });
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            if let Some(number) = screen.chat {
                fx.chat().open(number);
                fx.wait("the transcript", |h| {
                    h.state()
                        .chat_state()
                        .is_some_and(|s| s.transcript.is_some())
                });
            }
            fx.harness.run_steps(4);
            (screen.setup)(&mut fx);
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            // Your name, hovered, as the web's link shows it.
            if screen.name == "account" {
                fx.harness.get_by_label("Settings and usage").hover();
            }
            fx.harness.run_steps(12);
            let image = fx.harness.render().expect("rendering");
            let theme = if dark { "dark" } else { "light" };
            image
                .save(dir.join(format!("egui-{}-{theme}.png", screen.name)))
                .unwrap();
        }
    }
}

#[test]
fn copying_a_message_says_copied_then_turns_back() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.open(11);
    fx.wait_for_label("The Acme contract renews");
    // The latest answer's actions sit below the fold of the test window.
    fx.harness
        .query_all_by_label("Copy message")
        .last()
        .expect("a Copy button")
        .click_accesskit();
    fx.wait_for_label("Copied");
    // The harness steps a quarter of a second at a time.
    fx.harness.run_steps(10);
    assert!(fx.harness.query_all_by_label("Copied").next().is_none());
    fx.harness.get_by_label("Copy message");
}

#[test]
fn web_search_explains_cost_and_privacy_before_sending() {
    let mut fx = fixture(Options {
        web_search: true,
        ..Options::default()
    });
    fx.wait_for_label("Search the web");
    fx.harness.get_by_label("Search the web").click();
    fx.harness.run_steps(2);
    fx.harness.get_by_label_contains("outside the EU.");
    fx.harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput)
        .focus();
    fx.harness
        .event(egui::Event::Text("Find the announcement".into()));
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Send message").click();
    let sent = fx.wait_for_request("chat_send", |_| true);
    assert_eq!(sent["web_search"], true);
}

#[test]
fn dictation_checks_the_server_before_opening_a_microphone() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.chat().state.input = "Keep this draft".into();
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Dictate").click();
    fx.wait_for_request("dictation", |_| true);
    fx.wait_for_label("Dictation is off in the demo.");
    assert_eq!(fx.chat().state.input, "Keep this draft");
    assert_eq!(
        fx.chat().state.dictation.phase,
        cww::tui::dictation::Phase::Idle
    );
    assert!(!fx.server.requests().iter().any(|r| r["cmd"] == "dictate"));
}

#[test]
fn renders_dictation_controls_without_recording_audio() {
    for dark in [false, true] {
        for (name, size) in [
            ("chat-dictation", [1280.0, 820.0]),
            ("chat-dictation-narrow", [760.0, 820.0]),
        ] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                return;
            };
            let mut fx = fixture(Options {
                size,
                dark,
                web_search: true,
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait_for_label("Search the web");
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            {
                let state = &mut fx.chat().state;
                let generation = state.dictation.start();
                state.update(super::state::Msg::Dictation(
                    generation,
                    cww::tui::dictation::Update::Recording,
                ));
                for index in 0..24 {
                    state.update(super::state::Msg::Dictation(
                        generation,
                        cww::tui::dictation::Update::Level {
                            elapsed_ms: 12000 + index * 70,
                            level: ((index * 157 + 71) % 1000) as u16,
                        },
                    ));
                }
                state.input = "Summarize the meeting".into();
            }
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            fx.harness.get_by_label_contains("Listening 0:13");
            fx.harness.get_by_label("Stop dictating");
            let image = fx.harness.render().expect("GPU rendering");
            let theme = if dark { "dark" } else { "light" };
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(
                    &image,
                    format!("{name}-{theme}"),
                    &egui_kittest::SnapshotOptions::new()
                        .threshold(2.0)
                        .max_failed_pixels(1000),
                );
            }
        }
    }
}

#[test]
fn dictation_replaces_the_selection_and_remains_undoable() {
    use cww::tui::dictation::Update;
    use egui::text::{CCursor, CCursorRange};
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.chat().state.input = "Hello 🌍 friend".into();
    fx.harness.run_steps(2);
    let id = egui::Id::new(super::composer::ID);
    let ctx = fx.harness.ctx.clone();
    let mut editor = egui::text_edit::TextEditState::load(&ctx, id).unwrap();
    editor
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(6), CCursor::new(7))));
    editor.store(&ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
    let generation = fx.chat().state.dictation.start();
    fx.chat().update_state(
        &ctx,
        super::state::Msg::Dictation(generation, Update::Recording),
    );
    fx.chat().update_state(
        &ctx,
        super::state::Msg::Dictation(generation, Update::Finished(Ok("world".into()))),
    );
    fx.harness.run_steps(2);
    assert_eq!(fx.chat().state.input, "Hello world friend");
    let editor = egui::text_edit::TextEditState::load(&ctx, id).unwrap();
    assert_eq!(editor.cursor.char_range().unwrap().primary.index.0, 11);
    fx.harness.event(egui::Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            mac_cmd: cfg!(target_os = "macos"),
            command: true,
            ..Default::default()
        },
    });
    fx.harness.run_steps(2);
    assert_eq!(fx.chat().state.input, "Hello 🌍 friend");
}

#[test]
fn dictation_second_stop_click_keeps_the_transcription_and_escape_discards_it() {
    use cww::tui::dictation::{Phase, Update};
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.chat().state.input = "Keep this draft".into();
    let generation = fx.chat().state.dictation.start();
    fx.chat()
        .state
        .update(super::state::Msg::Dictation(generation, Update::Recording));
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Stop dictating").click();
    fx.harness.run_steps(2);
    assert_eq!(fx.chat().state.dictation.phase, Phase::Transcribing);
    fx.harness.get_by_label("Transcribing").click();
    fx.harness.run_steps(2);
    assert_eq!(fx.chat().state.dictation.phase, Phase::Transcribing);
    fx.harness.event(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(2);
    assert_eq!(fx.chat().state.dictation.phase, Phase::Idle);
    let ctx = fx.harness.ctx.clone();
    fx.chat().update_state(
        &ctx,
        super::state::Msg::Dictation(generation, Update::Finished(Ok("too late".into()))),
    );
    assert_eq!(fx.chat().state.input, "Keep this draft");
}

#[test]
fn renders_inline_links_and_citations() {
    for dark in [false, true] {
        for (name, size) in [
            ("chat-inline-links", [1280.0, 820.0]),
            ("chat-inline-links-narrow", [760.0, 820.0]),
        ] {
            let renderer = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new).ok();
            let can_render = renderer.is_some();
            let mut fx = fixture(Options {
                size,
                dark,
                renderer,
                ..Options::default()
            });
            fx.ready();
            fx.chat().open(11);
            fx.wait_for_label("When does the Acme contract renew");
            fx.caught_up();
            fx.logos();
            fx.chat().set_still(true);
            fx.chat().state.transcript.as_mut().unwrap().entries = vec![cww::tui::chat::Entry::Assistant {
                id: 9001,
                content: "The rollout is recorded [here](https://example.com/rollout). The alert rule in `acme/project` creates a new issue with the `bug` label ([GitHub issue](https://example.com/issue), [Sentry alert rule](https://example.com/rule)). These links should sit naturally within the surrounding sentence.\n\nA second paragraph has a [longer ordinary link that wraps across lines on a narrow screen](https://example.com/guide) and a final [source](https://example.com/issue).".into(),
                sources: vec![
                    cww::tui::chat::Source { title: "GitHub issue".into(), url: Some("https://example.com/issue".into()), ..Default::default() },
                    cww::tui::chat::Source { title: "Sentry alert rule".into(), url: Some("https://example.com/rule".into()), ..Default::default() },
                ],
                answered_with: None,
            }];
            fx.chat().pin = 3;
            fx.chat().pinned = None;
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(15);
            // The hosted runners have no GPU. Cursor and click checks still
            // run there; visual comparisons run where an adapter is available.
            if can_render {
                let image = fx.harness.render().expect("GPU rendering");
                let theme = if dark { "dark" } else { "light" };
                if let Some(directory) = std::env::var_os("CWW_INLINE_LINK_EVIDENCE") {
                    image
                        .save(std::path::Path::new(&directory).join(format!("{name}-{theme}.png")))
                        .unwrap();
                }
                if cfg!(target_os = "linux") {
                    egui_kittest::image_snapshot_options(
                        &image,
                        format!("{name}-{theme}"),
                        &egui_kittest::SnapshotOptions::new()
                            .threshold(2.0)
                            .max_failed_pixels(1000),
                    );
                }
            }
            OPENED.with(|opened| opened.borrow_mut().clear());
            for (label, url) in [
                ("here", "https://example.com/rollout"),
                ("Source 2: Sentry alert rule", "https://example.com/rule"),
            ] {
                fx.harness.get_by_label(label).hover();
                fx.harness.run_steps(2);
                assert_eq!(
                    fx.harness.output().platform_output.cursor_icon,
                    egui::CursorIcon::PointingHand,
                    "hovering {label} should identify a clickable link"
                );
                fx.harness.get_by_label(label).click();
                fx.harness.run_steps(2);
                assert_eq!(
                    OPENED.with(|opened| opened.borrow().last().cloned()),
                    Some(url.into())
                );
            }
        }
    }
}

#[test]
fn renders_web_search_and_schedule_context() {
    for dark in [false, true] {
        for (name, size) in [
            ("chat-web", [1280.0, 820.0]),
            ("chat-web-narrow", [760.0, 820.0]),
        ] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                eprintln!("no wgpu adapter here: skipping the snapshots");
                return;
            };
            let mut fx = fixture(Options {
                size,
                dark,
                web_search: true,
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait_for_label("Search the web");
            fx.chat().set_still(true);
            fx.chat().open(11);
            fx.wait("the transcript", |h| {
                h.state()
                    .chat_state()
                    .is_some_and(|s| s.transcript.is_some())
            });
            fx.caught_up();
            {
                let state = &mut fx.chat().state;
                state.web_search = Some(true);
                state.transcript.as_mut().unwrap().chat.schedule = Some(cww::tui::chat::Schedule {
                    mine: true,
                    name: "Morning briefing".into(),
                    timing: "Every weekday at 8:30".into(),
                    url: Some("https://chatwithwork.com/settings/schedules/1".into()),
                    owner: None,
                });
            }
            fx.logos();
            // A short scheduled answer keeps its context in the viewport.
            fx.chat().state.transcript.as_mut().unwrap().entries = vec![cww::tui::chat::Entry::Assistant {
                id: 9001, content: "Your morning briefing is ready. The Acme contract renews on March 1; Dana owns the review.".into(), sources: vec![], answered_with: None,
            }];
            fx.chat().pin = 3;
            fx.chat().pinned = None;
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(15);
            fx.harness
                .get_by_label("Morning briefing · Every weekday at 8:30");
            let image = fx.harness.render().expect("GPU rendering");
            let theme = if dark { "dark" } else { "light" };
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(
                    &image,
                    format!("{name}-{theme}"),
                    &egui_kittest::SnapshotOptions::new()
                        .threshold(2.0)
                        .max_failed_pixels(1000),
                );
            }
        }
    }
}

fn show_code_run(fx: &mut Fixture) {
    fx.ready();
    fx.chat().open(11);
    fx.wait_for_label("When does the Acme contract renew");
    fx.caught_up();
    fx.logos();
    fx.chat().set_still(true);
    let entry = serde_json::from_value(serde_json::json!({
        "kind":"activity", "id":9801, "title":"Ran code", "services":["Code"], "logos":[null], "glyphs":["terminal-window"],
        "steps":[{"summary":"Ran Python", "glyph":"terminal-window", "code":{"language":"python", "source":"total = 42000\nprint(total)", "stdout":"42000", "stderr":null, "exit_code":0, "error":null},
            "downloads":[{"filename":"budget.csv", "byte_size":27, "content_type":"text/csv", "path":"/rails/active_storage/blobs/proxy/signed-token/budget.csv?disposition=attachment"}]}]
    })).unwrap();
    fx.chat().state.transcript.as_mut().unwrap().entries = vec![entry];
    fx.chat().pin = 3;
    fx.chat().pinned = None;
    fx.harness.run_steps(10);
}

/// What happened at each step, as the web's rows say it: a plan, a change
/// denied with its reason, one a retry found done with its link, one that
/// failed, an answer with someone else's sources, and a failure with its
/// way out.
fn show_outcomes(fx: &mut Fixture) {
    fx.ready();
    fx.chat().open(11);
    fx.wait_for_label("When does the Acme contract renew");
    fx.caught_up();
    fx.logos();
    fx.chat().set_still(true);
    let entries = serde_json::from_value(serde_json::json!([
        {"kind":"activity", "id":9811, "title":"Planned and posted to Slack", "services":["Slack"], "logos":[null], "glyphs":["plugs-connected"],
         "steps":[
            {"summary":"Updated the plan", "glyph":"list-checks", "plan":{"headline":"Updated the plan", "items":[
                {"content":"Find the Q3 numbers", "state":"done"},
                {"content":"Draft the summary for the launch channel", "state":"pending"},
                {"content":"Ask finance", "state":"removed"}]}},
            {"summary":"Denied: Post to #general", "glyph":"prohibit", "details":"“Not before Monday”"},
            {"summary":"Posted to #launch", "glyph":"plugs-connected", "account":"Work", "badge":"already done", "link":{"label":"Open in Slack", "url":"https://slack.com/archives/C1/p2"}},
            {"summary":"Didn't upload budget.xlsx", "glyph":"plugs-connected", "tone":"warning", "details":"The channel is archived"}
         ]},
        {"kind":"assistant", "id":9812, "content":"Posted the summary to #launch.", "answered_with":"Ada Lovelace"},
        {"kind":"notice", "tone":"negative", "text":"Slack stopped working. Reconnect it to carry on.",
         "action":{"label":"Reconnect Slack", "url":"https://chatwithwork.com/482139075/settings/connectors"}}
    ])).unwrap();
    fx.chat().state.transcript.as_mut().unwrap().entries = entries;
    fx.chat().pin = 3;
    fx.chat().pinned = None;
    fx.harness.run_steps(10);
}

#[test]
fn step_outcomes_plans_and_notices_say_what_happened() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Options::default());
    show_outcomes(&mut fx);
    fx.harness
        .get_by_label("Answered with Ada Lovelace's sources");
    fx.harness
        .get_by_label("Planned and posted to Slack")
        .click_accesskit();
    fx.harness.run_steps(4);
    fx.harness.get_by_label("Denied: Post to #general");
    fx.harness.get_by_label("“Not before Monday”");
    fx.harness.get_by_label("Didn't upload budget.xlsx");
    fx.harness.get_by_label("The channel is archived");
    fx.harness.get_by_label(
        "Done: Find the Q3 numbers; Draft the summary for the launch channel; Ask finance",
    );
    fx.harness.get_by_label("Open in Slack").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Reconnect Slack").click_accesskit();
    fx.harness.run_steps(2);
    let opened = OPENED.with(|o| o.borrow().clone());
    assert_eq!(
        opened,
        [
            "https://slack.com/archives/C1/p2",
            "https://chatwithwork.com/482139075/settings/connectors",
        ]
    );
}

#[test]
fn a_free_answer_waiting_for_its_turn_says_so() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.chat().open(11);
    fx.wait_for_label("When does the Acme contract renew");
    fx.caught_up();
    let transcript = fx.chat().state.transcript.as_mut().unwrap();
    transcript.chat.state = "processing".into();
    transcript.chat.waiting_for_turn = true;
    fx.harness.run_steps(4);
    fx.harness
        .get_by_label("Free models are busy. Retrying in a moment...");
    fx.chat().state.transcript.as_mut().unwrap().chat.stopping = true;
    fx.harness.run_steps(4);
    fx.harness.get_by_label("Stopping...");
}

#[test]
fn renders_step_outcomes() {
    for dark in [false, true] {
        for (suffix, size) in [("", [1280.0, 820.0]), ("-narrow", [760.0, 820.0])] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                return;
            };
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(renderer),
                ..Options::default()
            });
            show_outcomes(&mut fx);
            fx.harness
                .get_by_label("Planned and posted to Slack")
                .click_accesskit();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            let image = fx.harness.render().expect("GPU rendering");
            let theme = if dark { "dark" } else { "light" };
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(
                    &image,
                    format!("chat-outcomes{suffix}-{theme}"),
                    &egui_kittest::SnapshotOptions::new()
                        .threshold(2.0)
                        .max_failed_pixels(1000),
                );
            }
        }
    }
}

fn show_local_preview(fx: &mut Fixture) {
    fx.open_waiting(5, "Waiting for your approval.");
    fx.caught_up();
    fx.logos();
    fx.chat().set_still(true);
    let transcript = fx.chat().state.transcript.as_mut().unwrap();
    transcript.entries.clear();
    let approval = &mut transcript.approvals[0];
    approval.summary = "Edit plan.md on Alex’s laptop".into();
    approval.service = "Alex’s laptop".into();
    approval.logo = None;
    approval.details = vec![cww::tui::chat::Detail {
        label: "Raw argument".into(),
        value: "Do not show this instead of the preview".into(),
    }];
    approval.preview = Some(serde_json::from_value(serde_json::json!({
        "action":"edit", "rows":[{"label":"File","value":"plan.md"},{"label":"In","value":"Shared work"}],
        "body":{"kind":"diff","lines":[{"kind":"hunk","text":"@@ -1 +1 @@"},{"kind":"removed","text":"-Launch on Monday."},{"kind":"added","text":"+Launch on Thursday."}]},
        "notes":[{"tone":"info","text":"If the file changes before approval, the edit will be refused."}]
    })).unwrap());
    fx.chat().pin = 3;
    fx.chat().pinned = None;
    fx.harness.run_steps(10);
}

#[test]
fn code_files_are_available_while_steps_are_folded_and_save_through_the_daemon() {
    let mut fx = fixture(Options::default());
    show_code_run(&mut fx);
    fx.harness.get_by_label("budget.csv, 27 Bytes");
    assert!(fx.harness.query_by_label("Ran Python").is_none());
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("saved.csv");
    SAVED.with(|p| *p.borrow_mut() = Some(destination.clone()));
    fx.harness
        .get_by_label("budget.csv, 27 Bytes")
        .click_accesskit();
    fx.wait_for_request("chat_download", |r| {
        r["path"].as_str().unwrap().contains("/budget.csv?")
    });
    fx.wait("file saved", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.notice.as_deref() == Some("Saved budget.csv"))
    });
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"Vendor,Total\nAcme,42000\n"
    );
    fx.harness.get_by_label("Ran code").click_accesskit();
    fx.wait_for_label("Ran Python");
    fx.harness.get_by_label_contains("total = 42000");
    fx.harness.get_by_label_contains("Exit code: 0");
}

#[test]
fn local_changes_show_the_preview_instead_of_raw_arguments() {
    let mut fx = fixture(Options::default());
    show_local_preview(&mut fx);
    fx.harness.get_by_label("File: plan.md");
    fx.harness.get_by_label_contains("+Launch on Thursday.");
    fx.harness
        .get_by_label_contains("If the file changes before approval");
    assert!(
        fx.harness
            .query_by_label_contains("Do not show this")
            .is_none()
    );
    fx.harness.get_by_label("Approve").click_accesskit();
    fx.wait_for_request("chat_approve", |_| true);
}

#[test]
fn renders_code_files_and_change_previews() {
    for dark in [false, true] {
        for (suffix, size) in [("", [1280.0, 820.0]), ("-narrow", [760.0, 820.0])] {
            for preview in [false, true] {
                let Ok(renderer) =
                    std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
                else {
                    return;
                };
                let mut fx = fixture(Options {
                    size,
                    dark,
                    renderer: Some(renderer),
                    ..Options::default()
                });
                if preview {
                    show_local_preview(&mut fx);
                } else {
                    show_code_run(&mut fx);
                    fx.harness.get_by_label("Ran code").click_accesskit();
                }
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(12);
                let image = fx.harness.render().expect("GPU rendering");
                let name = if preview {
                    "chat-local-change"
                } else {
                    "chat-code"
                };
                let theme = if dark { "dark" } else { "light" };
                if cfg!(target_os = "linux") {
                    egui_kittest::image_snapshot_options(
                        &image,
                        format!("{name}{suffix}-{theme}"),
                        &egui_kittest::SnapshotOptions::new()
                            .threshold(2.0)
                            .max_failed_pixels(1000),
                    );
                }
            }
        }
    }
}

fn wait_for_notification_view(fx: &mut Fixture, after: usize, visible: bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(request) = fx
            .server
            .requests()
            .into_iter()
            .skip(after)
            .find(|request| request["cmd"] == "notification_view" && request["visible"] == visible)
        {
            return request;
        }
        assert!(
            Instant::now() < deadline,
            "no visibility={visible} after request {after}"
        );
        fx.harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn notification_presence_follows_window_focus_page_and_close() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.open(11);
    let shown = wait_for_notification_view(&mut fx, 0, true);
    assert_eq!(shown["account"], "482139075");
    assert_eq!(shown["chat"], 11);
    let after = fx.server.requests().len();
    fx.harness.input_mut().focused = false;
    fx.harness.event(egui::Event::WindowFocused(false));
    let hidden = wait_for_notification_view(&mut fx, after, false);
    assert_eq!(hidden["session"], shown["session"]);
    assert_eq!(hidden["chat"], 11);
    let after = fx.server.requests().len();
    fx.harness.input_mut().focused = true;
    fx.harness.event(egui::Event::WindowFocused(true));
    wait_for_notification_view(&mut fx, after, true);
    let after = fx.server.requests().len();
    // Minimized windows may stop rendering. The shell must clear directly.
    fx.harness.state().shared().notification_foreground(false);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fx
            .server
            .requests()
            .iter()
            .skip(after)
            .any(|request| request["cmd"] == "notification_view" && request["visible"] == false)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "minimizing needed a UI frame to clear visibility"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let after = fx.server.requests().len();
    fx.harness.state().shared().notification_foreground(true);
    wait_for_notification_view(&mut fx, after, true);
    let after = fx.server.requests().len();
    fx.harness.state_mut().set_page(Page::General);
    wait_for_notification_view(&mut fx, after, false);
    let after = fx.server.requests().len();
    fx.harness.state_mut().set_page(Page::Chat);
    wait_for_notification_view(&mut fx, after, true);
    let after = fx.server.requests().len();
    let Fixture { server, harness } = fx;
    drop(harness);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if server
            .requests()
            .iter()
            .skip(after)
            .any(|request| request["cmd"] == "notification_view" && request["visible"] == false)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closing the window did not clear visibility"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn organization_switcher_review_screens() {
    let Ok(directory) = std::env::var("CWW_ORGANIZATIONS_SCREENSHOTS") else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                dark,
                size: [width, 900.0],
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait("the chats", |h| {
                h.state().chat_state().is_some_and(|s| s.ready())
            });
            let (_profile, _server) = add_organization(&mut fx);
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            if width < 600.0 {
                fx.chat().drawer = true;
            }
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            let image = fx.harness.render().expect("GPU rendering");
            let theme = if dark { "dark" } else { "light" };
            image
                .save(
                    std::path::Path::new(&directory)
                        .join(format!("organizations-{theme}-{width}.png")),
                )
                .unwrap();
            fx.harness.get_by_label("Organization: Northwind").click();
            fx.wait_for_label("Akamai Robotics");
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            fx.harness
                .render()
                .expect("GPU rendering")
                .save(
                    std::path::Path::new(&directory)
                        .join(format!("organizations-menu-{theme}-{width}.png")),
                )
                .unwrap();
        }
    }
}

fn add_organization(
    fx: &mut Fixture,
) -> (cww::auth::organizations::SavedOrganization, demo::Server) {
    use cww::auth::{client::Organization, organizations::SavedOrganization};
    let profile = SavedOrganization {
        id: "a".repeat(43),
        server: cww::config::ServerConfig {
            url: "https://chatwithwork.com".into(),
            device_id: "second-organization-device".into(),
        },
        account: Organization {
            id: "918273645".into(),
            name: "Akamai Robotics".into(),
            role: "admin".into(),
        },
        secret_store: "file".into(),
    };
    let paths = fx.harness.state().shared.paths.clone();
    cww::paths::write_private_file(
        &paths.daemon.config_dir.join("organizations.json"),
        &serde_json::to_vec(&serde_json::json!({"version":1,"organizations":[profile]})).unwrap(),
    )
    .unwrap();
    let server = demo::start_organization(&paths, &profile).unwrap();
    server.edit_elsewhere(11, |chat| {
        chat["title"] = "Robotics contract renewal".into()
    });
    let app = fx.harness.state_mut();
    app.organizations.refresh();
    fx.wait("organization loaded", |h| {
        h.state().organizations.profiles.len() == 1
    });
    (profile, server)
}

#[test]
fn organization_switching_keeps_requests_presence_and_settings_in_their_account() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.harness.get_by_label("Vendor contract renewal").click();
    fx.wait("first chat loaded", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.open == Some(11) && !s.loading)
    });
    // Presence runs on its own worker. Establish the view before switching:
    // a view that was never sent may correctly coalesce away without a clear.
    let shown = wait_for_notification_view(&mut fx, 0, true);
    let (_, second) = add_organization(&mut fx);
    let before = fx.server.requests().len();
    fx.harness.get_by_label("Organization: Northwind").click();
    fx.wait_for_label("Akamai Robotics");
    fx.harness.get_by_label("Akamai Robotics").click();
    fx.wait_for_label("Robotics contract renewal");
    assert!(fx.chat().state.open.is_none());
    fx.harness.get_by_label("Robotics contract renewal").click();
    fx.wait("second chat loaded", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.open == Some(11) && !s.loading)
    });
    let hidden = wait_for_notification_view(&mut fx, before, false);
    assert_eq!(hidden["session"], shown["session"]);
    assert_eq!(hidden["account"], shown["account"]);
    assert_eq!(hidden["chat"], shown["chat"]);
    fx.wait("second organization receives presence", |_| {
        second.requests().iter().any(|r| {
            r["cmd"] == "notification_view" && r["account"] == "918273645" && r["visible"] == true
        })
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .skip(before)
            .any(|r| r["cmd"] == "chat")
    );
    assert_eq!(
        fx.harness.state().chat.as_ref().unwrap().data_dir,
        fx.server.paths.daemon.data_dir
    );
    fx.harness.state_mut().set_page(Page::Notifications);
    fx.wait("second organization preferences", |_| {
        second
            .requests()
            .iter()
            .any(|r| r["cmd"] == "notification_preferences")
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .skip(before)
            .any(|r| r["cmd"] == "notification_preferences")
    );
    fx.harness.state_mut().set_page(Page::Folders);
    fx.wait_for_label("Documents");
    assert!(!second.requests().iter().any(|r| {
        r["cmd"]
            .as_str()
            .is_some_and(|cmd| cmd.starts_with("roots_"))
    }));
    fx.harness.state_mut().set_page(Page::Chat);
    fx.wait_for_label("Organization: Akamai Robotics");
    fx.harness
        .get_by_label("Organization: Akamai Robotics")
        .click();
    fx.wait_for_label("Northwind");
    fx.harness.get_by_label("Northwind").click();
    fx.wait("first chat remembered", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "482139075" && s.open == Some(11) && !s.loading)
    });
}

#[test]
fn notification_switches_organization_after_unsaved_instructions_are_decided() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("write your instructions");
    let (profile, second) = add_organization(&mut fx);
    fx.harness
        .query_all_by_label("write your instructions")
        .next()
        .unwrap()
        .click();
    fx.wait_for_label("Use a template");
    fx.harness.run_steps(8);
    fx.harness.get_by_label("Use a template").click();
    fx.harness.run_steps(3);
    let target = notification_target(&profile.server.url, &profile.account.id, 11);
    fx.harness.state().shared().open_chat(target.clone());
    fx.wait_for_label("Discard unsaved instructions?");
    assert!(second.requests().iter().all(|r| r["cmd"] != "chat"));
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(4);
    assert!(fx.harness.state().organizations.selected.is_none());
    fx.harness.state().shared().open_chat(target);
    fx.wait_for_label("Discard unsaved instructions?");
    fx.harness.get_by_label("Discard changes").click();
    fx.wait("notification opened the second account", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "918273645" && s.open == Some(11) && !s.loading)
    });
    assert!(!fx.server.requests().iter().any(|r| r["cmd"] == "chat"));
}

#[test]
fn switching_sign_ins_discards_editor_history_and_forgotten_grants() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    let (profile, _second) = add_organization(&mut fx);
    let field = fx
        .harness
        .get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("Synthetic private draft from first organization");
    fx.harness.run_steps(3);
    assert!(!fx.chat().state.input.is_empty());
    fx.harness
        .state_mut()
        .select_organization(Some(profile.id.clone()));
    assert!(
        egui::text_edit::TextEditState::load(&fx.harness.ctx, egui::Id::new(super::composer::ID))
            .is_none()
    );
    assert_eq!(
        AppState::load(&fx.server.paths).organization,
        Some(profile.id)
    );
    fx.wait_for_label("Robotics contract renewal");
    assert!(fx.chat().state.input.is_empty());
    let saved = fx.server.paths.daemon.config_dir.join("organizations.json");
    cww::paths::write_private_file(&saved, br#"{"version":1,"organizations":[]}"#).unwrap();
    fx.harness.state_mut().organizations.refresh();
    fx.wait("forgotten grant retires its chat", |h| {
        h.state().organizations.selected.is_none()
            && h.state()
                .chat_state()
                .is_some_and(|s| s.list.account.id == "482139075")
    });
    assert!(AppState::load(&fx.server.paths).organization.is_none());
}

#[test]
fn a_cancelled_native_sign_in_cannot_reopen_its_approval_code() {
    let mut fx = fixture(Options::default());
    let app = fx.harness.state_mut();
    let generation = app.account.generation;
    app.account.organization = true;
    app.account.pairing = Some(None);
    app.cancel_pairing();
    app.jobs
        .tx
        .send(crate::ui::Done::Pairing(
            generation,
            crate::pairing::PairEvent::Code(crate::pairing::Code {
                user_code: "SYNTHETIC".into(),
                ..Default::default()
            }),
        ))
        .unwrap();
    fx.harness.run_steps(3);
    assert!(fx.harness.state().account.pairing.is_none());
}

#[test]
fn chat_organization_review_screens() {
    let Ok(directory) = std::env::var("CWW_CHAT_ORGANIZATION_EVIDENCE") else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [false, true] {
        for (name, size) in [("wide", [1280.0, 900.0]), ("narrow", [390.0, 844.0])] {
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Default::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().drawer = size[0] < 800.0;
            fx.harness.run_steps(5);
            fx.chat().state.menu = Some(11);
            fx.harness.run_steps(5);
            fx.harness.event(egui::Event::PointerGone);
            let theme = if dark { "dark" } else { "light" };
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!("menu-{name}-{theme}.png")))
                .unwrap();
            fx.harness.get_by_label("Move to project").click();
            fx.wait_for_label("What they'll see");
            fx.harness.run_steps(5);
            fx.harness.event(egui::Event::PointerGone);
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!("move-top-{name}-{theme}.png")))
                .unwrap();
            fx.harness.event(egui::Event::PointerMoved(egui::pos2(
                size[0] - 80.0,
                size[1] / 2.0,
            )));
            fx.harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, -1000.0),
                modifiers: egui::Modifiers::NONE,
            });
            fx.harness.run_steps(15);
            fx.harness.event(egui::Event::PointerGone);
            fx.harness
                .render()
                .unwrap()
                .save(
                    std::path::Path::new(&directory)
                        .join(format!("move-bottom-{name}-{theme}.png")),
                )
                .unwrap();
            fx.chat().state.move_chat(10);
            fx.harness.run_steps(5);
            fx.harness.event(egui::Event::PointerMoved(egui::pos2(
                size[0] - 80.0,
                size[1] / 2.0,
            )));
            fx.harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, -1500.0),
                modifiers: egui::Modifiers::NONE,
            });
            fx.harness.run_steps(15);
            fx.harness.event(egui::Event::PointerGone);
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!("private-{name}-{theme}.png")))
                .unwrap();
        }
    }
}

#[test]
fn chat_pins_and_project_moves_use_native_controls_and_explain_the_audience() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.open(11);
    fx.wait_for_label("The Acme contract renews");
    fx.harness
        .get_all_by_label("Chat options")
        .nth(1)
        .unwrap()
        .click();
    fx.wait("pin menu item", |h| h.query_by_label("Pin").is_some());
    fx.harness.get_by_label("Pin").click();
    fx.wait("pin returned", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .list
            .pins
            .chats
            .contains(&11)
    });
    fx.wait_for_request("chat_pin", |r| r["chat"] == "11" && r["pinned"] == true);
    fx.harness
        .get_by_label("Unpin Vendor contract renewal")
        .click();
    fx.wait("unpin returned", |h| {
        !h.state()
            .chat_state()
            .unwrap()
            .list
            .pins
            .chats
            .contains(&11)
    });
    fx.harness.get_by_label("Pin to sidebar").click();
    fx.wait("conversation pin returned", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .list
            .pins
            .chats
            .contains(&11)
    });
    fx.harness.get_by_label("Unpin from sidebar").click();
    fx.wait("conversation unpin returned", |h| {
        !h.state()
            .chat_state()
            .unwrap()
            .list
            .pins
            .chats
            .contains(&11)
    });
    fx.harness
        .get_all_by_label("Chat options")
        .nth(1)
        .unwrap()
        .click();
    fx.wait_for_label("Move to project");
    fx.harness.get_by_label("Move to project").click();
    fx.wait_for_label("What they'll see");
    fx.harness.get_by_label("Not what your tools returned");
    assert!(!fx.server.requests().iter().any(|r| r["cmd"] == "chat_move"));
    assert!(
        super::super::visible_notification_chat(Page::Chat, true, fx.harness.state().chat_state())
            .is_none()
    );
    fx.harness.get_by_label("Project: Falcon").click();
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Move chat").click();
    fx.wait_for_request("chat_move", |r| r["chat"] == "11" && r["project"] == 7);
    fx.wait("project move returned", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .open_summary()
            .is_some_and(|chat| chat.project.as_ref().is_some_and(|p| p.id == 7))
    });
    fx.wait_for_label("Change project");
    fx.caught_up();
    fx.harness
        .event(egui::Event::PointerMoved(egui::pos2(1000.0, 400.0)));
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        phase: egui::TouchPhase::Move,
        delta: egui::vec2(0.0, 2000.0),
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(15);
    fx.harness.get_by_label("Change project").click();
    fx.wait_for_label("What they'll see");
    fx.harness
        .event(egui::Event::PointerMoved(egui::pos2(1000.0, 600.0)));
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        phase: egui::TouchPhase::Move,
        delta: egui::vec2(0.0, -1000.0),
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(15);
    fx.wait_for_label("Make private");
    fx.harness
        .get_by_label("Only you will see it. Anything people already read stays with them.");
    fx.harness.get_by_label("Make private").click();
    fx.wait_for_request("chat_move", |r| r["chat"] == "11" && r["project"].is_null());
    fx.wait("private again", |h| {
        h.state()
            .chat_state()
            .unwrap()
            .open_summary()
            .is_some_and(|chat| chat.project.is_none())
    });
    assert!(fx.harness.state().chat_state().unwrap().moving.is_none());
}

#[test]
fn chat_history_review_screens() {
    let Ok(directory) = std::env::var("CWW_CHAT_HISTORY_EVIDENCE") else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for dark in [false, true] {
        for (name, size) in [("wide", [1280.0, 900.0]), ("narrow", [390.0, 844.0])] {
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Default::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().drawer = size[0] < 800.0;
            fx.harness.run_steps(5);
            if std::env::var_os("CWW_CHAT_HISTORY_AFTER").is_some() {
                fx.harness.get_by_label("Chats").click();
                fx.wait_for_label("Search your chats");
                fx.wait_for_request("chat_history", |_| true);
                fx.wait("history loaded for capture", |h| {
                    h.state()
                        .chat_ref()
                        .unwrap()
                        .state
                        .history
                        .as_ref()
                        .is_some_and(|page| page.loaded)
                });
                let page = fx.chat().state.history.as_mut().unwrap();
                page.items.truncate(5);
                page.total = Some(5);
                let titles = [
                    "Q3 budget review",
                    "Vendor contract renewal",
                    "Weekly report for leadership",
                    "Falcon launch checklist",
                    "Competitor pricing notes",
                ];
                let groups = [
                    "Today",
                    "Yesterday",
                    "Previous 7 days",
                    "Previous 30 days",
                    "Earlier",
                ];
                let ages = [
                    "less than a minute ago",
                    "1 day ago",
                    "4 days ago",
                    "15 days ago",
                    "about 1 month ago",
                ];
                for (i, item) in page.items.iter_mut().enumerate() {
                    item.chat.title = titles[i].into();
                    item.chat.state = "idle".into();
                    item.group = groups[i].into();
                    item.age = ages[i].into();
                }
                fx.harness.run_steps(3);
            }
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(2);
            fx.harness
                .render()
                .unwrap()
                .save(std::path::Path::new(&directory).join(format!(
                    "history-{name}-{}.png",
                    if dark { "dark" } else { "light" }
                )))
                .unwrap();
        }
    }
}

#[test]
fn native_projects_open_pin_edit_and_start_a_scoped_chat() {
    let mut fx = fixture(Options::default());
    fx.ready();
    OPENED.with(|urls| urls.borrow_mut().clear());
    fx.harness.get_by_label("All projects").click();
    fx.wait_for_label("Projects you're not on");
    fx.wait_for_request("projects", |r| r["request"]["action"] == "list");
    fx.harness.get_by_label("Platform partners").click();
    fx.wait_for_label("You're not on this project");
    fx.harness.get_by_label("Join").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("Pin to sidebar").click();
    fx.wait_for_label("Unpin from sidebar");
    fx.harness.get_by_label("Settings").click();
    fx.wait_for_label("Save project");
    let field = fx.harness.get_by_label("Project name");
    field.click();
    field.type_text(" updated");
    fx.harness.run_steps(3);
    fx.harness.get_by_label("All projects").click();
    fx.wait_for_label("Discard unsaved project changes?");
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(2);
    assert!(fx.chat().state.projects.as_ref().unwrap().dirty());
    fx.harness.get_by_label("Save project").click();
    fx.wait_for_request("projects", |r| {
        r["request"]["action"] == "update"
            && r["request"]["fields"]["name"] == "Platform partners updated"
    });
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("New chat here").click();
    fx.wait_for_request("workspace", |r| r["project"] == 9);
    assert_eq!(fx.chat().state.new_project.as_ref().unwrap().id, 9);
    fx.chat().state.input = "Synthetic question in this project".into();
    let commands = fx.chat().state.send();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, super::state::Command::Send(ask) if ask.project == Some(9)))
    );
    OPENED.with(|urls| assert!(urls.borrow().is_empty()));
}

#[test]
fn native_projects_create_archive_and_delete_with_confirmation() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("All projects").click();
    fx.wait_for_label("New project");
    fx.harness.get_by_label("New project").click();
    fx.wait_for_label("Project name");
    let field = fx.harness.get_by_label("Project name");
    field.click();
    field.type_text("Synthetic launch");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Create project").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("Settings").click();
    fx.wait_for_label("Archive");
    fx.harness
        .query_all_by_label("Archive")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness
        .query_all_by_label("Archive")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .click();
    fx.wait_for_request("projects", |r| r["request"]["action"] == "archive");
    fx.wait_for_label("This project is archived.");
    fx.harness.get_by_label("Settings").click();
    fx.wait_for_label("Unarchive");
    fx.harness
        .query_all_by_label("Delete project")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness
        .query_all_by_label("Delete project")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .click();
    fx.wait_for_label("Delete project?");
    fx.harness.get_by_label("Cancel project action").click();
    fx.harness.run_steps(2);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "projects" && r["request"]["action"] == "delete")
    );
    fx.harness
        .query_all_by_label("Delete project")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness
        .query_all_by_label("Delete project")
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
        .unwrap()
        .click();
    fx.wait_for_label("Delete project?");
    fx.harness.get_by_label("Delete").click();
    fx.wait_for_request("projects", |r| r["request"]["action"] == "delete");
    fx.wait_for_label("New project");
}

#[test]
fn chat_history_search_open_and_delete_use_native_controls() {
    let mut fx = fixture(Options::default());
    fx.ready();
    OPENED.with(|urls| urls.borrow_mut().clear());
    fx.harness.get_by_label("Chats").click();
    fx.wait("history loaded", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| page.loaded)
    });
    assert_eq!(fx.chat().state.history.as_ref().unwrap().items.len(), 10);
    fx.chat().state.history.as_mut().unwrap().next_cursor = Some("next".into());
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Load more chats").click();
    fx.wait_for_request("chat_history", |r| r["before"] == "next");
    fx.wait("next page loaded", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| !page.loading)
    });
    assert_eq!(
        fx.chat().state.history.as_ref().unwrap().items.len(),
        10,
        "updated rows do not duplicate"
    );
    assert!(
        fx.chat()
            .state
            .attach(vec![std::path::PathBuf::from("synthetic.txt")])
            .is_empty(),
        "history has no attachment composer"
    );
    fx.chat().state.menu = Some(11);
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Move to project").click();
    fx.wait_for_label("What they'll see");
    assert!(fx.chat().state.history.is_none());
    fx.harness.get_by_label("Chats").click();
    fx.wait("history after move page", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| page.loaded)
    });
    assert!(fx.chat().state.open.is_none());
    let field = fx.harness.get_by_label("Search your chats");
    field.click();
    field.type_text("Vendor contract");
    fx.wait_for_request("chat_history", |r| r["query"] == "Vendor contract");
    fx.wait("filtered history", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| page.loaded && page.items.len() == 1)
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "chat_delete")
    );
    fx.harness
        .get_by_label("Delete Vendor contract renewal")
        .click();
    fx.wait_for_label("Are you sure?");
    fx.harness.get_by_label("Cancel").click();
    fx.harness.run_steps(3);
    fx.harness
        .query_all_by_label("Vendor contract renewal")
        .last()
        .unwrap()
        .click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    fx.caught_up();
    assert!(fx.chat().state.history.is_none());
    fx.harness.get_by_label("Chats").click();
    fx.wait("history reopened", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| page.loaded)
    });
    fx.harness
        .get_by_label("Delete Vendor contract renewal")
        .click();
    fx.wait_for_label("Are you sure?");
    fx.harness.get_by_label("Delete").click();
    fx.wait_for_request("chat_delete", |r| r["chat"] == "11");
    fx.wait("chat removed from history", |h| {
        h.state()
            .chat_ref()
            .unwrap()
            .state
            .history
            .as_ref()
            .is_some_and(|page| !page.items.iter().any(|item| item.chat.number == 11))
    });
    assert_eq!(fx.chat().state.history.as_ref().unwrap().total, Some(9));
    OPENED.with(|urls| assert!(urls.borrow().is_empty(), "history never opens a browser"));
}

#[test]
fn chat_history_keeps_unsaved_instructions_until_discarded() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("write your instructions");
    fx.harness
        .query_all_by_label("write your instructions")
        .next()
        .unwrap()
        .click();
    fx.wait_for_label("Use a template");
    fx.harness.run_steps(8);
    fx.harness.get_by_label("Use a template").click();
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Chats").click();
    fx.wait_for_label("Discard unsaved instructions?");
    assert!(fx.chat().state.history.is_none());
    fx.harness.get_by_label("Discard changes").click();
    fx.wait_for_label("Search your chats");
    assert!(fx.chat().state.instructions.is_none());
}

#[test]
fn project_pages_review_screens() {
    let Ok(directory) = std::env::var("CWW_PROJECTS_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for (name, size) in [("wide", [1280.0, 900.0]), ("narrow", [390.0, 844.0])] {
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Default::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().drawer = size[0] < 800.0;
            fx.harness.run_steps(5);
            for (kind, request) in [
                ("before", None),
                ("list", Some(cww::projects::Request::List)),
                (
                    "detail",
                    Some(cww::projects::Request::Show {
                        project: 7,
                        before: None,
                    }),
                ),
                ("form", Some(cww::projects::Request::Edit { project: 7 })),
            ] {
                if let Some(request) = request {
                    fx.chat().drawer = false;
                    let commands = fx.chat().state.project_request(request);
                    fx.chat().run(commands);
                    fx.wait("project page ready", |h| {
                        h.state()
                            .chat_ref()
                            .unwrap()
                            .state
                            .projects
                            .as_ref()
                            .is_some_and(|p| !p.busy && p.view.is_some())
                    });
                }
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(3);
                let path = std::path::Path::new(&directory).join(kind);
                std::fs::create_dir_all(&path).unwrap();
                fx.harness
                    .render()
                    .unwrap()
                    .save(path.join(format!(
                        "{name}-{}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
        }
    }
}

#[test]
fn native_project_instructions_and_connector_changes_keep_the_project_scope() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("Falcon").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("Project instructions").click();
    fx.wait_for_label("Back to project");
    fx.wait_for_request("instructions", |r| r["project"] == 7);
    fx.wait_for_label("Use a template");
    fx.harness
        .query_all_by_label("Use a template")
        .last()
        .unwrap()
        .scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness
        .query_all_by_label("Use a template")
        .last()
        .unwrap()
        .click();
    fx.harness.run_steps(5);
    fx.harness
        .query_all_by_label("Save")
        .last()
        .unwrap()
        .scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness
        .query_all_by_label("Save")
        .last()
        .unwrap()
        .click();
    fx.wait_for_request("instruction_update", |r| {
        r["project"] == 7 && r["layer"] == "project"
    });
    fx.wait_for_label("Saved your instructions.");
    fx.harness.get_by_label("Back to project").scroll_to_me();
    fx.harness.run_steps(10);
    fx.harness.get_by_label("Back to project").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("New chat here").click();
    fx.wait_for_label("Google Drive, connected");
    fx.harness.get_by_label("Google Drive, connected").click();
    fx.wait_for_label("Pause");
    fx.harness.get_by_label("Pause").click();
    fx.wait_for_label("Google Drive, paused");
    let requests = fx.server.requests();
    let paused = requests
        .iter()
        .rposition(|r| r["cmd"] == "service_pause")
        .unwrap();
    assert!(
        requests[paused + 1..]
            .iter()
            .any(|r| r["cmd"] == "workspace" && r["project"] == 7)
    );
    assert!(
        fx.chat()
            .state
            .start
            .as_ref()
            .unwrap()
            .instructions
            .iter()
            .any(|i| i.layer == cww::workspace::Layer::Project)
    );
}

#[test]
fn project_instructions_upload_templates_and_guard_navigation_to_personal_settings() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("Falcon").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("Project instructions").click();
    fx.wait_for_label("Upload project document template");
    fx.wait_for_request("templates", |r| r["request"]["project"] == 7);
    let path = fx
        .server
        .paths
        .daemon
        .data_dir
        .join("Project letterhead.docx");
    std::fs::write(&path, b"synthetic project template").unwrap();
    PICKED.with(|p| *p.borrow_mut() = Some(vec![path]));
    fx.harness
        .get_by_label("Upload project document template")
        .click_accesskit();
    fx.wait_for_request("template_upload", |r| {
        r["target"]["project"] == 7
            && r["target"]["account"] == "482139075"
            && r["target"]["layer"] == "project"
    });
    fx.wait_for_label("Replace project document template");
    fx.chat()
        .state
        .instructions
        .as_mut()
        .unwrap()
        .editors
        .iter_mut()
        .find(|e| e.instruction.layer == cww::workspace::Layer::Project)
        .unwrap()
        .body = "Unsaved project notes".into();
    fx.harness
        .get_by_label("Open Your instructions")
        .click_accesskit();
    fx.wait_for_label("Discard unsaved instructions?");
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(3);
    assert_eq!(
        fx.chat().state.instructions.as_ref().unwrap().project,
        Some(7)
    );
    fx.harness
        .get_by_label("Open Your instructions")
        .click_accesskit();
    fx.wait_for_label("Discard changes");
    fx.harness.get_by_label("Discard changes").click();
    fx.wait_for_request("instructions", |r| r["project"].is_null());
    fx.wait_for_label("Let the assistant remember things");
    let page = fx.chat().state.instructions.as_ref().unwrap();
    assert!(page.project.is_none() && page.templates.is_none());
    assert_eq!(page.context.as_ref().unwrap().account.id, "482139075");
    fx.harness.get_by_label("Templates").click_accesskit();
    fx.wait_for_label("Upload personal document template");
}

#[test]
fn instruction_context_binds_project_ids_and_template_numbers_and_discards_late_replies() {
    use super::{
        state::{ChatState, Command, Msg},
        workspace::Instructions,
    };
    use cww::workspace::{InstructionPage, Layer};
    let response = |account: &str, id, number| -> InstructionPage {
        serde_json::from_value(serde_json::json!({
            "context":{"account":{"id":account,"name":"Northwind"},"email":"alex@example.com","project":{"id":id,"number":number,"name":"Launch","archived":false}},
            "instructions":[],"memory":{"enabled":true,"entries":[]}
        })).unwrap()
    };
    let mut page = Instructions::new(Layer::Project);
    page.account = Some("482139075".into());
    page.project = Some(700);
    let commands = page.loaded(Ok(response("482139075", 700, 9)));
    assert!(matches!(
        commands.as_slice(),
        [Command::Templates(
            _,
            cww::templates::Request::List { project: Some(9) }
        )]
    ));
    let old = page.templates.as_ref().unwrap().generation;
    let mut state = ChatState::default();
    state.instructions = Some(page);
    let failure = cww::tui::chat::Failure {
        code: "forbidden".into(),
        message: "Access removed".into(),
        ..Default::default()
    };
    state.update(Msg::Templates(old, Err(failure.clone())));
    assert_eq!(
        state
            .instructions
            .as_ref()
            .unwrap()
            .templates
            .as_ref()
            .unwrap()
            .error
            .as_deref(),
        Some("Access removed")
    );
    for result in [response("918273645", 700, 9), response("482139075", 701, 9)] {
        let page = state.instructions.as_mut().unwrap();
        assert!(page.loaded(Ok(result)).is_empty());
        assert!(page.context.is_none() && page.templates.is_none() && page.memory.is_none());
    }
    let page = state.instructions.as_mut().unwrap();
    page.loaded(Ok(response("482139075", 700, 9)));
    assert!(state.update(Msg::Templates(old, Err(failure))).is_empty());
    let page = state.instructions.as_ref().unwrap();
    assert!(page.templates.as_ref().unwrap().busy);
    assert!(page.templates.as_ref().unwrap().error.is_none());
}

#[test]
fn native_project_edits_guard_notification_navigation() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("Falcon").click();
    fx.wait_for_label("New chat here");
    fx.harness.get_by_label("Settings").click();
    fx.wait_for_label("Save project");
    let field = fx.harness.get_by_label("Project name");
    field.click();
    field.type_text(" draft");
    fx.harness.run_steps(3);
    let target = notification_target("https://chatwithwork.com", "482139075", 11);
    assert!(!fx.chat().may_open_notification(&target));
    fx.wait_for_label("Discard unsaved project changes?");
    fx.harness.get_by_label("Keep editing").click();
    fx.harness.run_steps(3);
    assert!(fx.chat().state.projects.as_ref().unwrap().dirty());
    fx.chat().open(11);
    fx.wait_for_label("Discard unsaved project changes?");
    fx.harness.get_by_label("Discard changes").click();
    fx.wait_for_request("chat", |r| r["chat"] == "11");
    assert!(fx.chat().state.projects.is_none());
}

#[test]
fn model_preferences_save_the_personal_default_without_changing_the_open_chat() {
    let mut fx = fixture(Options::default());
    fx.open(11);
    fx.wait("the original transcript", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.transcript.is_some() && !s.loading)
    });
    let original_model = fx
        .chat()
        .state
        .transcript
        .as_ref()
        .unwrap()
        .chat
        .model
        .clone();
    fx.harness.state_mut().set_page(Page::Models);
    fx.wait_for_label("Save default model");
    fx.harness.get_by_label("GPT-6.1 Sol").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_label("Save default model")
        .click_accesskit();
    fx.wait_for_label("Default model updated.");
    let request = fx.wait_for_request("model_preferences_update", |r| r["model_id"] == "15");
    assert_eq!(request["account"], "482139075");
    let state = &fx.chat().state;
    let screen = state.model_preferences.as_ref().unwrap();
    let preferences = screen.preferences.as_ref().unwrap();
    assert_eq!(preferences.preferred_model_id.as_deref(), Some("15"));
    assert_eq!(preferences.catalog.default_model_id.as_deref(), Some("12"));
    assert_eq!(
        state.transcript.as_ref().unwrap().chat.model,
        original_model
    );
    assert!(super::super::visible_notification_chat(Page::Chat, true, Some(state)).is_none());
    fx.harness.get_by_label("Gemini 4 Argon").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_label("Save default model")
        .click_accesskit();
    fx.wait_for_request("model_preferences_update", |r| r["model_id"] == "14");
    fx.wait_for_label("Default model updated.");
    fx.harness.get_by_label("New chat").click_accesskit();
    fx.wait_for_label("Model: Gemini 4 Argon");
}

#[test]
fn model_preferences_guard_unsaved_navigation_and_keep_organization_requests_separate() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    let (profile, second) = add_organization(&mut fx);
    fx.harness.state_mut().set_page(Page::Models);
    fx.wait_for_label("Save default model");
    fx.harness.get_by_label("Gemini 4 Argon").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard model selection?");
    fx.harness.get_by_label("Keep editing").click_accesskit();
    fx.harness.run_steps(4);
    assert!(fx.harness.state().organizations.selected.is_none());
    assert!(fx.chat().state.model_preferences.as_ref().unwrap().dirty());
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard model selection?");
    fx.harness.get_by_label("Discard changes").click_accesskit();
    fx.wait("second account opened", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "918273645" && s.open == Some(11) && !s.loading)
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "model_preferences_update")
    );
    fx.harness.state_mut().set_page(Page::Models);
    fx.wait_for_label("Save default model");
    fx.harness.get_by_label("Gemini 4 Argon").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_label("Save default model")
        .click_accesskit();
    fx.wait_for_label("Default model updated.");
    assert!(
        second
            .requests()
            .iter()
            .any(|r| r["cmd"] == "model_preferences_update" && r["account"] == "918273645")
    );
}

#[test]
fn model_preferences_review_screens() {
    let Ok(directory) = std::env::var("CWW_MODEL_PREFERENCES_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                dark,
                size: [width, 900.0],
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait("the model settings account", |h| {
                h.state().chat_state().is_some_and(|s| s.ready())
            });
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            for (name, models) in [("before", false), ("models", true)] {
                if models {
                    fx.harness.state_mut().set_page(Page::Models);
                    fx.wait_for_label("Save default model");
                }
                fx.logos();
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(12);
                let path = std::path::Path::new(&directory).join(name);
                std::fs::create_dir_all(&path).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(path.join(format!(
                        "{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
        }
    }
}

#[test]
fn template_upload_replace_download_and_confirmed_removal_stay_in_the_current_organization() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.harness.state_mut().set_page(Page::Templates);
    fx.wait_for_label("Upload personal document template");
    assert!(
        super::super::visible_notification_chat(Page::Chat, true, Some(&fx.chat().state)).is_none()
    );
    let file = fx.server.paths.daemon.data_dir.join("letterhead.docx");
    std::fs::write(&file, b"synthetic office bytes").unwrap();
    PICKED.with(|p| *p.borrow_mut() = Some(vec![file.clone()]));
    fx.harness
        .get_by_label("Upload personal document template")
        .click_accesskit();
    fx.wait_for_label("Replace personal document template");
    let request = fx.wait_for_request("template_upload", |r| r["filename"] == "letterhead.docx");
    assert_eq!(request["target"]["account"], "482139075");
    assert_eq!(request["target"]["layer"], "personal");
    assert_eq!(request["target"]["kind"], "document");
    assert!(request["target"]["project"].is_null());
    let destination = fx.server.paths.daemon.data_dir.join("saved.docx");
    SAVED.with(|p| *p.borrow_mut() = Some(destination.clone()));
    fx.harness
        .get_by_label("Download personal document template")
        .click_accesskit();
    fx.wait("download saved", |_| destination.exists());
    assert!(!std::fs::read(&destination).unwrap().is_empty());
    let replaced = fx.server.paths.daemon.data_dir.join("replacement.dotx");
    std::fs::write(&replaced, b"replacement synthetic bytes").unwrap();
    PICKED.with(|p| *p.borrow_mut() = Some(vec![replaced]));
    fx.harness
        .get_by_label("Replace personal document template")
        .click_accesskit();
    fx.wait_for_label("replacement.dotx");
    fx.harness
        .get_by_label("Remove personal document template")
        .click_accesskit();
    fx.wait_for_label("Remove template");
    fx.harness.get_by_label("Cancel").click_accesskit();
    fx.harness.run_steps(3);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "templates" && r["request"]["action"] == "remove")
    );
    fx.harness
        .get_by_label("Remove personal document template")
        .click_accesskit();
    fx.wait_for_label("Remove template");
    fx.harness.get_by_label("Remove template").click_accesskit();
    fx.wait_for_label("Upload personal document template");
    assert!(fx.server.requests().iter().any(|r| r["cmd"] == "templates"
        && r["request"]["action"] == "remove"
        && r["request"]["account"] == "482139075"));
    // Same settings navigation switches native screens, without browser handoff.
    fx.harness.get_by_label("Models").click_accesskit();
    fx.wait_for_label("Save default model");
    fx.harness.get_by_label("Templates").click_accesskit();
    fx.wait_for_label("Upload personal document template");
}

#[test]
fn template_picker_results_are_ignored_after_the_page_or_account_changed() {
    let mut state = super::state::ChatState::default();
    state.list.account.id = "482139075".into();
    state.show_templates(None);
    let old = state.templates.as_ref().unwrap().generation;
    state.show_templates(Some(7));
    assert!(
        state
            .update(super::state::Msg::TemplatePicked(
                old,
                Ok(Some("old.docx".into()))
            ))
            .is_empty()
    );
    let old = state.templates.as_ref().unwrap().generation;
    state.new_chat();
    assert!(
        state
            .update(super::state::Msg::TemplatePicked(
                old,
                Ok(Some("old.docx".into()))
            ))
            .is_empty()
    );
}

#[test]
fn template_settings_review_screens() {
    let Ok(directory) = std::env::var("CWW_TEMPLATES_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.wait("template settings ready", |h| {
                h.state().chat_state().is_some_and(|s| s.ready())
            });
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            for (name, templates) in [("before", false), ("templates", true)] {
                if templates {
                    fx.harness.state_mut().set_page(Page::Templates);
                    fx.wait_for_label("Upload personal document template");
                    let file = fx
                        .server
                        .paths
                        .daemon
                        .data_dir
                        .join("Northwind letterhead.docx");
                    std::fs::write(&file, b"synthetic template").unwrap();
                    PICKED.with(|p| *p.borrow_mut() = Some(vec![file]));
                    fx.harness
                        .get_by_label("Upload personal document template")
                        .click_accesskit();
                    fx.wait_for_label("Replace personal document template");
                }
                fx.logos();
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(12);
                let path = std::path::Path::new(&directory).join(name);
                std::fs::create_dir_all(&path).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(path.join(format!(
                        "{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
        }
    }
}

#[test]
fn instruction_settings_review_screens() {
    let Ok(directory) = std::env::var("CWW_INSTRUCTIONS_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Options::default()
            });
            fx.wait_for_label("write your instructions");
            fx.harness
                .query_all_by_label("write your instructions")
                .next()
                .unwrap()
                .click_accesskit();
            fx.wait_for_label("Use a template");
            fx.chat().set_still(true);
            fx.chat().state.instructions.as_mut().unwrap().scroll_to =
                Some(cww::workspace::Layer::Organization);
            fx.harness.run_steps(5);
            fx.harness
                .event(egui::Event::PointerMoved(egui::pos2(width - 60.0, 300.0)));
            fx.harness.run_steps(2);
            fx.harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, 10000.0),
                modifiers: egui::Modifiers::NONE,
            });
            fx.harness.run_steps(12);
            for (name, scroll) in [("top", false), ("memory", true), ("memory-off", false)] {
                if name == "memory-off" {
                    fx.chat()
                        .state
                        .instructions
                        .as_mut()
                        .unwrap()
                        .memory
                        .as_mut()
                        .unwrap()
                        .enabled = false;
                }
                if scroll {
                    fx.harness.event(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: egui::TouchPhase::Move,
                        delta: egui::vec2(0.0, -10000.0),
                        modifiers: egui::Modifiers::NONE,
                    });
                    fx.harness.run_steps(12);
                }
                fx.logos();
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(8);
                let path = std::path::Path::new(&directory);
                std::fs::create_dir_all(path).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(path.join(format!(
                        "{name}-{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
                fx.harness
                    .event(egui::Event::PointerMoved(egui::pos2(width - 60.0, 300.0)));
            }
        }
    }
}

#[test]
fn project_instruction_review_screens() {
    let Ok(directory) = std::env::var("CWW_INSTRUCTIONS_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Options::default()
            });
            fx.wait_for_label("write your instructions");
            let mut page = super::workspace::Instructions::new(cww::workspace::Layer::Project);
            page.account = Some("482139075".into());
            page.project = Some(7);
            page.back_to_project = true;
            page.scroll_to = None;
            let generation = page.generation;
            fx.chat().state.instructions = Some(page);
            fx.chat().run(vec![super::state::Command::Instructions(
                generation,
                Some(7),
            )]);
            fx.wait_for_label("Upload project document template");
            fx.chat().set_still(true);
            fx.logos();
            for name in ["project-top", "project-templates"] {
                if name == "project-templates" {
                    fx.harness
                        .get_by_label("Upload project presentation template")
                        .scroll_to_me();
                    fx.harness.run_steps(12);
                }
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(8);
                let path = std::path::Path::new(&directory);
                std::fs::create_dir_all(path).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(path.join(format!(
                        "{name}-{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
        }
    }
}

#[test]
fn notification_click_waits_for_a_template_request_then_opens_the_chat() {
    let mut fx = fixture(Options::default());
    fx.wait_for_label("Vendor contract renewal");
    fx.harness.state_mut().set_page(Page::Templates);
    fx.wait_for_label("Upload personal document template");
    fx.chat().state.templates.as_mut().unwrap().busy = true;
    let status = fx.harness.state().shared().status().unwrap();
    fx.harness.state().shared().open_chat(notification_target(
        status.server.as_deref().unwrap(),
        "482139075",
        11,
    ));
    fx.harness.run_steps(5);
    assert!(fx.chat().state.templates.is_some());
    assert!(
        fx.harness
            .state()
            .shared()
            .open_chat
            .lock()
            .unwrap()
            .is_some()
    );
    fx.chat().state.templates.as_mut().unwrap().busy = false;
    fx.wait("notification opens after template work", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.open == Some(11) && s.templates.is_none() && !s.loading)
    });
}

#[test]
fn native_account_settings_save_names_and_copying_separately() {
    use cww::account_settings::CopyFormat;
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    let field = fx.harness.get_by_label("Full name");
    field.click();
    field.type_text(" Morgan");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Markdown").click_accesskit();
    fx.wait_for_label("Copying preference updated.");
    let request = fx.wait_for_request("account_settings", |r| {
        r["request"]["change"]["kind"] == "copy_format"
    });
    assert_eq!(request["request"]["account"], "482139075");
    assert_eq!(
        request["request"]["change"],
        serde_json::json!({"kind":"copy_format","copy_answers_as":"markdown"})
    );
    let state = &fx.chat().state;
    let screen = state.account_settings.as_ref().unwrap();
    assert_eq!(screen.draft.full_name, "Alex Rivera Morgan");
    assert!(screen.dirty());
    assert_eq!(state.list.copy_answers_as, CopyFormat::Markdown);
    assert!(super::super::visible_notification_chat(Page::Chat, true, Some(state)).is_none());
    fx.harness.get_by_label("Save profile").click_accesskit();
    fx.wait_for_label("Profile updated.");
    let request = fx.wait_for_request("account_settings", |r| {
        r["request"]["change"]["kind"] == "profile"
    });
    assert_eq!(
        request["request"]["change"]["full_name"],
        "Alex Rivera Morgan"
    );
    assert!(
        request["request"]["change"]
            .get("copy_answers_as")
            .is_none()
    );
    assert!(!fx.chat().state.account_settings.as_ref().unwrap().dirty());
    fx.harness.get_by_label("Rich text").click_accesskit();
    fx.wait_for_label("Copying preference updated.");
    assert_eq!(fx.chat().state.list.copy_answers_as, CopyFormat::RichText);
    fx.open_waiting(11, "Want me to add it to the Q4 checklist?");
    crate::clipboard::COPIED.with(|slot| *slot.borrow_mut() = None);
    fx.harness
        .query_all_by_label("Copy message")
        .last()
        .unwrap()
        .click_accesskit();
    fx.wait_for_label("Copied");
    let (html, markdown) =
        crate::clipboard::COPIED.with(|slot| slot.borrow_mut().take().expect("rich answer copy"));
    assert!(html.contains("<p>") && markdown.contains("Q4 checklist"));
}

#[test]
fn native_account_settings_guard_unsaved_notification_and_organization_navigation() {
    let mut fx = fixture(Options::default());
    fx.ready();
    let (profile, second) = add_organization(&mut fx);
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    let field = fx.harness.get_by_label("Full name");
    field.click();
    field.type_text(" draft");
    fx.harness.run_steps(2);
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard profile changes?");
    fx.harness.get_by_label("Keep editing").click_accesskit();
    fx.harness.run_steps(3);
    assert!(fx.harness.state().organizations.selected.is_none());
    assert!(fx.chat().state.account_settings.as_ref().unwrap().dirty());
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard profile changes?");
    fx.harness.get_by_label("Discard changes").click_accesskit();
    fx.wait("second organization", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "918273645" && s.open == Some(11) && !s.loading)
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "account_settings" && r["request"]["action"] == "update")
    );
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    fx.harness.get_by_label("Markdown").click_accesskit();
    fx.wait_for_label("Copying preference updated.");
    assert!(
        second
            .requests()
            .iter()
            .any(|r| r["cmd"] == "account_settings" && r["request"]["account"] == "918273645")
    );
}

#[test]
fn native_account_settings_reject_results_for_old_pages_and_clear_on_access_loss() {
    let mut state = super::state::ChatState::default();
    state.list.account.id = "482139075".into();
    state.show_account_settings();
    let old = state.account_settings.as_ref().unwrap().generation;
    state.show_account_settings();
    let current = state.account_settings.as_ref().unwrap().generation;
    let page: cww::account_settings::Page = serde_json::from_value(serde_json::json!({"account":{"id":"482139075","name":"Northwind"},"email":"alex@example.com","profile":{"full_name":"Alex","display_name":"Alex"},"copy_answers_as":"markdown"})).unwrap();
    assert!(
        state
            .update(super::state::Msg::AccountSettings(old, Ok(page.clone())))
            .is_empty()
    );
    assert!(state.account_settings.as_ref().unwrap().page.is_none());
    state.update(super::state::Msg::AccountSettings(
        current,
        Ok(page.clone()),
    ));
    assert!(state.account_settings.as_ref().unwrap().page.is_some());
    state.update(super::state::Msg::Listed(Err(
        cww::tui::chat::Failure::new("forbidden", "Access removed."),
    )));
    assert!(state.account_settings.is_none());
    assert!(
        state
            .update(super::state::Msg::AccountSettings(current, Ok(page)))
            .is_empty()
    );
}

#[test]
fn native_account_settings_review_screens() {
    let Ok(directory) = std::env::var("CWW_ACCOUNT_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                dark,
                size: [width, 900.0],
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(6);
            let suffix = format!("{}-{width}.png", if dark { "dark" } else { "light" });
            let before = std::path::Path::new(&directory).join("before");
            std::fs::create_dir_all(&before).unwrap();
            fx.harness
                .render()
                .expect("GPU rendering")
                .save(before.join(&suffix))
                .unwrap();
            // Narrow screens reach the same page via the drawer; direct event
            // here keeps the evidence focused on the settings layout.
            let commands = fx.chat().state.show_account_settings();
            fx.chat().run(commands);
            fx.wait_for_label("Save profile");
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            let after = std::path::Path::new(&directory).join("after");
            std::fs::create_dir_all(&after).unwrap();
            fx.harness
                .render()
                .expect("GPU rendering")
                .save(after.join(suffix))
                .unwrap();
        }
    }
}

#[test]
fn native_schedules_create_edit_pause_run_and_delete() {
    let mut fx = fixture(Options::default());
    fx.ready();
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    fx.harness.get_by_label("Schedules").click_accesskit();
    fx.wait_for_label("No schedules yet");
    fx.harness.get_by_label("Add schedule").click_accesskit();
    fx.wait_for_label("Create schedule");
    fx.harness.get_by_label("Create schedule").click_accesskit();
    fx.wait_for_label("Give the schedule a name and write what to ask.");
    let name = fx.harness.get_by_label("Name");
    name.click();
    name.type_text("Morning briefing");
    let prompt = fx.harness.get_by_label("Ask");
    prompt.click();
    prompt.type_text("Summarize the Q4 folder.");
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Once").click_accesskit();
    fx.wait_for_label("Date and time");
    fx.harness.get_by_label("Repeat").click_accesskit();
    fx.wait_for_label("Every");
    fx.harness.get_by_label("Create schedule").click_accesskit();
    fx.wait_for_label("Schedule saved.");
    let request = fx.wait_for_request("schedules", |r| r["request"]["action"] == "create");
    assert_eq!(request["request"]["account"], "482139075");
    assert_eq!(
        request["request"]["fields"]["prompt"],
        "Summarize the Q4 folder."
    );
    fx.harness
        .get_by_label("Pause Morning briefing")
        .click_accesskit();
    fx.wait_for_label("Resume");
    fx.harness.get_by_label("Resume").click_accesskit();
    fx.wait_for_label("Pause Morning briefing");
    fx.harness.get_by_label("Edit").click_accesskit();
    fx.wait_for_label("Save schedule");
    let name = fx.harness.get_by_label("Name");
    name.click();
    name.type_text(" updated");
    fx.harness.run_steps(3);
    fx.harness.get_by_label("Save schedule").click_accesskit();
    fx.wait_for_label("Morning briefing updated");
    fx.harness.get_by_label("Run now").click_accesskit();
    fx.wait("schedule run opens its chat", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.open == Some(11) && s.schedules.is_none() && !s.loading)
    });
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    fx.harness.get_by_label("Schedules").click_accesskit();
    fx.wait_for_label("Morning briefing updated");
    fx.harness
        .get_by_label("Delete Morning briefing updated")
        .click_accesskit();
    fx.wait_for_label("Delete schedule");
    fx.harness.get_by_label("Cancel").click_accesskit();
    fx.harness.run_steps(3);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "schedules" && r["request"]["action"] == "delete")
    );
    fx.harness
        .get_by_label("Delete Morning briefing updated")
        .click_accesskit();
    fx.wait_for_label("Delete schedule");
    fx.harness.get_by_label("Delete schedule").click_accesskit();
    fx.wait_for_label("No schedules yet");
}

#[test]
fn native_schedules_keep_drafts_on_refresh_and_guard_account_navigation() {
    use cww::schedules::Request;
    let mut fx = fixture(Options::default());
    fx.ready();
    let (profile, second) = add_organization(&mut fx);
    let commands = fx.chat().state.schedule_request(Request::New);
    fx.chat().run(commands);
    fx.wait_for_label("Create schedule");
    let name = fx.harness.get_by_label("Name");
    name.click();
    name.type_text("Keep this draft");
    fx.harness.run_steps(3);
    let commands = fx.chat().state.update(super::state::Msg::ListLive(
        cww::tui::chat::ListLive::Schedules,
    ));
    assert!(commands.is_empty());
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard schedule changes?");
    fx.harness.get_by_label("Keep editing").click_accesskit();
    fx.harness.run_steps(3);
    assert_eq!(
        fx.chat()
            .state
            .schedules
            .as_ref()
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .name,
        "Keep this draft"
    );
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard schedule changes?");
    fx.harness.get_by_label("Discard changes").click_accesskit();
    fx.wait("new organization's chat", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "918273645" && s.open == Some(11) && !s.loading)
    });
    assert!(
        !fx.server
            .requests()
            .iter()
            .chain(second.requests().iter())
            .any(|r| r["cmd"] == "schedules" && r["request"]["action"] == "create")
    );
}

#[test]
fn native_schedules_guard_page_identity_access_and_coalesce_live_refresh() {
    use super::state::{ChatState, Command, Msg};
    use cww::schedules::{Reply, Request};
    let mut state = ChatState::default();
    state.access = super::state::Access::Ready;
    state.list.account.id = "482139075".into();
    state.schedule_request(Request::List);
    let old = state.schedules.as_ref().unwrap().generation;
    state.new_chat();
    state.schedule_request(Request::List);
    let generation = state.schedules.as_ref().unwrap().generation;
    let page=serde_json::from_value(serde_json::json!({"account":{"id":"482139075","name":"Northwind"},"email":"alex@example.com","active_count":0,"limit":10,"schedules":[]})).unwrap();
    assert!(
        state
            .update(Msg::Schedules(old, Ok(Reply::Page(page))))
            .is_empty()
    );
    assert!(state.schedules.as_ref().unwrap().page.is_none());
    assert!(
        state
            .update(Msg::ListLive(cww::tui::chat::ListLive::Schedules))
            .is_empty()
    );
    let page=serde_json::from_value(serde_json::json!({"account":{"id":"482139075","name":"Northwind"},"email":"alex@example.com","active_count":0,"limit":10,"schedules":[]})).unwrap();
    assert!(matches!(
        state
            .update(Msg::Schedules(generation, Ok(Reply::Page(page))))
            .as_slice(),
        [Command::Schedules(_, Request::List)]
    ));
    let wrong=serde_json::from_value(serde_json::json!({"account":{"id":"918273645","name":"Other"},"email":"other@example.com","active_count":0,"limit":10,"schedules":[]})).unwrap();
    state.update(Msg::Schedules(generation, Ok(Reply::Page(wrong))));
    assert!(state.schedules.as_ref().unwrap().page.is_none());
    state.update(Msg::Listed(Err(cww::tui::chat::Failure::new(
        "forbidden",
        "Access removed.",
    ))));
    assert!(state.schedules.is_none());
}

#[test]
fn native_schedules_review_screens() {
    let Ok(directory) = std::env::var("CWW_SCHEDULES_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: Some(renderer),
                ..Options::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            fx.logos();
            let suffix = format!("{}-{width}.png", if dark { "dark" } else { "light" });
            for view in ["before", "empty", "form", "list"] {
                if view == "empty" {
                    let commands = fx
                        .chat()
                        .state
                        .schedule_request(cww::schedules::Request::List);
                    fx.chat().run(commands);
                    fx.wait_for_label("No schedules yet");
                }
                if view == "form" {
                    fx.harness.get_by_label("Add schedule").click_accesskit();
                    fx.wait_for_label("Create schedule");
                }
                if view == "list" {
                    {
                        let fields = fx
                            .chat()
                            .state
                            .schedules
                            .as_mut()
                            .unwrap()
                            .draft
                            .as_mut()
                            .unwrap();
                        fields.name = "Morning briefing".into();
                        fields.prompt = "Summarize the Q4 folder and today's meetings.".into();
                    }
                    let commands = fx.chat().state.schedules.as_mut().unwrap().save();
                    fx.chat().run(commands);
                    fx.wait_for_label("Morning briefing");
                }
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(12);
                let output = std::path::Path::new(&directory).join(view);
                std::fs::create_dir_all(&output).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(output.join(&suffix))
                    .unwrap();
            }
        }
    }
}

#[test]
fn native_schedule_date_and_time_controls_preserve_wall_clock_values() {
    let evidence = std::env::var("CWW_SCHEDULE_PICKERS_EVIDENCE").ok();
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: evidence
                    .as_ref()
                    .map(|_| egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Options::default()
            });
            fx.ready();
            let commands = fx
                .chat()
                .state
                .schedule_request(cww::schedules::Request::New);
            fx.chat().run(commands);
            fx.wait_for_label("Create schedule");
            {
                let draft = fx
                    .chat()
                    .state
                    .schedules
                    .as_mut()
                    .unwrap()
                    .draft
                    .as_mut()
                    .unwrap();
                draft.repeats = "once".into();
                draft.run_at = "2028-02-28T14:37".into();
            }
            fx.harness.run_steps(3);
            fx.harness.get_by_label("Choose date").click_accesskit();
            fx.wait_for_label("Save");
            if let Some(directory) = &evidence {
                std::fs::create_dir_all(directory).unwrap();
                fx.harness.run_steps(8);
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(std::path::Path::new(directory).join(format!(
                        "calendar-{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
            fx.harness.get_by_label("29").click_accesskit();
            fx.harness.run_steps(2);
            assert_eq!(
                fx.chat()
                    .state
                    .schedules
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .run_at,
                "2028-02-28T14:37",
                "calendar selection waits for Save"
            );
            fx.harness
                .query_all_by_label("Cancel")
                .last()
                .unwrap()
                .click_accesskit();
            fx.harness.run_steps(3);
            assert_eq!(
                fx.chat()
                    .state
                    .schedules
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .run_at,
                "2028-02-28T14:37",
                "canceling the picker leaves the schedule's date and time alone"
            );
            fx.harness.get_by_label("Choose date").click_accesskit();
            fx.wait_for_label("Save");
            fx.harness.get_by_label("29").click_accesskit();
            fx.harness.run_steps(2);
            fx.harness.get_by_label("Save").click_accesskit();
            fx.harness.run_steps(3);
            assert_eq!(
                fx.chat()
                    .state
                    .schedules
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .run_at,
                "2028-02-29T14:37",
                "leap-day selection preserves the entered wall-clock time"
            );
            fx.harness.get_by_label("Choose time").click_accesskit();
            fx.wait_for_label("Hour");
            if let Some(directory) = &evidence {
                fx.harness.run_steps(8);
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(std::path::Path::new(directory).join(format!(
                        "time-{}-{width}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
            fx.wait_for_label("Hour 15");
            fx.harness.get_by_label("Hour 15").click_accesskit();
            fx.harness.run_steps(3);
            fx.wait_for_label("Minute 38");
            fx.harness.get_by_label("Minute 38").click_accesskit();
            fx.harness.run_steps(3);
            assert_eq!(
                fx.chat()
                    .state
                    .schedules
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .run_at,
                "2028-02-29T15:38",
                "choosing a time preserves its date without a timezone conversion"
            );
            assert!(
                fx.server
                    .requests()
                    .iter()
                    .all(|r| r["cmd"] != "schedules" || r["request"]["action"] == "new"),
                "picker interaction never saves the schedule itself"
            );
        }
    }
}

#[test]
fn native_people_save_each_form_without_losing_other_drafts() {
    let mut fx = fixture(Options {
        size: [1280.0, 2600.0],
        ..Options::default()
    });
    fx.ready();
    fx.harness.get_by_label("Settings and usage").click();
    fx.wait_for_label("Save profile");
    fx.harness.get_by_label("People").click_accesskit();
    fx.wait_for_label("Guests see document titles");
    let field = fx.harness.get_by_label("Name");
    field.click();
    field.type_text(" Labs");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Email").click();
    fx.harness
        .get_by_label("Email")
        .type_text("guest@example.com");
    fx.harness.get_by_label("Guest").click_accesskit();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Launch plan")
        .click_accesskit();
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Send invitation").click_accesskit();
    fx.wait_for_label("Invitation sent.");
    let request = fx.wait_for_request("people", |r| r["request"]["action"] == "invite");
    assert_eq!(request["request"]["account"], "482139075");
    assert_eq!(
        request["request"]["fields"],
        serde_json::json!({"email":"guest@example.com","role":"guest","project_ids":[71]})
    );
    let screen = fx.chat().state.people.as_ref().unwrap();
    assert_eq!(screen.organization.name, "Northwind Labs");
    assert!(screen.dirty());
    assert_eq!(screen.invitation, cww::people::InvitationFields::default());
    fx.harness.get_by_label("Save").click_accesskit();
    fx.wait_for_label("Organization updated.");
    assert!(!fx.chat().state.people.as_ref().unwrap().dirty());
    fx.harness.get_by_label("Role for Sam Lee").click();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_role_and_label(egui::accesskit::Role::Button, "Admin")
        .click_accesskit();
    fx.wait_for_label("Role updated.");
    let role = fx.wait_for_request("people", |r| r["request"]["action"] == "role");
    assert_eq!(role["request"]["role"], "admin");
    fx.harness
        .get_by_label("Withdraw invitation to guest@example.com")
        .click_accesskit();
    fx.wait_for_label("Invitation withdrawn.");
    assert!(
        fx.chat()
            .state
            .people
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .invitations
            .as_ref()
            .unwrap()
            .is_empty()
    );
    fx.harness
        .get_by_label("Reset the join link")
        .click_accesskit();
    fx.wait_for_label("Reset link");
    fx.harness.get_by_label("Cancel").click_accesskit();
    fx.harness.run_steps(3);
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "people" && r["request"]["action"] == "reset_join_link")
    );
    fx.harness
        .get_by_label("Reset the join link")
        .click_accesskit();
    fx.wait_for_label("Reset link");
    fx.harness.get_by_label("Reset link").click_accesskit();
    fx.wait_for_label("Join link reset.");
    assert!(
        fx.chat()
            .state
            .people
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .join_link
            .ends_with("synthetic-reset")
    );
    fx.harness.get_by_label("Remove Sam Lee").click_accesskit();
    fx.wait_for_label("Remove");
    fx.harness.get_by_label("Remove").click_accesskit();
    fx.wait_for_label("Person removed.");
    assert_eq!(
        fx.chat()
            .state
            .people
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .people
            .len(),
        1
    );
}

#[test]
fn native_people_guard_unsaved_and_stale_account_navigation() {
    let mut fx = fixture(Options::default());
    fx.ready();
    let (profile, _second) = add_organization(&mut fx);
    let commands = fx.chat().state.show_people();
    fx.chat().run(commands);
    fx.wait_for_label("Guests see document titles");
    fx.chat().state.people.as_mut().unwrap().invitation.email = "unsent@example.com".into();
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard People changes?");
    fx.harness.get_by_label("Keep editing").click_accesskit();
    fx.harness.run_steps(3);
    assert!(fx.chat().state.people.as_ref().unwrap().dirty());
    assert!(
        super::super::visible_notification_chat(Page::Chat, true, Some(&fx.chat().state)).is_none()
    );
    fx.harness.state().shared().open_chat(notification_target(
        &profile.server.url,
        &profile.account.id,
        11,
    ));
    fx.wait_for_label("Discard People changes?");
    fx.harness.get_by_label("Discard changes").click_accesskit();
    fx.wait("the other organization", |h| {
        h.state()
            .chat_state()
            .is_some_and(|s| s.list.account.id == "918273645" && s.open == Some(11) && !s.loading)
    });
    assert!(fx.chat().state.people.is_none());
    assert!(
        !fx.server
            .requests()
            .iter()
            .any(|r| r["cmd"] == "people" && r["request"]["action"] != "list")
    );
}

#[test]
fn native_people_bind_responses_and_departure_to_the_current_request() {
    use super::state::{ChatState, Msg};
    use cww::people::{Reply, Request};
    let mut state = ChatState::default();
    state.list.account.id = "482139075".into();
    state.show_people();
    let old = state.people.as_ref().unwrap().generation;
    state.show_people();
    let current = state.people.as_ref().unwrap().generation;
    let page: Reply = serde_json::from_value(demo::people_page()).unwrap();
    state.update(Msg::People(old, Ok(page.clone())));
    assert!(state.people.as_ref().unwrap().page.is_none());
    state.update(Msg::People(current, Ok(page.clone())));
    assert!(state.people.as_ref().unwrap().page.is_some());
    let screen = state.people.as_mut().unwrap();
    screen.organization.name = "draft".into();
    screen.invitation.email = "draft@example.com".into();
    screen.request(Request::Invite {
        account: "482139075".into(),
        fields: screen.invitation.clone(),
    });
    screen.loaded(Err(cww::tui::chat::Failure::new("invalid", "Bad email")));
    assert!(screen.dirty());
    let mut denied = demo::people_page();
    denied["admin"] = false.into();
    screen.loaded(Ok(serde_json::from_value(denied).unwrap()));
    assert!(!screen.dirty());
    assert!(screen.invitation.email.is_empty());
    let left:Reply=serde_json::from_value(serde_json::json!({"account":{"id":"482139075","name":"Northwind"},"left":true,"notice":"You left Northwind."})).unwrap();
    assert!(screen.loaded(Ok(left.clone())).is_none());
    assert!(screen.page.is_none());
    screen.loaded(Ok(page));
    screen.request(Request::Remove {
        account: "482139075".into(),
        id: 1,
    });
    let commands = state.update(Msg::People(current, Ok(left)));
    assert!(commands.contains(&super::state::Command::ResetAccount));
    assert!(state.people.is_none());
    assert!(state.list.chats.is_empty());
    state.list.account.id = "482139075".into();
    state.show_people();
    let current = state.people.as_ref().unwrap().generation;
    state.update(Msg::People(
        current,
        Err(cww::tui::chat::Failure::new("revoked", "Access removed.")),
    ));
    assert!(state.people.as_ref().unwrap().page.is_none());
}

#[test]
fn native_people_review_screens() {
    let Ok(directory) = std::env::var("CWW_PEOPLE_EVIDENCE") else {
        return;
    };
    for dark in [false, true] {
        for width in [390.0, 1280.0] {
            let mut fx = fixture(Options {
                dark,
                size: [width, 1000.0],
                renderer: Some(egui_kittest::wgpu::WgpuTestRenderer::new()),
                ..Options::default()
            });
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().set_greeting(4);
            fx.logos();
            let suffix = format!("{}-{width}.png", if dark { "dark" } else { "light" });
            for view in ["before", "top", "middle", "bottom"] {
                if view == "top" {
                    let commands = fx.chat().state.show_people();
                    fx.chat().run(commands);
                    fx.wait_for_label("Guests see document titles");
                }
                if view == "middle" || view == "bottom" {
                    fx.harness
                        .event(egui::Event::PointerMoved(egui::pos2(width - 40.0, 800.0)));
                    fx.harness.event(egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -650.0),
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                fx.harness.event(egui::Event::PointerGone);
                fx.harness.run_steps(16);
                let path = std::path::Path::new(&directory).join(view);
                std::fs::create_dir_all(&path).unwrap();
                fx.harness
                    .render()
                    .expect("GPU rendering")
                    .save(path.join(&suffix))
                    .unwrap();
            }
        }
    }
}

fn command(id: &str, line: &str, state: &str, exit_code: Option<i32>, ms: u64) -> Value {
    serde_json::json!({
        "id": id, "state": state, "exit_code": exit_code, "duration_ms": ms,
        "output_bytes": 120, "error": null, "root": "app", "folder": "App",
        "dir": "api", "command": line, "network": false, "started_at": 1_792_217_040u64,
    })
}

fn run_sample_commands(server: &demo::Server) {
    server.run_commands(vec![
        (
            command("aaaaaaaaaaaa", "bin/rails test", "running", None, 42_000),
            "Running 214 tests in parallel using 8 processes\n\
             Run options: --seed 41207\n\n# Running:\n\n\
             \u{1b}[32m..................................................\u{1b}[0m\n\
             ..........F.........\n",
        ),
        (
            command("bbbbbbbbbbbb", "npm run build", "exited", Some(0), 8_100),
            "vite v7.1.4 building for production...\n✓ 412 modules transformed.\nbuilt in 8.10s\n",
        ),
        (
            command("cccccccccccc", "bin/rubocop", "exited", Some(1), 3_400),
            "app/models/chat.rb:12:5: C: Layout/IndentationWidth\n",
        ),
    ]);
}

/// A command Chat with Work runs on this computer shows over the composer
/// with what it is and where; Show output lists them all with what the
/// chosen one prints, and Stop stops it, through the daemon here.
#[test]
fn commands_running_here_show_over_the_composer() {
    let mut fx = fixture(Options::default());
    run_sample_commands(&fx.server);
    fx.ready();
    fx.chat().open(11);
    fx.wait_for_label("bin/rails test · App › api · 42s");
    fx.harness.get_by_label("Show output").click();
    fx.wait_for_request("command_output", |r| r["run"] == "aaaaaaaaaaaa");
    fx.wait_for_label("..........F.........");
    assert!(
        fx.harness
            .query_all_by_label_contains("\u{1b}")
            .next()
            .is_none(),
        "escape sequences never reach the page"
    );
    fx.harness.get_by_label("Stop").click();
    fx.wait_for_request("command_stop", |r| r["run"] == "aaaaaaaaaaaa");
    fx.harness.get_by_label("npm run build").click();
    fx.wait_for_request("command_output", |r| r["run"] == "bbbbbbbbbbbb");
    fx.wait_for_label("built in 8.10s");
    fx.harness.get_by_label("Hide commands").click();
    fx.harness.run_steps(2);
    assert!(fx.harness.query_by_label("Hide commands").is_none());
}

/// The commands line and the panel, light and dark, against the snapshots
/// (compared on Linux, where they were made).
#[test]
fn commands_render_like_the_web() {
    for dark in [false, true] {
        for (name, open) in [("chat-command-running", false), ("chat-commands", true)] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                eprintln!("no wgpu adapter here: skipping the snapshots");
                return;
            };
            let size = [1280.0, 820.0];
            let mut fx = fixture(Options {
                size,
                dark,
                renderer: Some(renderer),
                ..Options::default()
            });
            run_sample_commands(&fx.server);
            fx.ready();
            fx.chat().set_still(true);
            fx.chat().open(11);
            fx.wait("the transcript", |h| {
                h.state()
                    .chat_state()
                    .is_some_and(|s| s.transcript.is_some())
            });
            fx.wait_for_label("bin/rails test · App › api · 42s");
            if open {
                fx.harness.get_by_label("Show output").click();
                fx.wait_for_label("..........F.........");
            }
            fx.logos();
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(12);
            let theme = if dark { "dark" } else { "light" };
            let image = fx.harness.render().expect("rendering");
            let options = egui_kittest::SnapshotOptions::new()
                .threshold(2.0)
                .max_failed_pixels(((size[0] * size[1]) * 0.01) as usize);
            if cfg!(target_os = "linux") {
                egui_kittest::image_snapshot_options(&image, format!("{name}-{theme}"), &options);
            } else {
                assert!(image.width() > 0);
            }
        }
    }
}
