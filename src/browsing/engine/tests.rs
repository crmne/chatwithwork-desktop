use super::*;
use crate::browsing::bridge::{EXTENSION_ID, Policy, origin};
#[cfg(unix)]
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn digest(path: &Path) -> String {
    ring::digest::digest(&SHA256, &std::fs::read(path).unwrap())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn installation(body: &str) -> (tempfile::TempDir, Installation, Paths) {
    installation_with_startup(body, "printf '%s\\n' '{\"type\":\"ready\",\"protocol\":1}'")
}

fn installation_with_startup(
    body: &str,
    startup: &str,
) -> (tempfile::TempDir, Installation, Paths) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("browser-engine");
    std::fs::create_dir_all(root.join("node_modules/@xmorse/playwright-core")).unwrap();
    let node = if cfg!(windows) { "node.exe" } else { "node" };
    std::fs::write(root.join(node), format!("#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'v22.23.4\\n'; exit 0; fi\n{startup}\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join(node), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(root.join("entry.mjs"), "// fixed entry").unwrap();
    std::fs::write(
        root.join("node_modules/@xmorse/playwright-core/package.json"),
        "{}",
    )
    .unwrap();
    let files: BTreeMap<_, _> = [
        node,
        "entry.mjs",
        "node_modules/@xmorse/playwright-core/package.json",
    ]
    .into_iter()
    .map(|name| (name, digest(&root.join(name))))
    .collect();
    let target = match std::env::consts::OS {
        "windows" => format!("{}-pc-windows-msvc", std::env::consts::ARCH),
        "macos" => "macos-universal".into(),
        _ => format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
    };
    std::fs::write(root.join("manifest.json"), serde_json::to_vec(&json!({
        "protocol":1,"app_version":env!("CARGO_PKG_VERSION"),"node_version":NODE_VERSION,
        "engine_version":ENGINE_VERSION,"target":target,"node_support_ends":"2027-04-30","files":files,
    })).unwrap()).unwrap();
    let paths = Paths::under(&temp.path().join("state"));
    let installation = Installation::verify(&root, &Config::default(), &paths).unwrap();
    (temp, installation, paths)
}

#[test]
fn verifies_every_file_and_rejects_modified_or_added_code() {
    let (_temp, runtime, paths) = installation("exit 0");
    std::fs::write(runtime.root.join("entry.mjs"), "// modified").unwrap();
    assert!(Installation::verify(&runtime.root, &Config::default(), &paths).is_err());
    std::fs::write(runtime.root.join("entry.mjs"), "// fixed entry").unwrap();
    std::fs::write(
        runtime.root.join("node_modules/extra.js"),
        "// unlisted module",
    )
    .unwrap();
    assert!(Installation::verify(&runtime.root, &Config::default(), &paths).is_err());
}

#[test]
fn locator_pins_an_immutable_manifest_and_cannot_redirect_an_existing_installation() {
    let (temp, runtime, paths) = installation("exit 0");
    let container = temp.path().join("addressed-engine");
    std::fs::create_dir(&container).unwrap();
    let version = digest(&runtime.root.join("manifest.json"));
    std::fs::rename(&runtime.root, container.join(&version)).unwrap();
    let locator = container.join("current.json");
    std::fs::write(
        &locator,
        serde_json::to_vec(&json!({"protocol":1,"directory":version})).unwrap(),
    )
    .unwrap();
    let pinned = Installation::load(&container, &Config::default(), &paths).unwrap();
    assert_eq!(
        pinned.root,
        container.join(&version).canonicalize().unwrap()
    );
    for value in [
        "../outside".to_string(),
        "a".repeat(64),
        version.to_uppercase(),
    ] {
        std::fs::write(
            &locator,
            serde_json::to_vec(&json!({"protocol":1,"directory":value})).unwrap(),
        )
        .unwrap();
        assert!(Installation::load(&container, &Config::default(), &paths).is_err());
        assert_eq!(
            pinned.entry,
            container
                .join(&version)
                .join("entry.mjs")
                .canonicalize()
                .unwrap()
        );
    }
    std::fs::write(
        &locator,
        serde_json::to_vec(&json!({"protocol":1,"directory":version})).unwrap(),
    )
    .unwrap();
    std::fs::write(pinned.root.join("manifest.json"), b"{}").unwrap();
    assert!(Installation::load(&container, &Config::default(), &paths).is_err());
}

#[test]
fn msi_sibling_discovery_requires_the_exact_bounded_installer_marker() {
    let temp = tempfile::tempdir().unwrap();
    assert!(!msi_installation(temp.path()));
    let marker = temp.path().join("cww-installer.txt");
    for invalid in [
        b"anything".as_slice(),
        b"cww-installer-v1\nextra",
        &[b'x'; 65],
    ] {
        std::fs::write(&marker, invalid).unwrap();
        assert!(!msi_installation(temp.path()));
    }
    std::fs::write(&marker, b"cww-installer-v1\r\n").unwrap();
    assert!(msi_installation(temp.path()));
}

#[test]
fn rejects_wrong_runtime_versions_and_architecture_before_execution() {
    let (_temp, runtime, paths) = installation("exit 0");
    let path = runtime.root.join("manifest.json");
    let original = std::fs::read(&path).unwrap();
    for (field, value) in [
        ("protocol", json!(2)),
        ("node_version", json!("99.0.0")),
        ("app_version", json!("0.0.1")),
        ("target", json!("unrelated-platform")),
    ] {
        let mut manifest: Value = serde_json::from_slice(&original).unwrap();
        manifest[field] = value;
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            Installation::verify(&runtime.root, &Config::default(), &paths).is_err(),
            "{field}"
        );
    }
}

#[test]
fn never_trusts_code_inside_a_writable_shared_folder_or_own_state() {
    let (temp, runtime, paths) = installation("exit 0");
    let mut config = Config::default();
    config.roots.push(
        serde_json::from_value(json!({
            "id":"work","label":"Work","path":temp.path().canonicalize().unwrap(),"writable":true,
        }))
        .unwrap(),
    );
    assert!(Installation::verify(&runtime.root, &config, &paths).is_err());
    config.roots[0].writable = false;
    assert!(Installation::verify(&runtime.root, &config, &paths).is_ok());
    let mut paths = paths;
    paths.data_dir = runtime.root.parent().unwrap().to_path_buf();
    assert!(Installation::verify(&runtime.root, &config, &paths).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_and_world_writable_modules() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_temp, runtime, paths) = installation("exit 0");
    std::fs::set_permissions(&runtime.entry, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(Installation::verify(&runtime.root, &Config::default(), &paths).is_err());
    std::fs::set_permissions(&runtime.entry, std::fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&runtime.entry, runtime.root.join("linked.mjs")).unwrap();
    assert!(Installation::verify(&runtime.root, &Config::default(), &paths).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_probe_detects_missing_loader_or_wrong_version_before_admission() {
    let (_temp, mut runtime, _) = installation("exit 0");
    runtime.readiness.store(2, Ordering::Release);
    assert!(runtime.start().await.is_ok());
    runtime.executable = runtime.root.join("missing-node");
    assert!(runtime.start().await.is_err());
    assert_eq!(
        runtime.readiness.load(Ordering::Acquire),
        3,
        "future admissions use legacy, without retrying the failed request"
    );
}

#[cfg(unix)]
async fn execute(
    bridge: &Arc<Bridge>,
    installation: &Installation,
    session: EngineSession,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    super::execute(
        bridge,
        installation,
        session,
        action,
        params,
        tokio::time::Instant::now() + REQUEST_TIMEOUT,
    )
    .await
}

#[tokio::test]
async fn frame_limit_applies_even_when_the_oversized_line_has_a_newline() {
    let bytes = [vec![b'x'; MAX_FRAME], vec![b'\n']].concat();
    let mut input = BufReader::new(bytes.as_slice());
    assert!(read_frame(&mut input, &mut Vec::new()).await.is_err());
}

struct Attached {
    bridge: Arc<Bridge>,
    stop: CancellationToken,
    requests: mpsc::UnboundedReceiver<Value>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn attached(answer: impl Fn(&Value) -> Vec<Value> + Send + 'static) -> Attached {
    let bridge = Arc::new(Bridge::new(
        Policy {
            sites: vec!["example.com".into()],
            changes: true,
            ..Default::default()
        },
        vec![origin(EXTENSION_ID)],
        || {},
    ));
    let stop = CancellationToken::new();
    let (daemon, extension) = tokio::io::duplex(MAX_FRAME * 2);
    let server = Arc::clone(&bridge);
    let shutdown = stop.clone();
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(daemon);
        server
            .serve(
                origin(EXTENSION_ID),
                &mut write,
                &mut BufReader::new(read),
                shutdown,
            )
            .await
            .unwrap();
    });
    let (read, mut write) = tokio::io::split(extension);
    let mut lines = BufReader::new(read).lines();
    lines.next_line().await.unwrap().unwrap();
    lines.next_line().await.unwrap().unwrap();
    write_frame(
        &mut write,
        &json!({"event":"hello","engine_protocol":1,"site_policy_version":2}),
    )
    .await
    .unwrap();
    let (requests, receive) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            if request.get("id").is_none() {
                continue;
            }
            let _ = requests.send(request.clone());
            for message in answer(&request) {
                if write_frame(&mut write, &message).await.is_err() {
                    return;
                }
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while bridge.engine_session().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    Attached {
        bridge,
        stop,
        requests: receive,
    }
}

fn answer(request: &Value) -> Vec<Value> {
    let result = match request["method"].as_str().unwrap() {
        "engine_begin" => json!({"lease":"test-lease-123456789","tab":1,
            "timeout_ms":request["params"]["timeout_ms"],
            "action":request["params"]["action"],"params":request["params"]["params"]}),
        "engine_command" => json!({"result":{"ready":true}}),
        "engine_finish" => json!({"url":"https://example.com/","done":true}),
        "engine_cancel" => json!({}),
        other => panic!("unexpected legacy/other dispatch: {other}"),
    };
    vec![json!({"id":request["id"],"result":result})]
}

#[cfg(unix)]
#[tokio::test]
async fn fixed_child_round_trip_uses_lease_and_only_returns_extension_finish() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"command","id":1,"command":{"id":7,"method":"Runtime.enable","params":{}}}'
read -r reply
case "$reply" in *'"result":{"ready":true}'*) ;; *) exit 9;; esac
printf '%s\n' '{"type":"result","result":{"surfaceBackendNodeId":99}}'
"#,
    );
    let mut fixture = attached(answer).await;
    let result = execute(
        &fixture.bridge,
        &runtime,
        fixture.bridge.engine_session().unwrap(),
        "click",
        json!({"tab":1,"ref":"e1"}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"url":"https://example.com/","done":true}));
    let mut methods = Vec::new();
    while let Ok(request) = fixture.requests.try_recv() {
        methods.push(request["method"].as_str().unwrap().to_string());
        if request["method"] == "engine_command" {
            assert_eq!(request["params"]["lease"], "test-lease-123456789");
            assert_eq!(request["params"]["command"]["method"], "Runtime.enable");
        }
    }
    assert_eq!(methods, ["engine_begin", "engine_command", "engine_finish"]);
}

