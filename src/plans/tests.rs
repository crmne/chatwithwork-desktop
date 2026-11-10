use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::parse::{self, Stop};
use super::*;
use crate::config::Config;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plans")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// Feed a captured stream to `agent`'s parser, as the step's watcher does,
/// a line at a time; returns the parser and whether it said to stop before
/// the stream ended (at which line).
fn feed(agent: Agent, name: &str) -> (Box<dyn parse::Events>, Option<usize>) {
    let mut events = parse::parser(agent, "0123456789ab", None);
    let start = Instant::now();
    let mut stopped = None;
    for (n, line) in fixture(name).lines().enumerate() {
        events.line(line, start);
        if stopped.is_none() && events.done(start + parse::CODEX_GRACE) {
            stopped = Some(n);
        }
    }
    (events, stopped)
}

#[test]
fn the_yes_is_read_from_the_config_file() {
    let config: Config = toml::from_str(
        r#"
[plans.codex]
chats = true
bots = ["42"]
[plans.claude_code]
chats = false
"#,
    )
    .unwrap();
    assert!(allowed(&config, Agent::Codex, None));
    assert!(allowed(&config, Agent::Codex, Some("42")));
    assert!(!allowed(&config, Agent::Codex, Some("43")));
    assert!(!allowed(&config, Agent::ClaudeCode, None));
    assert!(!allowed(&config, Agent::ClaudeCode, Some("42")));
    // No other agent, whatever a file says.
    assert!(!allowed(&config, Agent::OpenCode, None));
    // An empty section isn't written back.
    let saved = toml::to_string(&Config::default()).unwrap();
    assert!(!saved.contains("plans"), "{saved}");
    // Nothing unknown is taken.
    assert!(toml::from_str::<Config>("[plans.codex]\nshell = true\n").is_err());
}

#[test]
fn the_owners_yes_and_a_bots_are_given_and_taken_back_apart() {
    let mut config = Config::default();
    config.plans.offer_claude = true;
    assert!(set(&mut config, Agent::ClaudeCode, Some("42"), true));
    assert!(!set(&mut config, Agent::ClaudeCode, Some("42"), true));
    assert!(allowed(&config, Agent::ClaudeCode, Some("42")));
    assert!(
        !allowed(&config, Agent::ClaudeCode, None),
        "a bot's yes isn't the owner's"
    );
    assert!(set(&mut config, Agent::ClaudeCode, None, true));
    assert!(set(&mut config, Agent::ClaudeCode, Some("42"), false));
    assert!(!allowed(&config, Agent::ClaudeCode, Some("42")));
    assert!(
        allowed(&config, Agent::ClaudeCode, None),
        "turning a bot off leaves the owner's"
    );
    assert!(!allowed(&config, Agent::Codex, None));
    let text = toml::to_string(&config).unwrap();
    assert!(
        text.contains("[plans.claude_code]") && text.contains("chats = true"),
        "{text}"
    );
}

fn ask() -> ThinkAsk {
    ThinkAsk {
        agent: "codex".into(),
        bot: None,
        instructions: "You are Chat with Work.".into(),
        transcript: "User: hi".into(),
        tools: vec![ToolSpec {
            name: "gmail_search".into(),
            description: "Search mail".into(),
            input_schema: json!({ "type": "object" }),
        }],
        model: None,
        raw: false,
    }
}

#[test]
fn a_step_is_checked_before_anything_runs() {
    assert_eq!(validate(&ask()), Ok(Agent::Codex));
    let refused = |change: &dyn Fn(&mut ThinkAsk)| {
        let mut ask = ask();
        change(&mut ask);
        validate(&ask).is_err()
    };
    assert!(refused(&|a| a.agent = "opencode".into()));
    assert!(
        refused(&|a| a.agent = "claude".into()),
        "the wire says claude_code"
    );
    assert!(refused(&|a| a.transcript = "  ".into()));
    assert!(refused(&|a| a.instructions = "a\0b".into()));
    assert!(refused(&|a| a.tools[0].name = "rm -rf /".into()));
    assert!(refused(&|a| a.tools[0].name = "x".repeat(65)));
    assert!(refused(&|a| a.tools[0].input_schema = json!("string")));
    assert!(refused(&|a| a.tools.push(a.tools[0].clone())));
    assert!(refused(&|a| a.model = Some("gpt'; rm -rf ~; '".into())));
    assert!(refused(&|a| a.model = Some("-c shell=true".into())));
    assert!(refused(&|a| a.bot = Some("42; ls".into())));
    let mut fine = ask();
    fine.agent = "claude_code".into();
    fine.model = Some("claude-opus-5-5[1m]".into());
    fine.bot = Some("42".into());
    assert_eq!(validate(&fine), Ok(Agent::ClaudeCode));
}

