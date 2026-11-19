//! Acting by position and keyboard: which extension method each tool calls,
//! the picture of an action's fresh look, and an older extension asked to
//! update rather than sent calls it doesn't know.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::bridge::{Bridge, EXTENSION_ID, Policy, origin};
use super::tools::{BrowserTools, Kind, definitions};
use crate::config::Limits;
use crate::error::ErrorCode;
use crate::limits::Limiter;
use crate::tools::Caller;

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

async fn attached(
    pointer: bool,
    answer: impl Fn(&str, &Value) -> Value + Send + 'static,
) -> (Arc<Bridge>, Calls) {
    let bridge = Arc::new(Bridge::new(
        Policy {
            sites: vec!["example.com".into()],
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
    lines.next_line().await.unwrap().unwrap();
    lines.next_line().await.unwrap().unwrap();
    let mut hello = json!({ "event": "hello", "version": "0.1.6", "site_policy_version": 2 });
    if pointer {
        hello["pointer"] = json!(1);
    }
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
    for _ in 0..100 {
        if bridge.extension().is_some_and(|e| !e.version.is_empty()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    (bridge, calls)
}

fn tools(bridge: Arc<Bridge>) -> BrowserTools {
    BrowserTools::new(
        bridge,
        None,
        Arc::new(Limiter::new(Limits::default())),
        true,
    )
}

const PICTURE: &str = "/9j/4AAQ";

fn page(method: &str, _: &Value) -> Value {
    let look = json!({ "capture": "c2", "width": 1280, "height": 720, "image": { "mime_type": "image/jpeg", "data": PICTURE } });
    match method {
        "look" => {
            json!({ "tab": 1, "url": "https://example.com/", "capture": "c1", "width": 1280, "height": 720, "image": { "mime_type": "image/jpeg", "data": PICTURE } })
        }
        _ => json!({ "tab": 1, "url": "https://example.com/", "dry_run": false, "look": look }),
    }
}

fn methods(calls: &Calls) -> Vec<String> {
    calls
        .lock()
        .unwrap()
        .iter()
        .map(|(m, _)| m.clone())
        .collect()
}

#[tokio::test]
async fn each_tool_reaches_its_method_and_a_fresh_look_comes_back_as_a_picture() {
    let (bridge, calls) = attached(true, page).await;
    let tools = tools(bridge);
    let shot = tools
        .call("browser_screenshot", json!({}), &Caller::Person, None)
        .await
        .unwrap();
    assert_eq!(shot.value["capture"], "c1");
    assert!(shot.image.is_some());
    let clicked = tools
        .call(
            "browser_click_at",
            json!({ "capture": "c1", "x": 10, "y": 20, "submits": false }),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    assert_eq!(clicked.value["look"]["capture"], "c2");
    assert!(
        clicked.value["look"].get("image").is_none(),
        "the picture is the result's image, not text"
    );
    assert_eq!(clicked.image.as_ref().unwrap().data, PICTURE);
    tools
        .call(
            "browser_hover",
            json!({ "capture": "c2", "x": 1, "y": 1 }),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    tools
        .call(
            "browser_hover",
            json!({ "capture": "c2", "x": 1, "y": 1, "delta_y": 300 }),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    tools
        .call(
            "browser_drag",
            json!({ "capture": "c2", "x": 1, "y": 1, "to_x": 5, "to_y": 5, "submits": false }),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    tools
        .call(
            "browser_press",
            json!({ "key": "Tab", "submits": false }),
            &Caller::Person,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        methods(&calls),
        ["look", "click_at", "hover", "wheel", "drag", "press"]
    );
}

#[tokio::test]
async fn an_older_extension_is_asked_to_update_and_keeps_its_screenshots() {
    let (bridge, calls) = attached(false, |_, _| json!({ "tab": 1, "url": "https://example.com/", "image": { "mime_type": "image/jpeg", "data": PICTURE } })).await;
    let tools = tools(bridge);
    for (name, args) in [
        (
            "browser_click_at",
            json!({ "capture": "c", "x": 1, "y": 1 }),
        ),
        ("browser_press", json!({ "key": "Tab" })),
        ("browser_hover", json!({ "capture": "c", "x": 1, "y": 1 })),
    ] {
        let error = tools
            .call(name, args, &Caller::Person, None)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::BrowserUnavailable, "{name}");
        assert!(error.message.contains("Update"));
    }
    tools
        .call("browser_screenshot", json!({}), &Caller::Person, None)
        .await
        .unwrap();
    assert_eq!(methods(&calls), ["screenshot"]);
}

#[test]
fn a_click_drag_or_key_may_submit_unless_the_call_says_it_doesnt_and_hovering_only_looks() {
    let kind = |name: &str| {
        definitions()
            .into_iter()
            .find(|d| d.name == name)
            .unwrap()
            .kind
    };
    for name in ["browser_click_at", "browser_drag", "browser_press"] {
        assert_eq!(
            kind(name),
            Kind::Destructive {
                write_when: Some(json!({ "submits": false }))
            },
            "{name}"
        );
    }
    assert_eq!(kind("browser_hover"), Kind::Read);
}