#[cfg(unix)]
#[tokio::test]
async fn optional_tab_uses_only_the_extensions_resolved_tab() {
    let (_temp, runtime, _) =
        installation("read -r call; printf '%s\\n' '{\"type\":\"result\",\"result\":{}}'");
    let fixture = attached(answer).await;
    assert!(
        execute(
            &fixture.bridge,
            &runtime,
            fixture.bridge.engine_session().unwrap(),
            "click",
            json!({"ref":"e1"})
        )
        .await
        .unwrap()["done"]
            .as_bool()
            .unwrap()
    );
    assert!(
        execute(
            &fixture.bridge,
            &runtime,
            fixture.bridge.engine_session().unwrap(),
            "click",
            json!({"tab":2,"ref":"e1"})
        )
        .await
        .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn helper_errors_are_sanitized_cancelled_and_never_retried_via_legacy() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"error","error":{"code":"page_error","message":"PRIVATE_CARD_VALUE /home/private"}}'
"#,
    );
    let mut fixture = attached(answer).await;
    let error = execute(
        &fixture.bridge,
        &runtime,
        fixture.bridge.engine_session().unwrap(),
        "click",
        json!({"tab":1,"ref":"e1"}),
    )
    .await
    .unwrap_err();
    assert!(!error.message.contains("PRIVATE"));
    assert!(!error.message.contains("/home"));
    assert!(error.message.contains("uncertain"));
    let methods: Vec<_> = std::iter::from_fn(|| fixture.requests.try_recv().ok())
        .map(|r| r["method"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(methods, ["engine_begin", "engine_cancel"]);
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_result_cannot_overtake_an_unacknowledged_command() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"command","id":1,"command":{"id":7,"method":"Runtime.releaseObject","params":{"objectId":"owned"}}}'
printf '%s\n' '{"type":"result","result":{}}'
read -r until_cancel
"#,
    );
    let mut fixture = attached(|request| {
        if request["method"] == "engine_command" {
            vec![]
        } else {
            answer(request)
        }
    })
    .await;
    assert!(
        execute(
            &fixture.bridge,
            &runtime,
            fixture.bridge.engine_session().unwrap(),
            "type",
            json!({"tab":1,"ref":"e1","text":"fixture"})
        )
        .await
        .is_err()
    );
    let methods: Vec<_> = std::iter::from_fn(|| fixture.requests.try_recv().ok())
        .map(|request| request["method"].as_str().unwrap().to_string())
        .collect();
    assert!(!methods.iter().any(|method| method == "engine_finish"));
    assert_eq!(methods.last().map(String::as_str), Some("engine_cancel"));
}

