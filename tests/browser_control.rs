//! A local client talks to a real daemon and a synthetic native extension.
//! No browser profile, user settings, cloud credentials or network is used.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cww::browsing::bridge::{EXTENSION_ID, origin};
use cww::config::Config;
use cww::control::{self, ControlRequest};
use cww::paths::Paths;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

async fn control(paths: &Paths, request: ControlRequest) -> Value {
    let socket = paths.socket_path();
    tokio::task::spawn_blocking(move || {
        control::Client::connect(&socket)
            .unwrap()
            .unwrap()
            .call_raw(&request)
            .unwrap()
    })
    .await
    .unwrap()
}

fn call(tool: &str, arguments: Value, approved: bool) -> ControlRequest {
    ControlRequest::BrowserCall {
        tool: tool.into(),
        arguments: arguments.as_object().unwrap().clone(),
        approved,
    }
}

fn code(response: &Value) -> &str {
    response["result"]["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap_or("")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_browser_control_preserves_policy_approval_budgets_and_audit() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let mut config = Config {
        secret_store: Some("file".into()),
        ..Config::default()
    };
    config.index.enabled = false;
    config.browser.enabled = true;
    config.browser.changes = true;
    config.browser.sites = vec!["example.com".into()];
    config.limits.read_chars_per_chat_hour = 20;
    config.limits.read_chars_per_hour = 100;
    config.limits.changes_per_day = 3;
    config.save(&paths).unwrap();
    let shutdown = CancellationToken::new();
    let daemon = tokio::spawn(cww::daemon::run(paths.clone(), shutdown.clone(), false));
    tokio::time::timeout(Duration::from_secs(10), async {
        while !paths.socket_path().exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let socket = paths.socket_path();
    let requests = Arc::clone(&seen);
    let (ready, attached) = tokio::sync::oneshot::channel();
    let extension = std::thread::spawn(move || {
        let control::BrowserAttach::Attached {
            mut reader,
            mut writer,
        } = control::attach_browser(&socket, &origin(EXTENSION_ID)).unwrap()
        else {
            panic!("extension attached");
        };
        writeln!(
            writer,
            "{}",
            json!({"event":"hello","version":"test","site_policy_version":2,"stopped":false})
        )
        .unwrap();
        writer.flush().unwrap();
        ready.send(()).unwrap();
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let request: Value = serde_json::from_str(&line).unwrap();
            line.clear();
            if request.get("id").is_none() {
                continue;
            }
            requests.lock().unwrap().push(request.clone());
            let args = &request["params"];
            let mut reply = json!({ "id": request["id"] });
            if args["ref"] == "sensitive" {
                reply["error"] = json!({"code":"sensitive_field", "message":"protected field"});
            } else if args["tab"] == 99 {
                reply["error"] = json!({"code":"not_in_group", "message":"outside group"});
            } else {
                reply["result"] = match request["method"].as_str().unwrap() {
                    "read" => json!({"url":"https://example.com/fixture", "text":"abcdefghij"}),
                    "tabs" => json!({"tabs":[
                        {"tab":1,"url":"https://example.com/fixture","title":"Fixture"},
                        {"tab":2,"url":"https://private.invalid/message","title":"Private title"}
                    ]}),
                    _ => json!({"url":"https://example.com/fixture", "dry_run":args["dry_run"]}),
                };
            }
            writeln!(writer, "{reply}").unwrap();
            writer.flush().unwrap();
        }
    });
    attached.await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while control(&paths, ControlRequest::Browser).await["browser"]["extension"]["site_policy_version"] != 2 {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();

    // All-sites is a local setting. A block wins even if an extension
    // returns content it should have refused, and saved allow entries survive.
    control(
        &paths,
        ControlRequest::BrowserSite {
            site: "elster.de".into(),
            allowed: true,
        },
    )
    .await;
    control(&paths, ControlRequest::BrowserAllSites { all_sites: true }).await;
    let all = control(&paths, call("browser_tabs", json!({}), false)).await;
    assert!(
        all.to_string().contains("Private title"),
        "all HTTP(S) sites are deliberately allowed"
    );
    for site in ["example.com", "private.invalid"] {
        control(
            &paths,
            ControlRequest::BrowserBlockedSite {
                site: site.into(),
                blocked: true,
            },
        )
        .await;
    }
    let blocked = control(&paths, call("browser_read", json!({}), false)).await;
    assert_eq!(code(&blocked), "site_not_allowed");
    assert!(!blocked.to_string().contains("abcdefghij"));
    let tabs = control(&paths, call("browser_tabs", json!({}), false)).await;
    assert!(!tabs.to_string().contains("Private title"));
    assert!(!tabs.to_string().contains("/message"));
    let status = control(&paths, ControlRequest::BrowserAllSites { all_sites: false }).await;
    assert_eq!(
        status["browser"]["sites"],
        json!(["elster.de", "example.com"])
    );
    assert_eq!(
        status["browser"]["blocked_sites"],
        json!(["example.com", "private.invalid"])
    );
    assert_eq!(
        code(&control(&paths, call("browser_read", json!({}), false)).await),
        "site_not_allowed"
    );
    for site in ["example.com", "private.invalid"] {
        control(
            &paths,
            ControlRequest::BrowserBlockedSite {
                site: site.into(),
                blocked: false,
            },
        )
        .await;
    }
    let saved = Config::load(&paths).unwrap();
    assert_eq!(saved.browser.sites, ["elster.de", "example.com"]);
    assert!(!saved.browser.all_sites);
    assert!(saved.browser.blocked_sites.is_empty());

    seen.lock().unwrap().clear();
    let tools = control(&paths, ControlRequest::BrowserTools).await;
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"browser_read"));
    assert!(names.contains(&"browser_click"));
    assert!(
        names
            .iter()
            .all(|name| cww::browsing::tools::is_browser_tool(name))
    );
    for tool in [
        "read",
        "roots",
        "run",
        "write",
        "browser_evaluate",
        "browser_cdp",
    ] {
        let refused = control(&paths, call(tool, json!({}), true)).await;
        assert_eq!(refused["code"], "unknown_tool", "{refused}");
    }
    assert!(seen.lock().unwrap().is_empty());

    let injected = control(
        &paths,
        call(
            "browser_read",
            json!({"script":"return document.cookie"}),
            false,
        ),
    )
    .await;
    assert_eq!(code(&injected), "invalid_argument");
    assert!(seen.lock().unwrap().is_empty());

    let tabs = control(&paths, call("browser_tabs", json!({}), false)).await;
    assert!(tabs.to_string().contains("Fixture"));
    assert!(!tabs.to_string().contains("Private title"));
    assert!(!tabs.to_string().contains("/message"));
    let outside = control(&paths, call("browser_read", json!({"tab":99}), false)).await;
    assert_eq!(code(&outside), "not_in_group");

    let listed = control(&paths, ControlRequest::BrowserTools).await;
    assert!(
        !listed
            .to_string()
            .contains(cww::tools::BROWSER_AUTO_APPROVE_META)
    );
    let changed = control(
        &paths,
        ControlRequest::BrowserAskBeforeActions { ask: false },
    )
    .await;
    assert_eq!(changed["browser"]["ask_before_actions"], false);
    let listed = control(&paths, ControlRequest::BrowserTools).await;
    for tool in listed["tools"].as_array().unwrap() {
        if cww::browsing::tools::CHANGE_TOOLS.contains(&tool["name"].as_str().unwrap()) {
            assert_eq!(tool["_meta"][cww::tools::BROWSER_AUTO_APPROVE_META], true);
            assert_eq!(tool["annotations"]["readOnlyHint"], false);
        } else {
            assert!(
                tool["_meta"]
                    .get(cww::tools::BROWSER_AUTO_APPROVE_META)
                    .is_none()
            );
        }
    }
    for _ in 0..2 {
        let before = seen.lock().unwrap().len();
        let automatic = control(&paths, call("browser_click", json!({"ref":"e1"}), false)).await;
        assert_ne!(automatic["result"]["isError"], true, "{automatic}");
        let requests = seen.lock().unwrap();
        assert_eq!(
            requests.len(),
            before + 2,
            "automatic actions retain preflight"
        );
        assert_eq!(requests[before]["params"]["dry_run"], true);
        assert!(requests[before + 1]["params"]["dry_run"].is_null());
    }
    control(
        &paths,
        ControlRequest::BrowserAskBeforeActions { ask: true },
    )
    .await;
    assert!(Config::load(&paths).unwrap().browser.ask_before_actions);
    assert!(
        !control(&paths, ControlRequest::BrowserTools)
            .await
            .to_string()
            .contains(cww::tools::BROWSER_AUTO_APPROVE_META)
    );

    let denied = control(&paths, call("browser_click", json!({"ref":"e1"}), false)).await;
    assert_eq!(code(&denied), "needs_approval");
    let before = seen.lock().unwrap().len();
    let preview = control(
        &paths,
        call("browser_click", json!({"ref":"e1","dry_run":true}), false),
    )
    .await;
    assert_ne!(preview["result"]["isError"], true);
    assert_eq!(seen.lock().unwrap().len(), before + 1);
    let action = control(&paths, call("browser_click", json!({"ref":"e1"}), true)).await;
    assert_ne!(action["result"]["isError"], true);
    {
        let calls = seen.lock().unwrap();
        assert_eq!(calls[calls.len() - 2]["params"]["dry_run"], true);
        assert!(calls[calls.len() - 1]["params"]["dry_run"].is_null());
    }
    let before = seen.lock().unwrap().len();
    let sensitive = control(
        &paths,
        call(
            "browser_type",
            json!({"ref":"sensitive","text":"private-input"}),
            true,
        ),
    )
    .await;
    assert_eq!(code(&sensitive), "sensitive_field");
    assert_eq!(
        seen.lock().unwrap().len(),
        before + 1,
        "failed dry run must not execute"
    );

    let before = seen.lock().unwrap().len();
    let limited = control(&paths, call("browser_click", json!({"ref":"e1"}), true)).await;
    assert_eq!(code(&limited), "rate_limited");
    assert_eq!(
        seen.lock().unwrap().len(),
        before + 1,
        "only its dry run reaches the extension"
    );

    for request in [
        ControlRequest::Pause,
        ControlRequest::BrowserStopped { stopped: true },
    ] {
        let expected = if matches!(request, ControlRequest::Pause) {
            "paused"
        } else {
            "browser_stopped"
        };
        control(&paths, request).await;
        let before = seen.lock().unwrap().len();
        assert_eq!(
            code(&control(&paths, call("browser_tabs", json!({}), false)).await),
            expected
        );
        assert_eq!(seen.lock().unwrap().len(), before);
        control(&paths, ControlRequest::Resume).await;
    }
    control(&paths, ControlRequest::BrowserStopped { stopped: false }).await;

    // Separate control connections still spend the same daemon-owned budget.
    for _ in 0..2 {
        let result = control(&paths, call("browser_read", json!({}), false)).await;
        assert_ne!(result["result"]["isError"], true, "{result}");
    }
    let forged: ControlRequest = serde_json::from_value(json!({
        "cmd":"browser_call", "tool":"browser_read", "chat_id":"another-budget",
        "arguments":{}, "approved":true
    }))
    .unwrap();
    assert_eq!(code(&control(&paths, forged).await), "rate_limited");
    control(&paths, ControlRequest::BrowserChanges { changes: false }).await;
    let tools = control(&paths, ControlRequest::BrowserTools).await;
    assert!(!tools.to_string().contains("browser_click"));
    assert_eq!(
        control(&paths, call("browser_click", json!({"ref":"e1"}), true)).await["code"],
        "unknown_tool"
    );

    control(&paths, ControlRequest::BrowserEnabled { enabled: false }).await;
    assert_eq!(
        control(&paths, ControlRequest::BrowserTools).await["tools"],
        json!([])
    );
    let audit = std::fs::read_to_string(paths.audit_file()).unwrap();
    assert!(audit.contains("local browser client"));
    assert!(audit.contains("local-browser"));
    assert!(audit.contains("browser_local_policy"));
    assert!(audit.contains("browser_automatic_actions_allowed"));
    assert!(audit.contains("browser_approvals_required"));
    for private in [
        "private-input",
        "abcdefghij",
        "Private title",
        "another-budget",
    ] {
        assert!(!audit.contains(private), "audit must not contain {private}");
    }
    shutdown.cancel();
    daemon.await.unwrap().unwrap();
    extension.join().unwrap();
}
