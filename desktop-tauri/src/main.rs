//! Chat with Work Desktop, Tauri prototype: the chat list, a chat's
//! transcript with its streaming answer and approval cards, and the
//! Folders page the egui app opens on, drawn in a web view with the web
//! app's Live Wire styles. Everything goes through the daemon's control
//! socket, as the egui app and `cww tui` do; the window never holds a
//! token.
//!
//! Built to measure whether Tauri is fast and light enough to replace
//! egui, not as a port: see docs/tauri-prototype.md.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;
mod markdown;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{Value, json};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Emitter, State};

/// The control requests the window may send. Anything else, such as
/// sharing a folder with `i_know`, stays with the CLI and settings.
const ALLOWED: [&str; 9] = [
    "status",
    "chats",
    "chat",
    "chat_send",
    "chat_cancel",
    "chat_approve",
    "chat_deny",
    "models",
    "roots_writable",
];

#[derive(Default)]
struct Follows {
    chat: Mutex<Option<daemon::Subscription>>,
    list: Mutex<Option<daemon::Subscription>>,
    status: Mutex<Option<daemon::Subscription>>,
}

/// Renders every answer's Markdown, so the page only inserts HTML.
fn with_html(mut answer: Value) -> Value {
    if let Some(entries) = answer["entries"].as_array_mut() {
        for entry in entries {
            if entry["kind"] == "assistant" {
                let html = markdown::to_html(entry["content"].as_str().unwrap_or_default());
                entry["html"] = json!(html);
            }
        }
    }
    answer
}

#[tauri::command]
async fn control(request: Value) -> Result<Value, String> {
    let cmd = request["cmd"].as_str().unwrap_or_default();
    if !ALLOWED.contains(&cmd) {
        return Err(format!("The window can't send {cmd:?}."));
    }
    let chat = cmd == "chat";
    tauri::async_runtime::spawn_blocking(move || daemon::request(&request).map(|a| if chat { with_html(a) } else { a }))
        .await
        .map_err(|e| e.to_string())?
}

/// Follows one chat, and stops following the one before. Chunks are
/// joined and rendered here, so each event carries the answer so far.
#[tauri::command]
async fn follow_chat(app: AppHandle, follows: State<'_, Follows>, chat: String) -> Result<(), String> {
    follows.chat.lock().unwrap().take();
    let number = chat.clone();
    let subscription = tauri::async_runtime::spawn_blocking(move || {
        let mut written: HashMap<u64, String> = HashMap::new();
        daemon::subscribe(&json!({ "cmd": "subscribe", "topics": ["chat"], "chat": number }), move |event| {
            if event["event"] != "chat" {
                return;
            }
            let update = &event["update"];
            if update["type"] == "chunk" {
                let id = update["message_id"].as_u64().unwrap_or_default();
                let text = written.entry(id).or_default();
                text.push_str(update["text"].as_str().unwrap_or_default());
                let html = markdown::to_html(text);
                let _ = app.emit("chat", json!({ "chat": event["chat"], "type": "chunk", "message_id": id, "html": html }));
            } else {
                let _ = app.emit("chat", json!({ "chat": event["chat"], "type": update["type"], "text": update["text"] }));
            }
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *follows.chat.lock().unwrap() = Some(subscription);
    Ok(())
}

#[tauri::command]
async fn follow_list(app: AppHandle, follows: State<'_, Follows>) -> Result<(), String> {
    let subscription = tauri::async_runtime::spawn_blocking(move || {
        daemon::subscribe(&json!({ "cmd": "subscribe", "topics": ["chats"] }), move |event| {
            if event["event"] == "chats" {
                let _ = app.emit("chats", &event["update"]);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *follows.list.lock().unwrap() = Some(subscription);
    Ok(())
}

#[tauri::command]
async fn follow_status(app: AppHandle, follows: State<'_, Follows>) -> Result<(), String> {
    let subscription = tauri::async_runtime::spawn_blocking(move || {
        daemon::subscribe(&json!({ "cmd": "subscribe", "topics": ["status"] }), move |event| {
            if event["event"] == "status" {
                let _ = app.emit("status", &event["status"]);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *follows.status.lock().unwrap() = Some(subscription);
    Ok(())
}

/// For measuring: with `CWW_PAINT_MARK` set, the page calls this once its
/// first frame with the daemon's data is on screen, and it prints the
/// wall clock, which the benchmark script compares with its launch time.
#[tauri::command]
fn painted(what: String) {
    if std::env::var_os("CWW_PAINT_MARK").is_some() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        eprintln!("cww-paint {what} {}", now.as_micros());
    }
}

/// For measuring: a line from the page's own checks (`CWW_AUTOTEST`).
#[tauri::command]
fn report(line: String) {
    if std::env::var_os("CWW_PAINT_MARK").is_some() {
        eprintln!("cww-report {line}");
    }
}

/// Settings the page reads on load, for benchmarks: which chat to open,
/// and which in-page check to run (`approve`, `scroll`, `type`).
#[tauri::command]
fn launch_options() -> Value {
    json!({
        "open_chat": std::env::var("CWW_OPEN_CHAT").ok(),
        "autotest": std::env::var("CWW_AUTOTEST").ok(),
    })
}

/// `cwwasset://localhost/assets/...`: logos and file icons, fetched by the
/// daemon from the paired server (CONTROL.md, `asset`) and kept for the
/// life of the window by the web view's cache.
fn asset(path: &str) -> Response<Vec<u8>> {
    let fetched = daemon::request(&json!({ "cmd": "asset", "path": path })).ok().and_then(|answer| {
        let data = base64::engine::general_purpose::STANDARD.decode(answer["data"].as_str()?).ok()?;
        Some((answer["content_type"].as_str()?.to_string(), data))
    });
    match fetched {
        Some((content_type, data)) => Response::builder()
            .header("Content-Type", content_type)
            .header("Cache-Control", "max-age=31536000, immutable")
            .body(data)
            .unwrap(),
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap(),
    }
}

fn main() {
    tauri::Builder::default()
        .manage(Follows::default())
        .register_asynchronous_uri_scheme_protocol("cwwasset", |_ctx, request, responder| {
            let path = request.uri().path().to_string();
            std::thread::spawn(move || responder.respond(asset(&path)));
        })
        .invoke_handler(tauri::generate_handler![
            control,
            follow_chat,
            follow_list,
            follow_status,
            painted,
            report,
            launch_options
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Tauri prototype");
}
