//! `browser_upload` as the server calls it: the files checked on this
//! computer and handed to the extension by where they really are, the
//! approval bound to them, and an upload that never runs by itself.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::bridge::{Bridge, EXTENSION_ID, Policy, origin};
use super::tools::BrowserTools;
use crate::config::{Config, Limits, Root};
use crate::error::ErrorCode;
use crate::limits::Limiter;
use crate::paths::Paths;
use crate::reader::Reader;
use crate::status::Changes;
use crate::tools::Caller;

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A bridge serving a fake extension that says hello with `hello`, then
/// answers each request with `answer(method, params)`, keeping each.
async fn attached(
    hello: Value,
    answer: impl Fn(&str, &Value) -> Value + Send + 'static,
) -> (Arc<Bridge>, Calls) {
    let bridge = Arc::new(Bridge::new(
        Policy {
            sites: vec!["play.google.com".into()],
            changes: true,
            ..Default::default()
        },
        vec![origin(EXTENSION_ID)],
        || {},
    ));
    let (daemon_end, extension_end) = tokio::io::duplex(1 << 20);
    {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_end);
            let mut lines = BufReader::new(read);
            let _ = bridge
                .serve(
                    origin(EXTENSION_ID),
                    &mut write,
                    &mut lines,
                    CancellationToken::new(),
                )
                .await;
        });
    }
    let (read, mut write) = tokio::io::split(extension_end);
    let mut lines = BufReader::new(read).lines();
    assert_eq!(lines.next_line().await.unwrap().unwrap(), r#"{"ok":true}"#);
    lines.next_line().await.unwrap().unwrap();
    let mut out = serde_json::to_vec(&hello).unwrap();
    out.push(b'\n');
    write.write_all(&out).await.unwrap();
    let calls: Calls = Arc::default();
    let kept = Arc::clone(&calls);
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            if request.get("id").is_none() {
                continue;
            }
            let method = request["method"].as_str().unwrap().to_string();
            kept.lock()
                .unwrap()
                .push((method.clone(), request["params"].clone()));
            let reply =
                json!({ "id": request["id"], "result": answer(&method, &request["params"]) });
            let mut out = serde_json::to_vec(&reply).unwrap();
            out.push(b'\n');
            write.write_all(&out).await.unwrap();
        }
    });
    // The hello lands before the first call.
    for _ in 0..100 {
        if bridge.extension().is_some_and(|e| !e.version.is_empty()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    (bridge, calls)
}

fn hello() -> Value {
    json!({ "event": "hello", "version": "0.1.5", "site_policy_version": 2, "uploads": 1, "file_access": true })
}

/// What the extension answers: the page's site, and, once uploaded, the
/// files the field holds.
fn page(chosen: Value) -> impl Fn(&str, &Value) -> Value + Send + 'static {
    move |_, params| {
        let mut answer = json!({ "tab": 4, "url": "https://play.google.com/console/app-bundles", "title": "App bundles",
            "dry_run": params["dry_run"], "field": { "how": "field", "label": "App bundle", "accept": ".aab" } });
        if params["dry_run"] != json!(true) {
            answer["chosen"] = chosen.clone();
        }
        answer
    }
}

struct Shared {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    reader: Arc<Reader>,
}

fn shared() -> Shared {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("builds")).unwrap();
    fs::create_dir_all(base.join("private")).unwrap();
    fs::write(base.join("builds/app-release.aab"), vec![7u8; 2048]).unwrap();
    fs::write(base.join("private/keys.txt"), "secret").unwrap();
    let mut config = Config {
        roots: vec![Root {
            bots: Vec::new(),
            id: "builds".into(),
            label: "Builds".into(),
            path: base.join("builds"),
            follow_symlinks: false,
            writable: false,
            commands: false,
            network: false,
            local_services: true,
            reach: Vec::new(),
            read_budget: None,
        }],
        ..Config::default()
    };
    config.index.enabled = false;
    let reader = Arc::new(
        Reader::new(
            &config,
            &Paths::under(&base.join("cww")),
            Changes::default(),
        )
        .unwrap(),
    );
    Shared {
        _tmp: tmp,
        base,
        reader,
    }
}

