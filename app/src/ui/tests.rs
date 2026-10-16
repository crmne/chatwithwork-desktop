//! The settings window, driven through its accessibility tree against the
//! stand-in daemon, which records every request it gets.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::Value;

use super::theme::Theme;
use super::{AppState, Page, SettingsApp, Shared};
use crate::control::Client;
use crate::demo::{self, Scenario};
use crate::model::Platform;

thread_local! {
    /// URLs the window asked to open, instead of opening a browser.
    pub static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct Fixture {
    server: demo::Server,
    harness: Harness<'static, SettingsApp>,
    documents: PathBuf,
}

fn fixture(scenario: Scenario, page: Option<Page>) -> Fixture {
    fixture_with_size(scenario, page, [840.0, 700.0])
}

fn fixture_with_size(scenario: Scenario, page: Option<Page>, size: [f32; 2]) -> Fixture {
    fixture_rendered(scenario, page, size, false, None)
}

fn fixture_rendered(
    scenario: Scenario,
    page: Option<Page>,
    size: [f32; 2],
    dark: bool,
    renderer: Option<egui_kittest::wgpu::WgpuTestRenderer>,
) -> Fixture {
    let server = demo::start(scenario).unwrap();
    if scenario == Scenario::Sample {
        AppState {
            onboarded: true,
            ..Default::default()
        }
        .save(&server.paths);
    }
    let documents = server
        .paths
        .daemon
        .config_dir
        .parent()
        .unwrap()
        .join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    let shared = Shared::new(
        Client::new(server.paths.socket_path()),
        server.paths.clone(),
        server.account(),
    );
    let watcher = Arc::clone(&shared);
    shared
        .client
        .spawn_watch(move |status| watcher.set_status(status));
    let mut app = SettingsApp::new(shared, Theme::new(Platform::current(), dark, None));
    app.set_documents(Some(documents.clone()));
    if let Some(page) = page {
        app.set_page(page);
    }
    let mut builder = Harness::builder().with_size(size);
    if let Some(renderer) = renderer {
        builder = builder.renderer(renderer);
    }
    let harness = builder.build_ui_state(|ui, app: &mut SettingsApp| app.show(ui), app);
    let mut fixture = Fixture {
        server,
        harness,
        documents,
    };
    fixture.wait("the first status", |h| h.state().shared.is_known());
    fixture
}

impl Fixture {
    /// Step the window until `done` holds; requests and status lines
    /// arrive from other threads.
    fn wait(&mut self, what: &str, done: impl Fn(&Harness<'static, SettingsApp>) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
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
            h.query_by_label_contains(&label).is_some()
        });
    }

    /// The requests that changed something (not status, subscribe, the log or the deny list).
    fn changes(&self) -> Vec<Value> {
        self.server
            .requests()
            .into_iter()
            .filter(|r| !matches!(r["cmd"].as_str(), Some("subscribe" | "audit_tail" | "deny")))
            .collect()
    }

    fn wait_for_request(&mut self, cmd: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = self.changes().into_iter().find(|r| r["cmd"] == cmd) {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "no {cmd} request: {:?}",
                self.changes()
            );
            self.harness.step();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn notification_preferences_save_for_the_loaded_organization_and_link_only_on_request() {
    OPENED.with(|urls| urls.borrow_mut().clear());
    let mut fx = fixture_with_size(
        Scenario::Sample,
        Some(Page::Notifications),
        [1080.0, 1200.0],
    );
    fx.wait_for_label("Answers Push");
    fx.harness.get_by_label("Answers Push").click();
    fx.harness
        .get_by_role_and_label(
            egui::accesskit::Role::CheckBox,
            "Show the chat and its answer",
        )
        .click();
    assert!(
        fx.server
            .requests()
            .iter()
            .all(|request| request["cmd"] != "notification_preferences_update")
    );
    fx.harness.get_by_label("Save preferences").click();
    let saved = fx.wait_for_request("notification_preferences_update");
    assert_eq!(saved["preferences"]["account"], "482139075");
    assert_eq!(
        saved["preferences"]["chat_notifications"]["answered"]["push"],
        false
    );
    assert_eq!(
        saved["preferences"]["chat_notifications"]["waiting"]["email"],
        true
    );
    assert_eq!(saved["preferences"]["notification_content"], false);
    fx.wait_for_label("Preferences saved.");
    assert!(OPENED.with(|urls| urls.borrow().is_empty()));
    fx.harness.get_by_label("Link this browser…").click();
    fx.wait_for_request("notification_browser_link");
    fx.wait_for_label("Confirm that this browser is on this computer.");
    let urls = OPENED.with(|urls| urls.borrow().clone());
    assert_eq!(urls.len(), 1);
    assert!(urls[0].contains("/482139075/native/notification_browser_link?token="));
}

#[test]
fn native_permission_is_requested_only_after_an_explicit_click() {
    let mut fx = fixture_with_size(
        Scenario::Sample,
        Some(Page::Notifications),
        [1080.0, 1300.0],
    );
    fx.server.set_notification_permission("not_determined");
    fx.wait_for_label("Turn on native notifications…");
    assert!(
        fx.server
            .requests()
            .iter()
            .all(|request| request["cmd"] != "notification_permission")
    );
    fx.harness
        .get_by_label("Turn on native notifications…")
        .click();
    assert_eq!(
        fx.wait_for_request("notification_permission")["request"],
        true
    );
}

#[test]
fn renders_notification_preferences_at_wide_and_narrow_sizes() {
    for dark in [false, true] {
        for (size_name, size) in [("wide", [1080.0, 1200.0]), ("narrow", [700.0, 820.0])] {
            let Ok(renderer) = std::panic::catch_unwind(egui_kittest::wgpu::WgpuTestRenderer::new)
            else {
                eprintln!("no wgpu adapter here: skipping notification snapshots");
                return;
            };
            let mut fx = fixture_rendered(
                Scenario::Sample,
                Some(Page::Notifications),
                size,
                dark,
                Some(renderer),
            );
            fx.wait_for_label("Answers Push");
            fx.wait_for_label("Chat with Work can send notifications on this computer");
            fx.harness.event(egui::Event::PointerGone);
            fx.harness.run_steps(8);
            let theme = if dark { "dark" } else { "light" };
            for position in ["top", "bottom"] {
                if position == "bottom" {
                    fx.harness.get_by_label("Link this browser…").scroll_to_me();
                    fx.harness.run_steps(8);
                }
                let image = fx.harness.render().expect("GPU rendering");
                let name = format!("notifications-{size_name}-{theme}-{position}");
                if let Some(directory) = std::env::var_os("CWW_NOTIFICATION_EVIDENCE") {
                    image
                        .save(std::path::Path::new(&directory).join(format!("{name}.png")))
                        .unwrap();
                }
                if cfg!(target_os = "linux") {
                    egui_kittest::image_snapshot_options(
                        &image,
                        name,
                        &egui_kittest::SnapshotOptions::new()
                            .threshold(2.0)
                            .max_failed_pixels(1000),
                    );
                }
            }
        }
    }
}

#[test]
fn nothing_is_shared_until_the_user_says_so() {
    let mut fx = fixture(Scenario::Fresh, None);
    assert_eq!(fx.harness.state().page(), Page::Welcome, "first run");
    fx.wait_for_label("It's running in the background");
    fx.harness
        .get_by_label_contains("Share your Documents folder");
    // Plenty of frames, and still nothing shared.
    fx.harness.run_steps(30);
    assert!(fx.changes().is_empty(), "{:?}", fx.changes());

    // The setup page can be taller than the window. Bring its last card
    // into view before clicking, just as a user would.
    fx.harness.hover_at(egui::pos2(420.0, 600.0));
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -3.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(60);
    fx.harness.get_by_label("Share Documents").click();
    let added = fx.wait_for_request("roots_add");
    assert_eq!(added["path"], serde_json::json!(fx.documents));
    fx.wait_for_label("Sharing Documents.");
    assert_eq!(fx.changes().len(), 1, "one folder, once");
}

#[test]
fn pairing_shows_the_code_and_can_reopen_the_page() {
    OPENED.with(|o| o.borrow_mut().clear());
    let mut fx = fixture(Scenario::Fresh, Some(Page::Account));
    fx.wait_for_label("This computer isn't paired");
    fx.harness.get_by_label("Pair with Chat with Work…").click();
    fx.wait_for_request("pair");
    fx.wait_for_label("WDJB-MJHT");
    // `cww login` opens the page; the window only offers to open it again.
    assert!(OPENED.with(|o| o.borrow().is_empty()));
    fx.harness.get_by_label("Open Page Again").click();
    fx.harness.run_steps(2);
    let opened = OPENED.with(|o| o.borrow().clone());
    assert_eq!(
        opened,
        ["https://chatwithwork.com/device?user_code=WDJB-MJHT"]
    );

    fx.harness.get_by_label("Cancel").click();
    fx.wait_for_request("pair_cancel");
    fx.wait_for_label("This computer isn't paired");
}

#[test]
fn a_self_hosted_server_is_never_left_blank() {
    let mut fx = fixture(Scenario::Fresh, Some(Page::Account));
    fx.wait_for_label("This computer isn't paired");
    fx.harness.get_by_label("Use a self-hosted server").click();
    fx.harness.run_steps(2);
    fx.harness
        .get_by_label_contains("Enter your server's address");
    // Blank: the button does nothing, rather than pair with chatwithwork.com.
    fx.harness.get_by_label("Pair with Chat with Work…").click();
    fx.harness.run_steps(10);
    assert!(
        !fx.changes().iter().any(|r| r["cmd"] == "pair"),
        "{:?}",
        fx.changes()
    );

    let field = fx.harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text("  https://chat.example.com ");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Pair with Chat with Work…").click();
    let pair = fx.wait_for_request("pair");
    assert_eq!(pair["server"], "https://chat.example.com");
}

#[test]
fn folders_are_renamed_and_removed_with_confirmation() {
    let mut fx = fixture(Scenario::Sample, Some(Page::Folders));
    fx.wait_for_label("Indexed · 1,532 files");

    fx.harness
        .get_all_by_label("Rename…")
        .next()
        .unwrap()
        .click();
    fx.harness.run_steps(2);
    let field = fx.harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text(" and notes");
    fx.harness.run_steps(2);
    fx.harness.get_by_label("Save").click();
    let renamed = fx.wait_for_request("roots_label");
    assert_eq!(renamed["root"], "documents");
    assert_eq!(renamed["label"], "Documents and notes");
    fx.wait_for_label("Documents and notes");

    // Removing asks first.
    fx.harness
        .get_all_by_label("Remove…")
        .next()
        .unwrap()
        .click();
    fx.harness.run_steps(3);
    assert!(!fx.changes().iter().any(|r| r["cmd"] == "roots_remove"));
    fx.harness
        .get_by_label_contains("Stop sharing “Documents and notes”?");
    fx.harness.get_by_label("Stop Sharing").click();
    let removed = fx.wait_for_request("roots_remove");
    assert_eq!(removed["root"], "documents");
    fx.wait("the folder to go", |h| {
        h.query_by_label_contains("Documents and notes").is_none()
    });
}

#[test]
fn allowing_changes_asks_first_and_stopping_does_not() {
    let mut fx = fixture(Scenario::Sample, Some(Page::Folders));
    fx.wait_for_label("Indexed · 1,532 files");
    let switches = || {
        fx.harness
            .get_all_by_role_and_label(egui::accesskit::Role::CheckBox, "Allow changes")
            .map(|n| n.accesskit_node().toggled())
            .collect::<Vec<_>>()
    };
    // Documents is read-only, Work projects allows changes.
    assert_eq!(
        switches(),
        [
            Some(egui::accesskit::Toggled::False),
            Some(egui::accesskit::Toggled::True)
        ]
    );
    fx.harness
        .get_all_by_role_and_label(egui::accesskit::Role::CheckBox, "Allow changes")
        .next()
        .unwrap()
        .click();
    fx.harness.run_steps(3);
    assert!(!fx.changes().iter().any(|r| r["cmd"] == "roots_writable"));
    fx.harness
        .get_by_label_contains("Let Chat with Work change files in “Documents”?");
    fx.harness.get_by_label("Allow Changes").click();
    let allowed = fx.wait_for_request("roots_writable");
    assert_eq!(allowed["root"], "documents");
    assert_eq!(allowed["writable"], true);

    // Making a folder read-only again needs no question.
    fx.wait("the folder to allow changes", |h| {
        h.get_all_by_role_and_label(egui::accesskit::Role::CheckBox, "Allow changes")
            .all(|n| n.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
            && !h.state().folders_busy()
    });
    fx.harness
        .get_all_by_role_and_label(egui::accesskit::Role::CheckBox, "Allow changes")
        .nth(1)
        .unwrap()
        .click();
    fx.wait("the second request", |h| h.state().shared.is_known());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let requests: Vec<Value> = fx
            .changes()
            .into_iter()
            .filter(|r| r["cmd"] == "roots_writable")
            .collect();
        if requests.len() == 2 {
            assert_eq!(requests[1]["root"], "work-projects");
            assert_eq!(requests[1]["writable"], false);
            break;
        }
        assert!(Instant::now() < deadline, "{requests:?}");
        fx.harness.step();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_deny_list_and_the_activity_log_are_shown() {
    let mut fx = fixture(Scenario::Sample, Some(Page::Privacy));
    fx.wait_for_label(".ssh");
    fx.harness.get_by_label_contains("*.secret");

    fx.harness.get_by_label("Activity").click();
    fx.wait_for_label("Read documents:Plans/Budget 2027.xlsx");
    fx.harness
        .get_by_label_contains("Read work-projects:Falcon/.env");
    // The filter, not the "Refused" badges on the rows.
    fx.harness
        .get_all_by_label("Refused")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::Button)
        .expect("the Refused filter")
        .click();
    fx.wait("the refused filter", |h| {
        h.query_by_label_contains("Read documents:Plans/Budget 2027.xlsx")
            .is_none()
    });
    fx.harness
        .get_by_label_contains("Read work-projects:Falcon/.env");
}

#[test]
fn pausing_is_a_switch_that_screen_readers_can_read() {
    let mut fx = fixture(Scenario::Sample, Some(Page::General));
    fx.wait_for_label("Running · version 0.1.0");
    let switch = fx
        .harness
        .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Pause sharing");
    assert_eq!(
        switch.accesskit_node().role(),
        egui::accesskit::Role::CheckBox
    );
    assert_eq!(
        switch.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::False)
    );
    switch.click();
    let paused = fx.wait_for_request("pause");
    assert_eq!(paused["cmd"], "pause");
    fx.wait_for_label("Paused");
}

#[test]
fn a_stopped_agent_can_be_started_from_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = crate::paths::Paths::under(tmp.path());
    AppState {
        onboarded: true,
        ..Default::default()
    }
    .save(&paths);
    let shared = Shared::new(
        Client::new(paths.socket_path()),
        paths,
        Arc::new(crate::pairing::Cli),
    );
    shared.set_status(None);
    let app = SettingsApp::new(shared, Theme::new(Platform::current(), true, None));
    let mut harness = Harness::builder()
        .with_size([840.0, 600.0])
        .build_ui_state(|ui, app: &mut SettingsApp| app.show(ui), app);
    harness.run_steps(3);
    harness.get_by_label_contains("The Local Agent isn't running");
    harness.get_by_label("Start Local Agent");
    harness.get_by_label_contains("Not running");
}

fn welcome_harness(
    size: [f32; 2],
    platform: Platform,
    dark: bool,
    renderer: Option<egui_kittest::wgpu::WgpuTestRenderer>,
) -> (tempfile::TempDir, Harness<'static, SettingsApp>) {
    // The shell normally enables caption support before opening a window.
    fastframe_titlebar::set_enabled(true);
    let tmp = tempfile::tempdir().unwrap();
    let paths = crate::paths::Paths::under(tmp.path());
    let shared = Shared::new(
        Client::new(paths.socket_path()),
        paths,
        Arc::new(crate::pairing::Cli),
    );
    shared.set_status(None);
    let app = SettingsApp::new(shared, Theme::new(platform, dark, None));
    let mut builder = Harness::builder().with_size(size);
    if let Some(renderer) = renderer {
        builder = builder.renderer(renderer);
    }
    let mut harness = builder.build_ui_state(
        |ui, app: &mut SettingsApp| {
            let screen = ui.ctx().content_rect();
            let mut window = ui.new_child(egui::UiBuilder::new().max_rect(screen));
            window.set_clip_rect(screen);
            app.show(&mut window);
        },
        app,
    );
    harness.run_steps(5);
    (tmp, harness)
}

#[test]
fn welcome_is_centered_at_each_window_size() {
    for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
        for dark in [false, true] {
            for size in [[660.0, 460.0], [840.0, 620.0], [1100.0, 760.0]] {
                let (_tmp, harness) = welcome_harness(size, platform, dark, None);
                let title = harness.get_by_label("Welcome to Chat with Work").rect();
                assert!(
                    (title.center().x - size[0] / 2.0).abs() < 1.0,
                    "welcome title is not centered at {size:?}: {title:?}"
                );
                let start = harness.get_by_label("Start Local Agent").rect();
                assert!(
                    start.left() >= 0.0 && start.right() <= size[0],
                    "startup button is clipped at {size:?}: {start:?}"
                );
                let step = harness.get_by_label("Run the Local Agent").rect();
                assert!(
                    step.left() < title.left(),
                    "step content should remain left-aligned"
                );
            }
        }
    }
}

