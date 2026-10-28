//! The coding tools as the server calls them: `grep`, `glob`,
//! `read_bytes` and `patch`, with the read budgets (shared and per folder),
//! the copy budget, bots, and the audit log.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use base64::Engine;
use serde_json::{Value, json};

use super::*;
use crate::audit::{AuditLog, tail};
use crate::config::{BotLoan, Config, Limits, Root};
use crate::paths::Paths;
use crate::status::Changes;
use crate::trash::Trash;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    audit: PathBuf,
    files: LocalFiles,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Two folders that allow changes: `app` with a read budget of its own
/// (`app_budget`), lent to bot 7, and `docs` on the shared budget.
fn fixture(limits: Limits, app_budget: Option<usize>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let app = base.join("app");
    let docs = base.join("docs");
    write(&app.join("src/lib.rs"), "fn main() {\n    run();\n}\n");
    write(&app.join("src/run.rs"), "pub fn run() {}\n");
    write(&docs.join("notes.md"), "run the numbers\n");
    fs::write(
        app.join("logo.png"),
        [0x89, b'P', b'N', b'G', 0, 1, 2, 3, 255],
    )
    .unwrap();
    let paths = Paths::under(&base.join("cww"));
    let root = |id: &str, path: &Path| Root {
        bots: Vec::new(),
        id: id.into(),
        label: id.to_uppercase(),
        path: path.to_path_buf(),
        follow_symlinks: false,
        writable: true,
        commands: false,
        network: false,
        read_budget: None,
    };
    let mut app_root = root("app", &app);
    app_root.read_budget = app_budget;
    app_root.bots.push(BotLoan {
        id: "7".into(),
        name: "Fixer".into(),
        unattended: false,
    });
    let mut config = Config {
        roots: vec![app_root, root("docs", &docs)],
        limits,
        ..Config::default()
    };
    config.index.enabled = false;
    let reader = Arc::new(Reader::new(&config, &paths, Changes::default()).unwrap());
    let limiter = Arc::new(Limiter::new(config.limits.clone()));
    let trash = paths.home_trash.clone().unwrap();
    let writer = Arc::new(Writer::new(
        Arc::clone(&reader),
        Arc::clone(&limiter),
        Trash::new(Some(trash)),
    ));
    let audit = base.join("audit.jsonl");
    let files = LocalFiles::new(
        reader,
        writer,
        limiter,
        Arc::new(AuditLog::open(&audit).unwrap()),
        Arc::new(AtomicBool::new(false)),
        config.limits.max_message_bytes,
    );
    Fixture {
        _tmp: tmp,
        base,
        audit,
        files,
    }
}

fn limits() -> Limits {
    Limits {
        read_chars_per_chat_hour: 60,
        read_chars_per_hour: 100,
        ..Limits::default()
    }
}

async fn call(f: &Fixture, tool: &str, args: Value) -> Value {
    call_as(f, tool, args, Caller::Person).await
}

async fn call_as(f: &Fixture, tool: &str, args: Value, caller: Caller) -> Value {
    let Value::Object(args) = args else {
        panic!("arguments are an object")
    };
    let result = f
        .files
        .call_as(tool, Some(args), Some("1".into()), "r".into(), None, caller)
        .await
        .expect("a known tool");
    result.structured_content.unwrap()
}

fn code(answer: &Value) -> &str {
    answer["error"]["code"].as_str().unwrap_or("none")
}

fn bot() -> Caller {
    Caller::Bot {
        id: "7".into(),
        unattended: false,
    }
}

#[tokio::test]
async fn lists_the_coding_tools() {
    let f = fixture(Limits::default(), None);
    let tools = f.files.tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "roots",
            "search",
            "list",
            "read",
            "grep",
            "glob",
            "read_bytes",
            "create",
            "write",
            "edit",
            "mkdir",
            "move",
            "delete",
            "create_document",
            "patch"
        ]
    );
    for tool in &tools {
        let a = tool.annotations.as_ref().unwrap();
        assert_eq!(a.open_world_hint, Some(false), "{}", tool.name);
        if ["grep", "glob", "read_bytes"].contains(&tool.name.as_ref()) {
            assert_eq!(a.read_only_hint, Some(true));
            assert_eq!(a.destructive_hint, Some(false));
            assert_eq!(a.idempotent_hint, Some(true));
        }
        if tool.name == "patch" {
            assert_eq!(a.read_only_hint, Some(false));
            assert_eq!(a.destructive_hint, Some(false));
            assert_eq!(a.idempotent_hint, Some(false));
            assert!(tool.meta.is_none());
        }
    }
    // Unknown arguments are refused, as for every tool.
    for tool in ["grep", "glob"] {
        let answer = call(&f, tool, json!({ "pattern": "x", "regex": true })).await;
        assert_eq!(code(&answer), "invalid_argument", "{tool}");
    }
    let answer = call(
        &f,
        "read_bytes",
        json!({ "path": "app:logo.png", "bytes": 1 }),
    )
    .await;
    assert_eq!(code(&answer), "invalid_argument");
    let answer = call(&f, "patch", json!({ "changes": [], "atomic": true })).await;
    assert_eq!(code(&answer), "invalid_argument");
}