#[cfg(unix)]
#[tokio::test]
async fn revocation_cancels_a_pending_command_without_waiting_for_its_reply() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"command","id":1,"command":{"id":7,"method":"Runtime.evaluate","params":{}}}'
read -r reply
"#,
    );
    let waiting = Arc::new(Notify::new());
    let started = Arc::clone(&waiting);
    let mut fixture = attached(move |request| {
        if request["method"] == "engine_command" {
            started.notify_one();
            vec![]
        } else {
            answer(request)
        }
    })
    .await;
    let bridge = Arc::clone(&fixture.bridge);
    let call = tokio::spawn(async move {
        execute(
            &bridge,
            &runtime,
            bridge.engine_session().unwrap(),
            "click",
            json!({"tab":1,"ref":"e1"}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), waiting.notified())
        .await
        .unwrap();
    let mut policy = fixture.bridge.policy();
    policy.stopped = true;
    fixture
        .bridge
        .configure(policy, vec![origin(EXTENSION_ID)], vec![]);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let methods: Vec<_> = std::iter::from_fn(|| fixture.requests.try_recv().ok())
        .map(|r| r["method"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(methods, ["engine_begin", "engine_command", "engine_cancel"]);
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_a_call_releases_its_extension_lease() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"command","id":1,"command":{"id":7,"method":"Runtime.enable","params":{}}}'
read -r reply
read -r until_cancel
"#,
    );
    let mut fixture = attached(answer).await;
    let bridge = Arc::clone(&fixture.bridge);
    let call = tokio::spawn(async move {
        execute(
            &bridge,
            &runtime,
            bridge.engine_session().unwrap(),
            "click",
            json!({"tab":1,"ref":"e1"}),
        )
        .await
    });
    assert_eq!(
        fixture.requests.recv().await.unwrap()["method"],
        "engine_begin"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), fixture.requests.recv())
            .await
            .unwrap()
            .unwrap()["method"],
        "engine_command"
    );
    call.abort();
    let _ = call.await;
    let cancel = tokio::time::timeout(Duration::from_secs(2), fixture.requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancel["method"], "engine_cancel");
}