fn tools(bridge: Arc<Bridge>, shared: &Shared) -> BrowserTools {
    let limiter = Arc::new(Limiter::new(Limits::default()));
    BrowserTools::new(bridge, None, limiter, true).with_reader(Arc::clone(&shared.reader))
}

fn uploads(calls: &Calls) -> Vec<Value> {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "upload")
        .map(|(_, params)| params.clone())
        .collect()
}

const ARGS: &str = r#"{"ref":"e12","paths":["builds:app-release.aab"]}"#;

fn args(extra: Value) -> Value {
    let mut args: Value = serde_json::from_str(ARGS).unwrap();
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    args
}

#[tokio::test]
async fn the_browser_gets_the_checked_real_path_and_the_server_never_does() {
    let shared = shared();
    let (bridge, calls) = attached(hello(), page(json!([]))).await;
    let answer = tools(bridge, &shared)
        .call(
            "browser_upload",
            args(json!({ "dry_run": true })),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    let sent = uploads(&calls);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["files"],
        json!([{ "path": shared.base.join("builds/app-release.aab").to_string_lossy(), "name": "app-release.aab",
            "size": 2048, "mime_type": "application/octet-stream" }])
    );
    assert!(sent[0].get("paths").is_none());
    let value = answer.value;
    assert_eq!(value["files"][0]["path"], "builds:app-release.aab");
    assert_eq!(value["files"][0]["folder"], "Builds");
    assert_eq!(value["files"][0]["size"], 2048);
    assert_eq!(value["fingerprint"].as_str().unwrap().len(), 64);
    assert_eq!(value["field"]["label"], "App bundle");
    assert!(
        !value
            .to_string()
            .contains(shared.base.to_string_lossy().as_ref())
    );
    assert_eq!(answer.site.as_deref(), Some("play.google.com"));
}