#[tokio::test]
async fn grep_answers_count_against_the_shared_read_budget() {
    let f = fixture(limits(), None);
    // Each match costs its path and its line: "app:src/lib.rs" (14) and
    // "    run();" (10), "app:src/run.rs" (14) and "pub fn run() {}" (15).
    let answer = call(&f, "grep", json!({ "pattern": "run", "path": "app:" })).await;
    assert_eq!(answer["matches"].as_array().unwrap().len(), 2, "{answer}");
    assert_eq!(answer["files_searched"], 2);
    assert_eq!(answer["truncated"], false);
    assert_eq!(answer["timed_out"], false);
    assert_eq!(answer["next_offset"], Value::Null);
    // 53 of 60 are spent: the next answer is cut where the budget ends.
    let answer = call(&f, "glob", json!({ "pattern": "*.rs" })).await;
    assert_eq!(code(&answer), "rate_limited", "{answer}");
    let answer = call(&f, "read", json!({ "path": "docs:notes.md" })).await;
    assert_eq!(answer["text"], "run the", "{answer}");
    let answer = call(&f, "grep", json!({ "pattern": "run" })).await;
    assert_eq!(code(&answer), "rate_limited");
    // Each grep is in the audit log with its pattern, like a search.
    let entries = tail(&f.audit, 10).unwrap();
    let grep = entries
        .iter()
        .find(|e| e.tool.as_deref() == Some("grep"))
        .unwrap();
    assert_eq!(grep.query.as_deref(), Some("run"));
    assert_eq!(grep.results, Some(2));
}

#[tokio::test]
async fn a_cut_answer_says_where_the_next_page_starts() {
    let f = fixture(
        Limits {
            read_chars_per_chat_hour: 30,
            read_chars_per_hour: 100,
            ..Limits::default()
        },
        None,
    );
    let answer = call(&f, "grep", json!({ "pattern": "run", "path": "app:" })).await;
    assert_eq!(answer["matches"].as_array().unwrap().len(), 1, "{answer}");
    assert_eq!(answer["truncated"], true);
    assert_eq!(answer["next_offset"], 1);
    assert!(answer["note"].as_str().unwrap().contains("read budget"));
}

#[tokio::test]
async fn a_folder_with_its_own_budget_reads_on_it() {
    // `app` may read 100 characters a chat an hour; the shared budget is 60.
    let f = fixture(limits(), Some(100));
    let roots = call(&f, "roots", json!({})).await;
    assert_eq!(roots["roots"][0]["read_budget"], 100);
    assert!(roots["roots"][1].get("read_budget").is_none());
    let answer = call(&f, "grep", json!({ "pattern": "run", "path": "app:" })).await;
    assert_eq!(answer["matches"].as_array().unwrap().len(), 2);
    let answer = call(&f, "grep", json!({ "pattern": "run", "path": "app:" })).await;
    assert_eq!(answer["matches"].as_array().unwrap().len(), 1, "{answer}");
    assert_eq!(answer["truncated"], true);
    // 77 of app's 100 are spent, none of the shared budget; then 93.
    let answer = call(&f, "read", json!({ "path": "docs:notes.md" })).await;
    assert_eq!(answer["text"], "run the numbers\n");
    let answer = call(&f, "read", json!({ "path": "app:src/run.rs" })).await;
    assert_eq!(answer["text"], "pub fn run() {}\n", "{answer}");
    let answer = call(&f, "glob", json!({ "pattern": "*.rs", "path": "app:" })).await;
    assert_eq!(code(&answer), "rate_limited", "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("this folder")
    );
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("a more specific pattern"),
        "{answer}"
    );
    // A search of every folder still gets what is outside it.
    let answer = call(&f, "glob", json!({ "pattern": "*.md" })).await;
    assert_eq!(answer["paths"][0]["path"], "docs:notes.md");
}