#[cfg(unix)]
#[tokio::test]
async fn wrong_or_missing_ready_never_acquires_a_lease_or_dispatches_input() {
    for startup in [
        "exit 0",
        "printf '%s\\n' '{\"type\":\"ready\",\"protocol\":2}'",
    ] {
        let (_temp, runtime, _) = installation_with_startup("read -r call", startup);
        let mut fixture = attached(answer).await;
        let error = execute(
            &fixture.bridge,
            &runtime,
            fixture.bridge.engine_session().unwrap(),
            "click",
            json!({"tab":1,"ref":"e1"}),
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("No browser action was sent"));
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn only_events_for_the_bound_lease_reach_the_private_helper() {
    let (_temp, runtime, _) = installation(
        r#"
read -r call
printf '%s\n' '{"type":"command","id":1,"command":{"id":1,"method":"Runtime.enable","params":{}}}'
read -r event
case "$event" in *'"params":{"marker":"correct"}'*) ;; *) exit 9;; esac
read -r reply
case "$reply" in *'"result":{"ready":true}'*) ;; *) exit 9;; esac
printf '%s\n' '{"type":"result","result":{}}'
"#,
    );
    let fixture = attached(|request| {
        let mut replies = Vec::new();
        if request["method"] == "engine_command" {
            for (lease, marker) in [("unrelated-lease", "private"), ("test-lease-123456789", "correct")] {
                replies.push(json!({"event":"engine","lease":lease,"message":{
                    "method":"Runtime.executionContextCreated","params":{"marker":marker},"sessionId":"owned"}}));
            }
        }
        replies.extend(answer(request));
        replies
    }).await;
    assert!(
        execute(
            &fixture.bridge,
            &runtime,
            fixture.bridge.engine_session().unwrap(),
            "click",
            json!({"tab":1,"ref":"e1"})
        )
        .await
        .unwrap()["done"]
            .as_bool()
            .unwrap()
    );
}