#[test]
fn the_command_line_is_built_here_and_holds_none_of_the_servers_words() {
    let dir = Path::new("/data/cww/thinking/0123456789ab");
    let shim = Path::new("/usr/bin/cww");
    for agent in AGENTS {
        let line = command(agent, dir, shim, Some("model-1"));
        assert!(!line.contains("You are Chat with Work"));
        assert!(!line.contains("gmail_search"));
        assert_eq!(line.contains("prompt.md"), agent != Agent::Codex, "{line}");
        let words = arguments(agent, dir, shim, None);
        match agent {
            Agent::Codex => {
                assert_eq!(
                    &words[..3],
                    [
                        "/usr/bin/cww",
                        "plan-codex",
                        "/data/cww/thinking/0123456789ab"
                    ]
                );
                let words = codex_server_arguments(dir, shim);
                assert_eq!(words[0], "app-server");
                for feature in ["shell_tool", "unified_exec", "apps", "plugins", "hooks"] {
                    assert!(
                        words.contains(&format!("features.{feature}=false")),
                        "{feature}"
                    );
                }
                assert!(words.contains(&"web_search=\"disabled\"".to_string()));
                assert!(
                    words.contains(&format!(
                        "model_instructions_file={}",
                        serde_json::to_string(&dir.join(INSTRUCTIONS_FILE)).unwrap()
                    )),
                    "{words:?}"
                );
            }
            _ => {
                let at = |flag: &str| words.iter().position(|w| w == flag).unwrap();
                assert_eq!(words[at("--tools") + 1], "", "no built-in tools");
                assert_eq!(
                    words[at("--setting-sources") + 1],
                    "",
                    "no settings of the person's"
                );
                assert!(words.iter().any(|w| w == "--strict-mcp-config"));
                assert_eq!(words[at("--allowedTools") + 1], "mcp__cww__*");
            }
        }
    }
    // A model's name is one quoted word.
    if cfg!(unix) {
        let line = command(Agent::ClaudeCode, dir, shim, Some("sonnet"));
        assert!(line.starts_with("exec 'claude' '-p'"), "{line}");
        assert!(line.contains("'--model' 'sonnet'"), "{line}");
        assert!(line.ends_with("< '/data/cww/thinking/0123456789ab/prompt.md'"));
    }
}

#[test]
fn the_step_files_are_written_new_and_the_tools_in_mcp_shape() {
    let dir = tempfile::tempdir().unwrap();
    let mut ask = ask();
    ask.agent = "claude_code".into();
    write_files(
        dir.path(),
        &ask,
        Agent::ClaudeCode,
        Path::new("/usr/bin/cww"),
        None,
    )
    .unwrap();
    let tools: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(TOOLS_FILE)).unwrap())
            .unwrap();
    assert_eq!(tools[0]["name"], "gmail_search");
    assert_eq!(tools[0]["inputSchema"], json!({ "type": "object" }));
    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(MCP_FILE)).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["cww"]["args"][0], "plan-tools");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(PROMPT_FILE)).unwrap(),
        "User: hi"
    );
    // A second time, nothing is written through what's there.
    assert!(
        write_files(
            dir.path(),
            &ask,
            Agent::ClaudeCode,
            Path::new("/usr/bin/cww"),
            None,
        )
        .is_err()
    );
}

