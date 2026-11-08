# Thinking with your own ChatGPT or Claude plan

A Chat with Work chat or bot can use the ChatGPT plan (through Codex) or the Claude plan (through Claude Code) of the person whose computer this is as its model. The agent's own command-line program does the thinking on this computer, signed in the normal way; Chat with Work keeps running every tool, with its approvals, audit and taint rules, on the server. This page is what runs, what the sandbox gives it, what the computer's owner allows, and what a compromised server could do with it.

Status: Codex and Claude Code, on Linux (Bubblewrap) and macOS (Seatbelt, Claude Code only). Windows is written but not tried yet ([Testing](#testing)).

## What runs

The agent is only the model. Chat with Work sends one **step** at a time (`think`, [PROTOCOL.md 7.2.2](../PROTOCOL.md)): its instructions, the conversation so far as text, and the tools it offers. Whoever starts commands on this computer (the supervisor on Linux and macOS, the daemon on Windows) then:

1. Checks the yes in `config.toml` itself ([below](#what-the-computers-owner-allows)), that the agent is installed and signed in, and that this computer can run it in a sandbox. The worker's request names only the agent and the bot.
2. Makes a fresh folder for the step in Desktop's data folder (`<data>/thinking/<id>`, 0700) and writes the step's files there: the instructions, the conversation, the tools, and for Claude Code its MCP config. Each is a new file.
3. Starts the agent there, in a sandbox of its own, with a command line it builds itself from the agent's kind ([below](#the-command-lines)). The server never gives a word of it.
4. Reads the agent's own events as they come: the answer's text goes back as progress, and as soon as the agent asks for one or more of Chat with Work's tools, it stops the agent and answers with those calls.
5. Removes the step's folder and the run's home.

The server runs the calls as it runs any tool and sends the next step with their results in the conversation. Every step starts a fresh process: nothing is resumed, and no session is kept.

Chat with Work's tools reach the agent through `cww plan-tools`, a small MCP server over stdio that is this same `cww` program. It lists exactly the tools of the step and never runs one: a call gets no answer, since the agent is stopped as soon as it asks.

## What the sandbox gives it

A step runs as a bot's command does ([commands.md](commands.md)), with its own home and temporary folder, its limits from `[limits]` (its clock is 10 minutes), and the agent's sign-in lent exactly as a bot's coding run gets it ([coding-computers.md](coding-computers.md#claude-codes-sign-in)):

- **No files of yours.** No shared folder, ever. It writes only the step's own folder, its home, and the agent's own sign-in where the agent keeps it (`~/.codex`; `~/.claude` and `~/.claude.json`, and on macOS the login keychain), which it refreshes there. It reads the system, the toolchains, the agent's program and this `cww`.
- **The internet, and nothing of this computer.** The agent must reach its provider. On Linux with Bubblewrap the step reaches no service of this computer, none of its addresses, its containers' networks or the local network's private ranges. macOS's Seatbelt can't tell addresses apart: there it gets the internet without this computer's loopback ports. Where Linux can only use Landlock, which could only give the whole network, steps don't run (`unavailable`).
- **No tools of the agent's own.** Codex runs with `--ignore-user-config`, every built-in tool and extra turned off, its own sandbox read-only, and Chat with Work's tools as its only MCP server. Claude Code runs with `--tools ""`, `--strict-mcp-config`, and `--setting-sources ""`, so none of your settings load: no hooks, plugins, MCP servers, `CLAUDE.md` or permissions of yours. Asked to run `ls` or read a file, both say they can't.
- **Its tokens taken out.** Every token Desktop can read in the sign-in is taken out of what the agent prints, before it's read, as for bots' runs.

## The command lines

Built in `src/plans/mod.rs`, with each word quoted:

- **Codex:** `codex exec --json --skip-git-repo-check --ephemeral --ignore-user-config --ignore-rules --sandbox read-only`, then `-c features.<name>=false` for `shell_tool`, `unified_exec`, `unified_exec_tty`, `shell_snapshot`, `view_image`, `image_generation`, `multi_agent`, `multi_agent_v2`, `goals`, `sleep_tool`, `apps`, `plugins`, `remote_plugin`, `plugin_discovery_prefetch`, `hooks`, `browser_use`, `browser_use_external`, `in_app_browser`, `computer_use`, `memories`, `tool_suggest`, `skill_search`, `skill_mcp_dependency_install`, `workspace_dependencies` and `tool_call_mcp_elicitation`; `-c web_search="disabled"`; `-c model_instructions_file=<instructions>` (in place of Codex's own instructions); `mcp_servers.cww` as `cww plan-tools <tools.json>` with `default_tools_approval_mode="approve"`, `supports_parallel_tool_calls=true` and a 600-second tool timeout; `--model` when the step names one; and `-` with the conversation on standard input. Code mode stays on: the models that only call tools through it reach Chat with Work's that way, and it runs nothing else. Its events: an `agent_message` item's text, an `mcp_tool_call` item of server `cww` started (a call, with its arguments), `turn.completed` with the usage. Codex starts a model's parallel calls together; the step waits 1.5 seconds after the latest one for the others, then stops it.
- **Claude Code:** `claude -p --output-format stream-json --verbose --include-partial-messages --input-format text --tools "" --strict-mcp-config --setting-sources "" --disable-slash-commands --no-session-persistence --max-turns 4 --allowedTools 'mcp__cww__*' --mcp-config <mcp.json> --system-prompt-file <instructions>`, `--model` when the step names one, and the conversation on standard input. Its events: `text_delta`s, each `assistant` block's `tool_use` named `mcp__cww__<tool>`, and the message's `message_delta` and `message_stop`. It's stopped at the end of the first message that asked for Chat with Work's tools.

Captured streams from both are in `tests/fixtures/plans/`, and the parsers are behind one small trait (`src/plans/parse.rs`), so a step can send the events back unparsed instead (`"output": "raw"`).

## What the computer's owner allows

Only the person on this computer says which chats may think with which sign-in, in `config.toml`:

```toml
[plans.codex]
chats = true      # this computer's owner's own chats may think with Codex's sign-in
bots = ["42"]     # and these bots, by their ID in Chat with Work
[plans.claude_code]
chats = false
```

- `cww plans` (or `--json`) says what this computer has for each: installed, signed in, who pays, and the yes.
- `cww plans allow <codex|claude> [--bot 42] [--off]` gives or takes back a yes. The control API's `plans_allow` (CONTROL.md) is the same, for the app's window.
- Chat with Work can ask (`ask_plan`), never allow: the app asks in its own dialog, "Let Chat with Work use your ChatGPT plan for your chats?" or "...for Issue fixer?", with Allow and Deny, named from this computer's own settings. Without the app, a system notification says to open it, and `cww roots requests` lists what waits.

Every change is audited (`plan_asked`, `plan_allowed`, `plan_declined`, `plan_withdrawn`), and so is every step (`think`: the agent, chat, bot, how it ended, how many calls, sizes; never the instructions, the conversation, the text or the arguments). Turning a yes off takes effect at the next step; pausing the computer stops a step at once.

## Who pays

The sign-in's own plan or key: Chat with Work charges nothing for the model. `plans` says which (`plan`, `api_key` or `unknown`), from the sign-in, as for bots' runs; a Claude Code signed in with an Anthropic Console account keeps its key in `~/.claude.json` (`primaryApiKey`), which reads as `api_key`. Each provider's own terms and limits apply to a plan used this way.

## What a compromised server could do

Spend the person's plan, within its own limits, on any text it sends, for the chats the person allowed: the server writes the instructions and the conversation, so it decides what the agent reads and what it answers. Nothing more: the agent gets no files of the person's, no shell, no tools but those the server names (which run on the server, where they always did), no local services, and the command line is Desktop's. It never learns a token, a path or a version.

What the agent's own program does with the sign-in it's lent is as for bots' coding runs ([coding-computers.md](coding-computers.md#claude-codes-sign-in)): it could change its own settings there, which the person's next `codex` or `claude` would read. With no tools, the model has no way to make it.

## Testing

**2026-11-08, chalk (Arch, Bubblewrap) and the Mac mini (macOS 27, Seatbelt), with test homes holding API keys, never a person's sign-in or daemon:**

- `real_agent_thinks_from_the_sandbox` (`CWW_TEST_AGENT`, `CWW_TEST_PERSON_HOME`, `CWW_TEST_CWW`, `CWW_TEST_PROGRAM`, `CWW_TEST_MODEL`): a real Codex 0.167.0 (chalk) and a real Claude Code 2.1.319 (chalk) and 2.1.299 (Mac) each answered a step in text, called a tool and were stopped with the call returned, and, asked to run `ls -la ~` and print their own sign-in, said they had no tool for it; no token appeared in anything a step returned. The Codex test home's `config.toml` named instructions and an MCP server that would have left a file: neither was read. The Claude Code test home had a hook and a `CLAUDE.md`: neither ran.
- In Seatbelt, Codex still exits at once, as for bots' runs ([coding-computers.md](coding-computers.md#codexs-sign-in)), so on macOS it's `unavailable`.
- The unit tests (both parsers on captured streams, the yes, arguments, command lines, progress, the MCP shim), a sandbox test with a stand-in agent (stopped at its calls, token taken out, folder removed, raw output), and an end-to-end test through the tunnel (`plans`, `ask_plan` and its answer, `think` with progress, the yes turned off).

Not tried: a plan's own sign-in (a ChatGPT or Claude subscription, rather than an API key), and Windows, where the command line goes through `cmd.exe` to give the agent its conversation.