#[tokio::test]
async fn removed_extension_origin_cannot_start_or_continue_engine_calls() {
    let mut fixture = attached(answer).await;
    let session = fixture.bridge.engine_session().unwrap();
    fixture
        .bridge
        .configure(fixture.bridge.policy(), vec![], vec![]);
    assert!(fixture.bridge.engine_session().is_none());
    assert!(
        fixture
            .bridge
            .call_in_session(session.id, "engine_begin", json!({}))
            .await
            .is_err()
    );
    assert!(fixture.requests.try_recv().is_err());
    // Revocation still permits idempotent cleanup, which cannot send input.
    assert!(
        fixture
            .bridge
            .call_in_session(session.id, "engine_cancel", json!({"lease":"old"}))
            .await
            .is_ok()
    );
    assert_eq!(
        fixture.requests.recv().await.unwrap()["method"],
        "engine_cancel"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn warming_proves_import_readiness_without_any_operation_input() {
    let (_temp, runtime, _) =
        installation("read -r forbidden; printf '%s' \"$forbidden\" > forbidden-input");
    let runtime = Arc::new(runtime);
    assert_ne!(runtime.readiness.load(Ordering::Acquire), 2);
    Arc::clone(&runtime).warm(CancellationToken::new()).await;
    assert_eq!(runtime.readiness.load(Ordering::Acquire), 2);
    assert!(!runtime.root.join("forbidden-input").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn slow_per_call_startup_cannot_outlive_the_shared_request_budget() {
    let (_temp, runtime, _) = installation_with_startup("exit 99", "read -r never");
    let mut fixture = attached(answer).await;
    let now = tokio::time::Instant::now();
    let deadline = now + CLEANUP_TIMEOUT + Duration::from_millis(50);
    let error = super::execute(
        &fixture.bridge,
        &runtime,
        fixture.bridge.engine_session().unwrap(),
        "click",
        json!({"tab":1,"ref":"e1"}),
        deadline,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("No browser action was sent"));
    assert!(fixture.requests.try_recv().is_err());
    assert!(now.elapsed() < Duration::from_secs(1));
}

#[cfg(unix)]
#[tokio::test]
async fn second_automatic_phase_never_starts_after_the_shared_deadline() {
    let (_temp, runtime, _) = installation("read -r call");
    let mut fixture = attached(answer).await;
    let error = super::execute(
        &fixture.bridge,
        &runtime,
        fixture.bridge.engine_session().unwrap(),
        "click",
        json!({"tab":1,"ref":"e1"}),
        tokio::time::Instant::now(),
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("No browser action was sent"));
    assert!(fixture.requests.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn policy_cancellation_during_startup_does_not_disable_a_healthy_runtime() {
    let (_temp, runtime, _) =
        installation_with_startup("exit 99", "printf ready > started; read -r never");
    let runtime = Arc::new(runtime);
    runtime.readiness.store(2, Ordering::Release);
    let mut fixture = attached(answer).await;
    let bridge = Arc::clone(&fixture.bridge);
    let child_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move {
        execute(
            &bridge,
            &child_runtime,
            bridge.engine_session().unwrap(),
            "click",
            json!({"tab":1,"ref":"e1"}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !runtime.root.join("started").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut policy = fixture.bridge.policy();
    policy.stopped = true;
    fixture
        .bridge
        .configure(policy, vec![origin(EXTENSION_ID)], vec![]);
    assert!(task.await.unwrap().is_err());
    assert_eq!(runtime.readiness.load(Ordering::Acquire), 2);
    assert!(fixture.requests.try_recv().is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
#[ignore = "requires CWW_TEST_ENGINE_DIR naming a staged immutable package"]
fn packaged_helper_imports_under_confinement() {
    let directory = std::env::var_os("CWW_TEST_ENGINE_DIR").expect("staged engine directory");
    if std::env::var_os("CWW_TEST_ENGINE_CONFINED").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browsing::engine::tests::packaged_helper_imports_under_confinement",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("CWW_TEST_ENGINE_DIR", directory)
            .env("CWW_TEST_ENGINE_CONFINED", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths::under(temp.path());
    paths.ensure().unwrap();
    let runtime = Installation::load(Path::new(&directory), &Config::default(), &paths).unwrap();
    let status = crate::sandbox::confine(
        crate::sandbox::Plan::new([], &paths, false)
            .with_helper(runtime.executable.clone(), runtime.root.clone()),
    );
    assert_eq!(status.state, "enforced");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(STARTUP_TIMEOUT, runtime.start())
                .await
                .unwrap()
                .unwrap();
        });
}