#[test]
fn claude_codes_parallel_calls_come_back_whole_and_stop_the_step() {
    let (events, stopped) = feed(Agent::ClaudeCode, "claude_parallel_tool_calls.jsonl");
    let step = events.step();
    assert_eq!(step.stop, Some(Stop::ToolUse));
    let names: Vec<&str> = step.tool_calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["gmail_search", "calendar_list"], "prefix taken off");
    assert_eq!(
        step.tool_calls[0].arguments,
        json!({ "query": "from:alice budget" })
    );
    assert_eq!(step.tool_calls[1].arguments, json!({ "day": "2026-11-09" }));
    assert!(step.tool_calls[0].id.starts_with("toolu_"));
    assert_eq!(step.model.as_deref(), Some("claude-opus-5-5"));
    let usage = step.usage.unwrap();
    assert_eq!(usage.output_tokens, 119);
    // Stopped at the message's end, before Claude Code's own result of the
    // call that never ran.
    let text = fixture("claude_parallel_tool_calls.jsonl");
    let lines: Vec<&str> = text.lines().collect();
    let at = stopped.expect("stopped before the stream ended");
    assert!(lines[at].contains("message_stop"), "{}", lines[at]);
    assert!(
        lines[at + 1..]
            .iter()
            .any(|l| l.contains("Connection closed"))
    );
    assert_eq!(step.result()["stop"], "tool_use");
}

#[test]
fn claude_codes_text_comes_from_its_deltas_and_ends_the_turn() {
    let (events, stopped) = feed(Agent::ClaudeCode, "claude_text_answer.jsonl");
    let step = events.step();
    assert!(
        step.text
            .starts_with("The sea covers more than seventy percent"),
        "{}",
        step.text
    );
    assert!(step.text.ends_with("variety of life."));
    assert!(step.tool_calls.is_empty());
    assert_eq!(step.stop, Some(Stop::EndTurn));
    assert_eq!(step.model.as_deref(), Some("claude-haiku-5-5"));
    let usage = step.usage.unwrap();
    assert_eq!(usage.output_tokens, 70);
    assert_eq!(usage.input_tokens, 2 + 946);
    assert_eq!(
        stopped,
        Some(fixture("claude_text_answer.jsonl").lines().count() - 1)
    );

    let (events, _) = feed(Agent::ClaudeCode, "claude_text_then_tool_call.jsonl");
    let step = events.step();
    assert_eq!(
        step.text,
        "I'll search your Gmail for messages about Alice and the budget."
    );
    assert_eq!(step.tool_calls.len(), 1);
    assert_eq!(step.tool_calls[0].name, "gmail_search");
}

#[test]
fn codexs_parallel_calls_wait_for_each_other_then_stop() {
    let mut events = parse::parser(Agent::Codex, "0123456789ab", Some("gpt-6".into()));
    let start = Instant::now();
    let lines: Vec<String> = fixture("codex_parallel_tool_calls.jsonl")
        .lines()
        .map(String::from)
        .collect();
    events.line(&lines[0], start);
    events.line(&lines[1], start);
    assert!(
        !events.done(start),
        "the model's other calls may still start"
    );
    events.line(&lines[2], start + Duration::from_millis(5));
    assert!(!events.done(start + Duration::from_millis(1000)));
    assert!(events.done(start + Duration::from_millis(5) + parse::CODEX_GRACE));
    let step = events.step();
    let names: Vec<&str> = step.tool_calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["gmail_search", "calendar_list"]);
    assert!(step.tool_calls[0].id.starts_with("0123456789ab-exec-"));
    assert_eq!(step.tool_calls[1].arguments, json!({ "day": "2026-11-09" }));
    assert_eq!(step.model.as_deref(), Some("gpt-6"));
    assert_eq!(step.result()["stop"], "tool_use");
    assert_eq!(step.usage.unwrap().input_tokens, 7332);
}

#[test]
fn codexs_text_and_usage_end_the_turn() {
    let (events, stopped) = feed(Agent::Codex, "codex_text_answer.jsonl");
    let step = events.step();
    assert_eq!(step.text, "ready");
    assert!(step.tool_calls.is_empty());
    assert_eq!(step.stop, Some(Stop::EndTurn));
    let usage = step.usage.unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.cached_tokens),
        (7313, 5, 0)
    );
    assert_eq!(stopped, Some(4));
}

fn codex_line(method: &str, params: Value) -> String {
    json!({ "method": method, "params": params }).to_string()
}

