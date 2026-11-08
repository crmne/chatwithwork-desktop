//! A person's own ChatGPT (Codex) or Claude (Claude Code) sign-in as the
//! model of a Chat with Work chat or bot (docs/own-plans.md).
//!
//! The agent is only the model. Chat with Work sends one step at a time
//! (`think`): its instructions, the conversation so far as text, and its own
//! tools. Whoever starts commands (the supervisor, or the daemon on Windows)
//! starts the agent in a sandbox of its own, in a fresh folder, with none of
//! its own tools: Chat with Work's tools are its only ones, served by `cww
//! plan-tools` ([`shim`]), which never runs one. As soon as the agent asks
//! for them, the agent is stopped and the calls go back to the server, which
//! runs them as it runs any tool and sends the next step with their results.
//!
//! Only the person on this computer says which chats may do this
//! (`[plans]` in `config.toml`, `cww plans allow`, or the app's dialog after
//! `ask_plan`): the server can only ask.

pub mod parse;
pub mod shim;
#[cfg(test)]
mod tests;

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::coding::{Agent, Billing, Login};
use crate::config::{Config, PlanYes};
use crate::paths::Paths;

/// The agents a chat can think with.
pub const AGENTS: [Agent; 2] = [Agent::Codex, Agent::ClaudeCode];

/// The longest a step may take.
pub const STEP_SECONDS: u64 = 600;
/// Most tools a step offers.
pub const MAX_TOOLS: usize = 256;
/// Longest tool name: Anthropic's and OpenAI's limit, less the room the
/// agents' own prefixes take.
pub const MAX_TOOL_NAME: usize = 64;

/// `codex` or `claude_code`, as the wire names them; `claude` too, as a
/// person types it.
pub fn agent(key: &str) -> Option<Agent> {
    match key {
        "codex" => Some(Agent::Codex),
        "claude_code" | "claude" => Some(Agent::ClaudeCode),
        _ => None,
    }
}

/// What this computer's own windows call it: never "Claude Code".
pub fn name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude",
        _ => agent.name(),
    }
}

/// Whose plan a yes lends.
pub fn plan_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "ChatGPT",
        _ => "Claude",
    }
}

fn yes(config: &Config, agent: Agent) -> &PlanYes {
    match agent {
        Agent::Codex => &config.plans.codex,
        _ => &config.plans.claude_code,
    }
}

fn yes_mut(config: &mut Config, agent: Agent) -> &mut PlanYes {
    match agent {
        Agent::Codex => &mut config.plans.codex,
        _ => &mut config.plans.claude_code,
    }
}

/// Whether the person on this computer let `agent` think for this
/// computer's owner's own chats (`bot` is `None`) or for that bot.
pub fn allowed(config: &Config, agent: Agent, bot: Option<&str>) -> bool {
    if !AGENTS.contains(&agent) {
        return false;
    }
    let yes = yes(config, agent);
    match bot {
        None => yes.chats,
        Some(bot) => yes.bots.iter().any(|b| b == bot),
    }
}

/// Give or take back the yes. Returns whether anything changed.
pub fn set(config: &mut Config, agent: Agent, bot: Option<&str>, allow: bool) -> bool {
    let yes = yes_mut(config, agent);
    match bot {
        None => std::mem::replace(&mut yes.chats, allow) != allow,
        Some(bot) => {
            let had = yes.bots.iter().any(|b| b == bot);
            if allow && !had {
                yes.bots.push(bot.to_string());
            } else if !allow && had {
                yes.bots.retain(|b| b != bot);
            }
            had != allow
        }
    }
}

/// Who a yes is for, as the audit log and this computer's windows say it.
pub fn for_whom(bot: Option<&str>) -> String {
    match bot {
        None => "your chats".into(),
        Some(bot) => format!("bot {bot}"),
    }
}

/// A bot's ID as Chat with Work gives it: digits and letters, short.
pub fn valid_bot(bot: &str) -> bool {
    !bot.is_empty()
        && bot.len() <= 20
        && bot
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One agent as `plans` answers it: never a path, a version or a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLook {
    pub agent: String,
    pub name: String,
    pub installed: bool,
    pub signed_in: bool,
    pub billing: Billing,
    /// This computer said yes for its owner's own chats.
    pub chats: bool,
    /// It said yes for the bot asked about; `null` without one.
    pub bot: Option<bool>,
    /// Why a step can't run here, if it can't.
    pub unavailable: Option<String>,
}

/// What this computer has for each agent, for `bot` if a bot asks.
/// `sandbox` says why steps can't run here, if they can't.
pub fn look(
    config: &Config,
    paths: &Paths,
    bot: Option<&str>,
    sandbox: Option<String>,
) -> Vec<PlanLook> {
    let home = paths.person_home.as_deref();
    AGENTS
        .into_iter()
        .map(|agent| {
            let login = home.map(|h| Login::find_for(agent, h)).unwrap_or_default();
            PlanLook {
                agent: agent.key().into(),
                name: name(agent).into(),
                installed: crate::coding::setup::program_for_run(
                    agent,
                    &config.coding,
                    &config.roots,
                )
                .is_some(),
                signed_in: login.signed_in(),
                billing: login.billing,
                chats: allowed(config, agent, None),
                bot: bot.map(|bot| allowed(config, agent, Some(bot))),
                unavailable: sandbox
                    .clone()
                    .or_else(|| agent.unavailable().map(String::from)),
            }
        })
        .collect()
}

/// A tool Chat with Work offers a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
}