#[tokio::test]
async fn the_approved_upload_must_name_the_files_the_dry_run_checked() {
    let shared = shared();
    let (bridge, calls) = attached(
        hello(),
        page(json!([{ "name": "app-release.aab", "size": 2048 }])),
    )
    .await;
    let tools = tools(bridge, &shared);
    let checked = tools
        .call(
            "browser_upload",
            args(json!({ "dry_run": true })),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    let fingerprint = checked.value["fingerprint"].clone();

    let missing = tools
        .call("browser_upload", args(json!({})), &Caller::Person, None)
        .await
        .err()
        .unwrap();
    assert_eq!(missing.code, ErrorCode::InvalidArgument);
    let other = tools
        .call(
            "browser_upload",
            args(json!({ "expected_sha256": "0".repeat(64) })),
            &Caller::Person,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(other.code, ErrorCode::Conflict);
    assert_eq!(
        uploads(&calls).len(),
        1,
        "only the dry run reached the browser"
    );

    let done = tools
        .call(
            "browser_upload",
            args(json!({ "expected_sha256": fingerprint })),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    assert_eq!(done.value["files"][0]["name"], "app-release.aab");
    assert!(done.value.get("chosen").is_none());

    // The file changed after the person saw it: nothing is uploaded.
    fs::write(shared.base.join("builds/app-release.aab"), vec![8u8; 2048]).unwrap();
    let changed = tools
        .call(
            "browser_upload",
            args(json!({ "expected_sha256": fingerprint })),
            &Caller::Person,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(changed.code, ErrorCode::Conflict);
    assert_eq!(uploads(&calls).len(), 2);
}

#[tokio::test]
async fn a_page_holding_other_files_fails_the_upload() {
    let shared = shared();
    let (bridge, _calls) = attached(
        hello(),
        page(json!([{ "name": "other.aab", "size": 2048 }])),
    )
    .await;
    let tools = tools(bridge, &shared);
    let fingerprint = tools
        .call(
            "browser_upload",
            args(json!({ "dry_run": true })),
            &Caller::Person,
            None,
        )
        .await
        .unwrap()
        .value["fingerprint"]
        .clone();
    let error = tools
        .call(
            "browser_upload",
            args(json!({ "expected_sha256": fingerprint })),
            &Caller::Person,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::PageError);
}

#[cfg(unix)]
#[tokio::test]
async fn files_outside_the_shared_folders_never_reach_the_browser() {
    let shared = shared();
    std::os::unix::fs::symlink(
        shared.base.join("private/keys.txt"),
        shared.base.join("builds/keys.aab"),
    )
    .unwrap();
    let (bridge, calls) = attached(hello(), page(json!([]))).await;
    let tools = tools(bridge, &shared);
    for (path, code) in [
        ("builds:../private/keys.txt", ErrorCode::InvalidPath),
        ("private:keys.txt", ErrorCode::UnknownRoot),
        ("builds:keys.aab", ErrorCode::Denied),
    ] {
        let error = tools
            .call(
                "browser_upload",
                json!({ "ref": "e12", "paths": [path], "dry_run": true }),
                &Caller::Person,
                None,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, code, "{path}");
        assert!(!error.message.contains("keys"), "{path}: {}", error.message);
    }
    let twice = tools
        .call(
            "browser_upload",
            json!({ "ref": "e12", "paths": ["builds:app-release.aab", "builds:app-release.aab"], "dry_run": true }),
            &Caller::Person,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(twice.code, ErrorCode::InvalidArgument);
    assert!(uploads(&calls).is_empty());
}

#[tokio::test]
async fn an_upload_never_runs_by_itself() {
    let shared = shared();
    let (bridge, calls) = attached(hello(), page(json!([]))).await;
    bridge.configure(
        Policy {
            sites: vec!["play.google.com".into()],
            changes: true,
            auto_approve: true,
            ..Default::default()
        },
        vec![origin(EXTENSION_ID)],
        vec![],
    );
    let tools = tools(bridge, &shared);
    assert!(tools.auto_approves());
    for caller in [
        Caller::BrowserAutomatic { bot: None },
        Caller::LocalBrowser { approved: false },
    ] {
        let error = tools
            .call("browser_upload", args(json!({})), &caller, None)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::NeedsApproval, "{caller:?}");
    }
    assert!(uploads(&calls).is_empty());
    // The definition says it always asks.
    let definition = super::tools::definitions()
        .into_iter()
        .find(|d| d.name == "browser_upload")
        .unwrap();
    assert_eq!(
        definition.kind,
        super::tools::Kind::Destructive { write_when: None }
    );
}

#[tokio::test]
async fn an_older_extension_is_asked_to_update() {
    let shared = shared();
    let mut old = hello();
    old.as_object_mut().unwrap().remove("uploads");
    let (bridge, calls) = attached(old, page(json!([]))).await;
    let error = tools(bridge, &shared)
        .call(
            "browser_upload",
            args(json!({ "dry_run": true })),
            &Caller::Person,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::BrowserUnavailable);
    assert!(error.message.contains("Update"));
    assert!(uploads(&calls).is_empty());
}

#[tokio::test]
async fn offered_only_with_shared_folders_and_actions_allowed() {
    let shared = shared();
    let (bridge, _calls) = attached(hello(), page(json!([]))).await;
    let limiter = Arc::new(Limiter::new(Limits::default()));
    let without = BrowserTools::new(Arc::clone(&bridge), None, Arc::clone(&limiter), true);
    assert!(!without.offers("browser_upload"));
    assert!(tools(Arc::clone(&bridge), &shared).offers("browser_upload"));
    let read_only =
        BrowserTools::new(bridge, None, limiter, false).with_reader(Arc::clone(&shared.reader));
    assert!(!read_only.offers("browser_upload"));
}