/// A call of another server's tool is never Chat with Work's, a warning
/// that retried is no error, and a text-only answer that follows it stands.
#[test]
fn codexs_retries_and_other_servers_calls_dont_end_a_step() {
    let mut events = parse::parser(Agent::Codex, "0123456789ab", None);
    let now = Instant::now();
    events.line(
        &codex_line(
            "item/started",
            json!({ "item": { "type": "mcpToolCall", "id": "a", "server": "other", "tool": "x", "arguments": {} } }),
        ),
        now,
    );
    events.line(
        &codex_line(
            "error",
            json!({ "error": { "message": "reconnecting" }, "willRetry": true }),
        ),
        now,
    );
    events.line(
        &codex_line(
            "item/agentMessage/delta",
            json!({ "itemId": "m", "delta": "He" }),
        ),
        now,
    );
    events.line(
        &codex_line(
            "item/agentMessage/delta",
            json!({ "itemId": "m", "delta": "llo" }),
        ),
        now,
    );
    events.line(
        &codex_line(
            "item/completed",
            json!({ "item": { "type": "agentMessage", "id": "m", "text": "Hello" } }),
        ),
        now,
    );
    events.line(
        &codex_line(
            "turn/completed",
            json!({ "turn": { "status": "completed" } }),
        ),
        now,
    );
    let step = events.step();
    assert_eq!(step.text, "Hello", "deltas and the whole item count once");
    assert!(step.tool_calls.is_empty());
    assert!(step.error.is_none());
    assert_eq!(step.stop, Some(Stop::EndTurn));
    assert!(events.done(now));
}

#[test]
fn a_failed_turn_or_a_codex_error_is_said() {
    let mut events = parse::parser(Agent::Codex, "0123456789ab", None);
    let now = Instant::now();
    events.line(
        &codex_line(
            "turn/completed",
            json!({ "turn": { "status": "failed", "error": { "message": "over its limit" } } }),
        ),
        now,
    );
    assert_eq!(events.step().error.as_deref(), Some("over its limit"));
    assert!(events.done(now));
}

/// `cww plan-codex`, message by message: it says who it is, starts one
/// ephemeral read-only thread and one turn with the conversation, declines
/// anything Codex asks, prints only what matters, and ends with the turn.
#[test]
fn the_codex_driver_says_who_it_is_and_runs_one_turn() {
    use codex_server::Conversation;
    let conversation = Conversation::new("/step".into(), "User: hi".into(), Some("gpt-6".into()));
    let first = conversation.first();
    assert_eq!(first["method"], "initialize");
    assert_eq!(
        first["params"]["clientInfo"]["name"],
        codex_server::CLIENT_NAME
    );

    let reaction = conversation.react(json!({ "id": 1, "result": {} }));
    assert_eq!(reaction.send[0]["method"], "initialized");
    let start = &reaction.send[1];
    assert_eq!(start["method"], "thread/start");
    assert_eq!(start["params"]["ephemeral"], true);
    assert_eq!(start["params"]["approvalPolicy"], "never");
    assert_eq!(start["params"]["sandbox"], "read-only");
    assert_eq!(start["params"]["model"], "gpt-6");

    let reaction = conversation.react(json!({ "id": 2, "result": { "thread": { "id": "t" } } }));
    assert_eq!(reaction.send[0]["method"], "turn/start");
    assert_eq!(reaction.send[0]["params"]["threadId"], "t");
    assert_eq!(reaction.send[0]["params"]["input"][0]["text"], "User: hi");

    // A request of Codex's is declined, never printed.
    let reaction = conversation
        .react(json!({ "id": 9, "method": "item/tool/requestUserInput", "params": {} }));
    assert_eq!(reaction.send[0]["id"], 9);
    assert!(reaction.send[0].get("error").is_some());
    assert!(reaction.print.is_none());

    // Nothing that isn't the answer leaves the step: not the transcript's
    // user message, not this computer's installation id.
    assert!(
        conversation
            .react(
                json!({ "method": "item/started", "params": { "item": { "type": "userMessage" } } })
            )
            .print
            .is_none()
    );
    assert!(conversation.react(json!({ "method": "remoteControl/status/changed", "params": { "installationId": "x" } })).print.is_none());
    assert!(
        conversation
            .react(
                json!({ "method": "item/started", "params": { "item": { "type": "mcpToolCall" } } })
            )
            .print
            .is_some()
    );

    let reaction = conversation.react(json!({ "method": "turn/completed", "params": { "turn": { "items": [1], "status": "completed" } } }));
    assert!(reaction.done);
    assert_eq!(
        reaction.print.unwrap()["params"]["turn"]["items"],
        json!([])
    );

    // A request refused ends the step, in words the parser reads.
    let reaction = conversation.react(json!({ "id": 2, "error": { "message": "no such model" } }));
    assert!(reaction.done);
    let line = reaction.print.unwrap().to_string();
    let mut events = parse::parser(Agent::Codex, "0123456789ab", None);
    events.line(&line, Instant::now());
    assert_eq!(events.step().error.as_deref(), Some("no such model"));
}