/// One step, as the worker asks whoever starts commands for it. Checked
/// again there ([`validate`]): it trusts nothing it's sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkAsk {
    pub agent: String,
    /// The bot whose chat thinks, `None` for this computer's owner's own.
    #[serde(default)]
    pub bot: Option<String>,
    pub instructions: String,
    pub transcript: String,
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub model: Option<String>,
    /// Send the agent's own events back as they come, unparsed.
    #[serde(default)]
    pub raw: bool,
}

/// The agent `ask` names, or why it can't be run as it is.
pub fn validate(ask: &ThinkAsk) -> Result<Agent, String> {
    let agent = agent(&ask.agent)
        .filter(|_| ask.agent != "claude")
        .ok_or("`agent` is `codex` or `claude_code`")?;
    if let Some(bot) = &ask.bot
        && !valid_bot(bot)
    {
        return Err("that isn't a bot's ID".into());
    }
    if ask.instructions.contains('\0') || ask.transcript.contains('\0') {
        return Err("`instructions` and `transcript` are text without NUL".into());
    }
    if ask.transcript.trim().is_empty() {
        return Err("`transcript` is the conversation so far, and it's empty".into());
    }
    if ask.tools.len() > MAX_TOOLS {
        return Err(format!("at most {MAX_TOOLS} tools"));
    }
    let mut seen = std::collections::HashSet::new();
    for tool in &ask.tools {
        let plain = !tool.name.is_empty()
            && tool.name.len() <= MAX_TOOL_NAME
            && tool
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !plain {
            return Err(format!(
                "a tool's name is 1 to {MAX_TOOL_NAME} letters, digits, `_` or `-`"
            ));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err("each tool's name is its own".into());
        }
        if !tool.input_schema.is_object() {
            return Err("a tool's `input_schema` is a JSON Schema object".into());
        }
    }
    if let Some(model) = &ask.model
        && !valid_model(model)
    {
        return Err(
            "`model` is a model's name: letters, digits and `.` `_` `-` `:` `/` `[` `]`".into(),
        );
    }
    Ok(agent)
}

/// A model's name the agents take: what `--model` / `-m` accepts, and
/// nothing a shell or TOML would read as more.
pub fn valid_model(model: &str) -> bool {
    let mut bytes = model.bytes();
    model.len() <= 100
        && bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b"._-:/[]".contains(&b))
}

/// The files a step's agent reads, in its own folder.
pub const INSTRUCTIONS_FILE: &str = "instructions.md";
pub const PROMPT_FILE: &str = "prompt.md";
pub const TOOLS_FILE: &str = "tools.json";
pub const MCP_FILE: &str = "mcp.json";

/// The server name the agent knows Chat with Work's tools by.
pub const SERVER: &str = "cww";

/// Write the step's files into `dir`, a folder made for it alone: each
/// new, never through anything already there.
pub fn write_files(dir: &Path, ask: &ThinkAsk, agent: Agent, shim: &Path) -> std::io::Result<()> {
    let tools: Vec<Value> = ask
        .tools
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
        .collect();
    let mut files = vec![
        (INSTRUCTIONS_FILE, ask.instructions.clone().into_bytes()),
        (PROMPT_FILE, ask.transcript.clone().into_bytes()),
        (TOOLS_FILE, serde_json::to_vec(&tools)?),
    ];
    if agent == Agent::ClaudeCode {
        let config = json!({ "mcpServers": { SERVER: {
            "type": "stdio",
            "command": display(shim),
            "args": ["plan-tools", display(&dir.join(TOOLS_FILE))],
        } } });
        files.push((MCP_FILE, serde_json::to_vec(&config)?));
    }
    for (name, bytes) in files {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        options.open(dir.join(name))?.write_all(&bytes)?;
    }
    Ok(())
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Codex's features a step turns off: every tool and extra of its own.
/// Code mode stays: models that only call tools through it reach Chat with
/// Work's that way, and it runs nothing but those tools.
pub const CODEX_FEATURES_OFF: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "unified_exec_tty",
    "shell_snapshot",
    "view_image",
    "image_generation",
    "multi_agent",
    "multi_agent_v2",
    "goals",
    "sleep_tool",
    "apps",
    "plugins",
    "remote_plugin",
    "plugin_discovery_prefetch",
    "hooks",
    "browser_use",
    "browser_use_external",
    "in_app_browser",
    "computer_use",
    "memories",
    "tool_suggest",
    "skill_search",
    "skill_mcp_dependency_install",
    "workspace_dependencies",
    "tool_call_mcp_elicitation",
];