#[tokio::test]
async fn read_bytes_copies_any_file_in_chunks() {
    let f = fixture(
        Limits {
            copy_bytes_per_hour: 12,
            ..Limits::default()
        },
        None,
    );
    let first = call(
        &f,
        "read_bytes",
        json!({ "path": "app:logo.png", "max_bytes": 5 }),
    )
    .await;
    let data = |answer: &Value| {
        base64::engine::general_purpose::STANDARD
            .decode(answer["data"].as_str().unwrap())
            .unwrap()
    };
    assert_eq!(data(&first), [0x89, b'P', b'N', b'G', 0]);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["next_offset"], 5);
    assert_eq!(first["size"], 9);
    assert!(first["modified"].as_str().unwrap().ends_with('Z'));
    let rest = call(
        &f,
        "read_bytes",
        json!({ "path": "app:logo.png", "offset": 5 }),
    )
    .await;
    assert_eq!(data(&rest), [1, 2, 3, 255]);
    assert_eq!(rest["next_offset"], Value::Null);
    // 9 of the hour's 12 bytes are spent: the next chunk gets 3.
    let cut = call(&f, "read_bytes", json!({ "path": "docs:notes.md" })).await;
    assert_eq!(data(&cut), b"run");
    assert_eq!(cut["next_offset"], 3);
    let none = call(&f, "read_bytes", json!({ "path": "docs:notes.md" })).await;
    assert_eq!(code(&none), "rate_limited");
    // Copies don't touch the read budget.
    let read = call(&f, "read", json!({ "path": "docs:notes.md" })).await;
    assert_eq!(read["text"], "run the numbers\n");
}

#[tokio::test]
async fn read_bytes_refuses_what_read_refuses() {
    let f = fixture(Limits::default(), None);
    write(&f.base.join("app/.env"), "TOKEN=1\n");
    std::os::unix::fs::symlink(f.base.join("docs/notes.md"), f.base.join("app/link.md")).unwrap();
    let big = fs::File::create(f.base.join("app/big.bin")).unwrap();
    big.set_len(crate::reader::MAX_COPY_FILE_BYTES + 1).unwrap();
    for (args, expected) in [
        (json!({ "path": "app:.env" }), "denied"),
        (json!({ "path": "app:link.md" }), "denied"),
        (json!({ "path": "app:src" }), "not_a_file"),
        (json!({ "path": "app:big.bin" }), "too_large"),
        (json!({ "path": "app:missing" }), "not_found"),
        (
            json!({ "path": "app:logo.png", "offset": 10 }),
            "invalid_argument",
        ),
        (
            json!({ "path": "app:logo.png", "max_bytes": 0 }),
            "invalid_argument",
        ),
        (
            json!({ "path": "app:logo.png", "max_bytes": 65_537 }),
            "invalid_argument",
        ),
        (json!({ "path": "/etc/passwd" }), "invalid_path"),
    ] {
        let answer = call(&f, "read_bytes", args.clone()).await;
        assert_eq!(code(&answer), expected, "{args}");
    }
}