#[test]
fn a_steps_codex_home_holds_only_the_sign_in_and_gives_a_new_one_back() {
    let person = tempfile::tempdir().unwrap();
    std::fs::write(person.path().join("auth.json"), "old").unwrap();
    std::fs::write(person.path().join("config.toml"), "mine").unwrap();
    std::fs::write(person.path().join("AGENTS.md"), "mine").unwrap();
    let step = tempfile::tempdir().unwrap();
    write_files(
        step.path(),
        &ask(),
        Agent::Codex,
        Path::new("/usr/bin/cww"),
        Some(person.path()),
    )
    .unwrap();
    let home = step.path().join(CODEX_HOME_DIR);
    assert!(!home.join("config.toml").exists());
    assert!(!home.join("AGENTS.md").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("auth.json")).unwrap(),
        "old"
    );
    // Codex replaced the link, or on Windows changed the copy: put back.
    std::fs::remove_file(home.join("auth.json")).unwrap();
    std::fs::write(home.join("auth.json"), "refreshed").unwrap();
    return_codex_sign_in(step.path(), person.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(person.path().join("auth.json")).unwrap(),
        "refreshed"
    );
}

#[test]
fn claude_is_offered_to_nobody_until_set_by_hand() {
    let mut config = Config::default();
    assert!(offered(&config, Agent::Codex));
    assert!(!offered(&config, Agent::ClaudeCode));
    // A yes on its own doesn't reach it.
    set(&mut config, Agent::ClaudeCode, None, true);
    assert!(!allowed(&config, Agent::ClaudeCode, None));
    config.plans.offer_claude = true;
    assert!(allowed(&config, Agent::ClaudeCode, None));
}

#[test]
fn progress_goes_in_pieces_of_at_most_200_characters_in_order() {
    let text = "é".repeat(450);
    let pieces = chunks(&text, 200);
    assert_eq!(
        pieces.iter().map(|p| p.chars().count()).collect::<Vec<_>>(),
        [200, 200, 50]
    );
    assert_eq!(pieces.concat(), text);
    assert!(chunks("", 200).is_empty());
}

#[test]
fn the_shim_lists_exactly_its_tools_and_never_answers_a_call() {
    let tools =
        json!([{ "name": "gmail_search", "description": "", "inputSchema": { "type": "object" } }]);
    let init = shim::answer(
        &tools,
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
    )
    .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["capabilities"], json!({ "tools": {} }));
    let list = shim::answer(&tools, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
    assert_eq!(list["result"]["tools"], tools);
    assert!(
        shim::answer(
            &tools,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gmail_search"}}"#
        )
        .is_none()
    );
    assert!(
        shim::answer(
            &tools,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none()
    );
    let other = shim::answer(
        &tools,
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
    )
    .unwrap();
    assert_eq!(other["error"]["code"], -32601);
}

#[test]
fn the_question_names_the_plan_and_the_bot_from_this_computer() {
    assert_eq!(
        question(Agent::Codex, None, "").split('?').next().unwrap(),
        "Let Chat with Work use your ChatGPT plan for your chats"
    );
    assert!(
        question(Agent::ClaudeCode, Some("42"), "")
            .starts_with("Let Chat with Work use your Claude plan for Bot 42?")
    );
    assert!(question(Agent::ClaudeCode, Some("42"), "Issue fixer").contains("for Issue fixer?"));
    for agent in AGENTS {
        assert!(!question(agent, None, "").contains("Claude Code"));
        assert!(question(Agent::Codex, None, "").contains("OpenAI, outside the EU"));
        assert!(!title(agent).contains("Claude Code"));
    }
}