/// The command line for a step of `agent` in `dir`, built here from the
/// agent's kind alone: the server never gives a word of it. `shim` is this
/// `cww`, which serves the tools; `model` was checked by [`valid_model`].
/// `sh` on Linux and macOS; on Windows `cmd.exe` reads the prompt, as
/// PowerShell 5.1 would re-encode it.
pub fn command(agent: Agent, dir: &Path, shim: &Path, model: Option<&str>) -> String {
    let words = arguments(agent, dir, shim, model);
    let prompt = display(&dir.join(PROMPT_FILE));
    if cfg!(windows) {
        let line: Vec<String> = words.iter().map(|w| cmd_quote(w)).collect();
        format!(
            "cmd.exe /d /s /c \"{} < {}\"",
            line.join(" "),
            cmd_quote(&prompt)
        )
    } else {
        let line: Vec<String> = words.iter().map(|w| shell_quote(w)).collect();
        format!("exec {} < {}", line.join(" "), shell_quote(&prompt))
    }
}

/// The agent's program and its arguments.
pub fn arguments(agent: Agent, dir: &Path, shim: &Path, model: Option<&str>) -> Vec<String> {
    let file = |name: &str| display(&dir.join(name));
    let mut words: Vec<String> = Vec::new();
    match agent {
        Agent::Codex => {
            words.extend(
                [
                    "codex",
                    "exec",
                    "--json",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--sandbox",
                    "read-only",
                ]
                .map(String::from),
            );
            let mut config: Vec<String> = CODEX_FEATURES_OFF
                .iter()
                .map(|f| format!("features.{f}=false"))
                .collect();
            config.extend([
                "web_search=\"disabled\"".to_string(),
                format!(
                    "model_instructions_file={}",
                    toml_string(&file(INSTRUCTIONS_FILE))
                ),
                format!(
                    "mcp_servers.{SERVER}.command={}",
                    toml_string(&display(shim))
                ),
                format!(
                    "mcp_servers.{SERVER}.args=[\"plan-tools\", {}]",
                    toml_string(&file(TOOLS_FILE))
                ),
                // Its calls reach `plan-tools`, which never answers, so a
                // step stops at them, never at a refusal the model would read.
                format!("mcp_servers.{SERVER}.default_tools_approval_mode=\"approve\""),
                format!("mcp_servers.{SERVER}.supports_parallel_tool_calls=true"),
                format!("mcp_servers.{SERVER}.tool_timeout_sec={STEP_SECONDS}"),
                format!("mcp_servers.{SERVER}.startup_timeout_sec=30"),
            ]);
            for value in config {
                words.push("-c".into());
                words.push(value);
            }
            if let Some(model) = model {
                words.push("--model".into());
                words.push(model.into());
            }
            words.push("-".into());
        }
        _ => {
            words.extend(
                [
                    "claude",
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--input-format",
                    "text",
                    "--tools",
                    "",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "",
                    "--disable-slash-commands",
                    "--no-session-persistence",
                    "--max-turns",
                    "4",
                    "--allowedTools",
                ]
                .map(String::from),
            );
            words.push(format!("mcp__{SERVER}__*"));
            words.push("--mcp-config".into());
            words.push(file(MCP_FILE));
            words.push("--system-prompt-file".into());
            words.push(file(INSTRUCTIONS_FILE));
            if let Some(model) = model {
                words.push("--model".into());
                words.push(model.into());
            }
        }
    }
    words
}

/// `text` as a TOML basic string: JSON's escapes are TOML's.
fn toml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// `text` as one word of `sh`, in single quotes.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `text` as one word of `cmd.exe`: quoted, its own quotes doubled, as the
/// C runtime's argument parsing reads them.
fn cmd_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:=/\\".contains(&b))
    {
        return text.to_string();
    }
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// What the person is asked on this computer, from its own data: which
/// plan, and for whom (the bot's name a loan here gave it, or its number).
pub fn question(agent: Agent, bot: Option<&str>, bot_name: &str) -> String {
    let whom = match bot {
        None => "your chats".to_string(),
        Some(bot) if bot_name.trim().is_empty() => format!("Bot {bot}"),
        Some(_) => bot_name.trim().to_string(),
    };
    format!(
        "Let Chat with Work use your {} plan for {whom}? It thinks with {}'s sign-in on this \
         computer, with no files or commands here: Chat with Work runs its tools, each as \
         you allow there.",
        plan_name(agent),
        name(agent)
    )
}

/// The dialog's title.
pub fn title(agent: Agent) -> String {
    format!("Use your {} plan", plan_name(agent))
}

/// The name this computer gave the bot `bot` anywhere it was lent
/// something, or nothing.
pub fn bot_name(config: &Config, bot: &str) -> String {
    config
        .roots
        .iter()
        .filter_map(|root| root.loan(bot))
        .map(|loan| loan.name.trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_default()
}

/// Split `text` into pieces of at most `max` characters, in order.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut count = 0;
    for c in text.chars() {
        if count == max {
            out.push(std::mem::take(&mut piece));
            count = 0;
        }
        piece.push(c);
        count += 1;
    }
    if !piece.is_empty() {
        out.push(piece);
    }
    out
}