#[tokio::test]
async fn a_big_chunk_fits_one_message() {
    let f = fixture(Limits::default(), None);
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(f.base.join("app/data.bin"), &bytes).unwrap();
    let Value::Object(args) = json!({ "path": "app:data.bin" }) else {
        unreachable!()
    };
    let result = f
        .files
        .call("read_bytes", Some(args), None, "r".into())
        .await
        .unwrap();
    // A full 64 KiB chunk, twice in base64, within the 256 KiB message.
    let size = serde_json::to_vec(&result).unwrap().len();
    assert!(size <= 256 * 1024 - 2048, "{size}");
    let answer = result.structured_content.unwrap();
    assert_eq!(answer["next_offset"], 65_536);
    let data = base64::engine::general_purpose::STANDARD
        .decode(answer["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(data, bytes[..65_536]);
}

#[tokio::test]
async fn bots_reach_only_the_folders_lent_to_them() {
    let f = fixture(Limits::default(), None);
    let answer = call_as(&f, "grep", json!({ "pattern": "run" }), bot()).await;
    let paths: Vec<&str> = answer["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["app:src/lib.rs", "app:src/run.rs"]);
    let answer = call_as(&f, "glob", json!({ "pattern": "*" }), bot()).await;
    assert!(
        answer["paths"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["path"].as_str().unwrap().starts_with("app:"))
    );
    for (tool, args) in [
        ("grep", json!({ "pattern": "run", "path": "docs:" })),
        ("glob", json!({ "pattern": "*", "path": "docs:" })),
        ("read_bytes", json!({ "path": "docs:notes.md" })),
        (
            "patch",
            json!({ "changes": [
                { "path": "app:src/run.rs", "old_text": "run", "new_text": "go" },
                { "path": "docs:notes.md", "old_text": "run", "new_text": "crunch" }
            ] }),
        ),
        (
            "patch",
            json!({ "changes": [{ "path": "app:src/run.rs", "old_text": "run", "new_text": "go" }],
                    "expected_sha256": { "docs:notes.md": "0" } }),
        ),
    ] {
        let answer = call_as(&f, tool, args.clone(), bot()).await;
        assert_eq!(code(&answer), "not_lent", "{tool} {args}");
    }
    assert_eq!(
        fs::read_to_string(f.base.join("app/src/run.rs")).unwrap(),
        "pub fn run() {}\n"
    );
    let answer = call_as(
        &f,
        "patch",
        json!({ "changes": [{ "path": "app:src/run.rs", "old_text": "run", "new_text": "go" }] }),
        bot(),
    )
    .await;
    assert_eq!(answer["effect"], "patched", "{answer}");
}

#[tokio::test]
async fn a_patch_is_one_audit_entry_per_file() {
    let f = fixture(Limits::default(), None);
    let answer = call(
        &f,
        "patch",
        json!({ "changes": [
            { "path": "app:src/run.rs", "old_text": "run", "new_text": "go" },
            { "path": "docs:new.md", "content": "new\n" }
        ] }),
    )
    .await;
    assert_eq!(answer["dry_run"], false, "{answer}");
    assert_eq!(answer["files"][0]["previous"]["in_trash"], true);
    assert_eq!(answer["files"][1]["sha256"].as_str().unwrap().len(), 64);
    let entries = tail(&f.audit, 10).unwrap();
    let patched: Vec<_> = entries
        .iter()
        .filter(|e| e.tool.as_deref() == Some("patch"))
        .collect();
    assert_eq!(patched.len(), 2);
    assert_eq!(patched[0].path.as_deref(), Some("app:src/run.rs"));
    assert_eq!(patched[0].effect.as_deref(), Some("edited"));
    assert!(patched[0].trash.is_some());
    assert_eq!(patched[1].path.as_deref(), Some("docs:new.md"));
    assert_eq!(patched[1].effect.as_deref(), Some("created"));
    assert_eq!(patched[1].written, Some(4));
    // Never the contents.
    let log = fs::read_to_string(&f.audit).unwrap();
    assert!(!log.contains("go()") && !log.contains("new\\n"), "{log}");
    // A refused patch is one entry, naming its first file.
    let answer = call(
        &f,
        "patch",
        json!({ "changes": [{ "path": "docs:notes.md", "old_text": "absent", "new_text": "x" }] }),
    )
    .await;
    assert_eq!(code(&answer), "invalid_argument");
    let last = tail(&f.audit, 1).unwrap().pop().unwrap();
    assert_eq!(last.path.as_deref(), Some("docs:notes.md"));
    assert!(last.reason.unwrap().starts_with("Change 0 (docs:notes.md)"));
}

#[tokio::test]
async fn a_patch_dry_run_reads_against_each_folders_budget() {
    let f = fixture(limits(), Some(1000));
    let changes = json!([
        { "path": "app:src/run.rs", "old_text": "run", "new_text": "go" },
        { "path": "docs:new.md", "content": "a fairly long line of text to show\n" },
        { "path": "docs:more.md", "content": "more\n" }
    ]);
    let answer = call(&f, "patch", json!({ "changes": changes, "dry_run": true })).await;
    assert_eq!(answer["dry_run"], true, "{answer}");
    // app's own budget takes the first diff whole.
    let first = answer["files"][0]["diff"].as_str().unwrap();
    assert!(first.contains("+pub fn go() {}"), "{first}");
    // The shared budget (60) cuts the second, and the third gets none.
    let second = answer["files"][1]["diff"].as_str().unwrap();
    assert_eq!(second.chars().count(), 60, "{second}");
    assert!(second.ends_with("(the read budget ends here)\n"));
    assert_eq!(answer["files"][2]["diff"], Value::Null);
    assert_eq!(answer["files"][2]["diff_cut"], true);
    // With the shared budget spent, a dry run touching docs is refused.
    let answer = call(&f, "patch", json!({ "changes": changes, "dry_run": true })).await;
    assert_eq!(code(&answer), "rate_limited");
    assert!(!f.base.join("docs/new.md").exists());
    // Made for real, it needs no reading.
    let answer = call(&f, "patch", json!({ "changes": changes })).await;
    assert_eq!(answer["effect"], "patched", "{answer}");
}