#[test]
#[cfg(not(target_os = "macos"))]
fn seamless_window_controls_keep_the_agent_in_the_tray() {
    for platform in [Platform::Windows, Platform::Linux] {
        let (_tmp, mut harness) = welcome_harness([840.0, 620.0], platform, false, None);
        harness.state_mut().custom_titlebar = true;
        harness.run_steps(2);
        harness.get_by_label("Minimize window").click();
        harness.step();
        assert!(
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Minimized(true))
        );
        harness.get_by_label("Maximize window").click();
        harness.step();
        assert!(
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Maximized(true))
        );
        harness.get_by_label("Close window").click();
        harness.step();
        assert!(
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        harness
            .input_mut()
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        harness.step();
        assert!(
            harness
                .state()
                .shared
                .close_requested
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
    }
}

#[test]
fn macos_keeps_its_native_traffic_lights() {
    let (_tmp, harness) = welcome_harness([840.0, 620.0], Platform::MacOs, false, None);
    assert!(
        harness.query_by_label("Close window").is_none(),
        "macOS uses its native traffic lights"
    );
}

#[test]
fn titlebar_defaults_follow_the_platform_and_explicit_choices_persist() {
    let old: AppState = serde_json::from_str(r#"{"onboarded":true}"#).unwrap();
    assert!(!old.custom_titlebar(Platform::Linux));
    assert!(old.custom_titlebar(Platform::Windows));
    assert!(old.custom_titlebar(Platform::MacOs));
    let tmp = tempfile::tempdir().unwrap();
    let paths = crate::paths::Paths::under(tmp.path());
    for choice in [false, true] {
        AppState {
            custom_titlebar: Some(choice),
            ..old.clone()
        }
        .save(&paths);
        let saved = AppState::load(&paths);
        assert!(saved.onboarded);
        for platform in [Platform::Linux, Platform::Windows, Platform::MacOs] {
            assert_eq!(saved.custom_titlebar(platform), choice);
        }
    }
}

#[test]
fn linux_uses_desktop_decorations_unless_the_person_chooses_custom() {
    let (_tmp, mut harness) = welcome_harness([840.0, 620.0], Platform::Linux, false, None);
    assert!(!harness.state().custom_titlebar);
    assert!(harness.query_by_label("Close window").is_none());
    harness.state_mut().set_page(Page::General);
    harness.run_steps(3);
    harness
        .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Custom title bar")
        .click();
    harness.run_steps(2);
    let app = harness.state();
    assert!(
        !app.custom_titlebar,
        "the open window keeps its original decorations"
    );
    let reopened = SettingsApp::new(Arc::clone(&app.shared), app.theme);
    assert!(
        reopened.custom_titlebar,
        "reopening uses the saved preference"
    );
    harness
        .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Custom title bar")
        .click();
    harness.run_steps(2);
    let app = harness.state();
    let reopened = SettingsApp::new(Arc::clone(&app.shared), app.theme);
    assert!(!reopened.custom_titlebar);
}

#[test]
#[cfg(not(target_os = "macos"))]
fn seamless_window_can_be_resized_at_every_edge_and_corner() {
    use egui::{ResizeDirection as D, ViewportCommand};
    for (position, direction) in [
        ([1.0, 1.0], D::NorthWest),
        ([420.0, 1.0], D::North),
        ([839.0, 1.0], D::NorthEast),
        ([1.0, 310.0], D::West),
        ([839.0, 310.0], D::East),
        ([1.0, 619.0], D::SouthWest),
        ([420.0, 619.0], D::South),
        ([839.0, 619.0], D::SouthEast),
    ] {
        let (_tmp, mut harness) = welcome_harness([840.0, 620.0], Platform::Windows, false, None);
        harness.hover_at(position.into());
        harness.step();
        harness.drag_at(position.into());
        harness.step();
        assert!(
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&ViewportCommand::BeginResize(direction)),
            "resize at {position:?}"
        );
        assert!(
            !harness
                .state()
                .shared
                .close_requested
                .load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}

#[test]
#[cfg(not(target_os = "macos"))]
fn seamless_header_drags_and_fullscreen_has_no_caption_controls() {
    let (_tmp, mut harness) = welcome_harness([840.0, 620.0], Platform::Windows, false, None);
    harness.hover_at(egui::pos2(300.0, 20.0));
    harness.step();
    harness.drag_at(egui::pos2(300.0, 20.0));
    harness.step();
    harness.hover_at(egui::pos2(330.0, 20.0));
    harness.step();
    assert!(
        harness.output().viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::StartDrag)
    );
    harness.drop_at(egui::pos2(330.0, 20.0));
    harness
        .input_mut()
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .fullscreen = Some(true);
    harness.run_steps(2);
    assert!(harness.query_by_label("Close window").is_none());
}

/// Synthetic first-run images, rendered only when explicitly requested.
#[test]
fn renders_welcome_review() {
    let Some(dir) = std::env::var_os("CWW_WELCOME_REVIEW_DIR").map(PathBuf::from) else {
        return;
    };
    let _ = env_logger::builder().is_test(true).try_init();
    std::fs::create_dir_all(&dir).unwrap();
    for (platform, name) in [
        (Platform::Windows, "windows"),
        (Platform::MacOs, "macos"),
        (Platform::Linux, "linux"),
    ] {
        for dark in [false, true] {
            for (size, label) in [
                ([840.0, 620.0 + super::HEADER_HEIGHT], "normal"),
                ([1100.0, 760.0], "wide"),
            ] {
                let renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
                let (_tmp, mut harness) = welcome_harness(size, platform, dark, Some(renderer));
                let theme = if dark { "dark" } else { "light" };
                harness
                    .render()
                    .unwrap()
                    .save(dir.join(format!("{name}-{label}-{theme}.png")))
                    .unwrap();
            }
        }
    }
}

/// A wheel notch scrolls as far as it does in other apps (fastframe-scroll's
/// step), not egui's 40 points.
#[test]
fn a_wheel_notch_scrolls_as_far_as_other_apps() {
    let mut fx = fixture(Scenario::Sample, Some(Page::Activity));
    let row = "Read documents:Plans/Budget 2027.xlsx";
    fx.wait_for_label(row);
    let before = fx.harness.get_by_label_contains(row).rect().top();
    fx.harness
        .hover_at(fx.harness.get_by_label_contains(row).rect().center());
    fx.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    fx.harness.run_steps(60);
    let moved = before - fx.harness.get_by_label_contains(row).rect().top();
    assert!(
        (moved - fastframe_scroll::WHEEL_STEP).abs() < 1.0,
        "{moved}"
    );
}

#[test]
fn notification_presence_requires_the_displayed_transcript_and_account() {
    use super::chat::state::{Access, ChatState};
    use cww::tui::chat::{ChatSummary, Transcript};
    let mut state = ChatState::default();
    state.access = Access::Ready;
    state.open = Some(11);
    state.list.account.id = "482139075".into();
    assert!(super::visible_notification_chat(Page::Chat, true, Some(&state)).is_none());
    state.transcript = Some(Transcript {
        chat: ChatSummary {
            number: 12,
            ..Default::default()
        },
        ..Default::default()
    });
    assert!(super::visible_notification_chat(Page::Chat, true, Some(&state)).is_none());
    state.transcript.as_mut().unwrap().chat.number = 11;
    assert_eq!(
        super::visible_notification_chat(Page::Chat, true, Some(&state))
            .unwrap()
            .account,
        "482139075"
    );
    assert!(super::visible_notification_chat(Page::Chat, false, Some(&state)).is_none());
    assert!(super::visible_notification_chat(Page::General, true, Some(&state)).is_none());
    state.list.account.id.clear();
    assert!(super::visible_notification_chat(Page::Chat, true, Some(&state)).is_none());
}
