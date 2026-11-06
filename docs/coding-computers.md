# Lending this computer to a coding bot

A coding bot in Chat with Work (the Issue fixer with Claude Code, the Dependency upgrader on a computer, and bots like them) runs commands on a computer lent to it: it fetches the repository, has Claude Code (`claude -p`) make a change in a fresh worktree, runs the tests, and then commits the changed files through Chat with Work's GitHub App. This page is how a person gets a computer ready for that in one guided step, on Linux, macOS and Windows, and what the computer then holds to.

Status: built in `cww coding` ([below](#the-guided-setup)) and tried on Linux, macOS and Windows on 2026-11-03 ([Testing](#testing)); the desktop app's page for it is not built yet ([Not built yet](#not-built-yet)).

**Nothing here widens what the server can do.** Every choice is made on this computer: which folder, which bot, whether its commands run unattended, whether it gets your Claude Code sign-in. The server learns only whether a lent folder is ready, never a token, a path or a repository's address.

## What it took by hand

Setting up Plenty's NUC took six steps a customer couldn't do: share a folder with changes, commands and the bot; install Claude Code; run `claude setup-token` and put the token in a `.env` file the sandbox could read through a `claude` wrapper; work around the command `PATH`, which found mise's `claude` before the wrapper; clone the repository and keep it fresh with a timer and a deploy key; install Ruby and the gems.

Two of those were unsafe, and this design removes them:

- **The token sat in a file in the lent folder.** The deny list kept it from Chat with Work's tools, but not from the commands the bot runs there ([commands.md, What it doesn't protect against](commands.md#what-it-doesnt-protect-against)), any of which could print it. Desktop now uses Claude Code's own sign-in where Claude Code keeps it ([below](#claude-codes-sign-in)), never a copy in a shared folder.
- **The mirror was fetched, outside the sandbox, from inside the lent folder.** A command the bot runs can write that mirror's `.git/config` (`core.sshCommand`, `core.fsmonitor`, a credential helper), and the timer's next `git fetch` would run it as the person, outside every sandbox.

## Without any setup: the bot asks

A coding bot needs nothing typed on the computer. What it needs is what the computer already has: a folder shared with the bot that allows commands and the network (the folder's settings in the app, as for any bot), Claude Code installed and signed in the normal way (`claude`, then signing in), and the person's yes. When the bot's computer says Claude Code isn't allowed for it in a lent folder (`coding.claude_code` is `not_allowed`), Chat with Work asks the computer (`ask_claude_code`, PROTOCOL.md 7.2.1), and the desktop app asks in a dialog of its own, named from this computer's settings: "Let Issue fixer run Claude Code in bots?", saying what that gives the bot's runs ([below](#claude-codes-sign-in)), with Allow and Deny. Allow gives that bot's runs in that folder this computer's Claude Code sign-in, as `cww coding allow` does; Deny drops the request. Only the person on this computer answers: in that dialog, with `cww roots allow <root> <bot> --claude-code` or `cww roots decline <root> <bot> --claude-code`, or through the control channel. The server can only ask. When the app isn't running, a system notification says to open it, and `cww roots requests` lists what waits. A request lasts 24 hours, and needs a folder lent to the bot with commands allowed.

A run then finds `claude` itself where no setup recorded one, or the recorded one is gone (the `PATH` of whoever starts runs, the native installer's `~/.local/bin`, mise's install, Homebrew, npm's global programs), never one in a shared folder a command could change. It only looks: finding it runs nothing outside the sandbox. `cww coding setup` does run the `claude` it offers you, once, and never one in `~/.claude` once a bot's runs can change that folder. The guided setup below remains for scripts and servers, and for the repository mirror.

## The guided setup (for scripts)

```
cww coding setup
cww coding setup --bot "Issue fixer" --repo git@github.com:acme/app.git --folder ~/bots
cww coding setup --bot 42 --repo git@github.com:acme/app.git --ssh-key ~/.ssh/app-deploy
cww coding status [--bot 42] [--json]
cww coding allow <root> <bot> [--off]   # this bot's runs there use Claude Code's sign-in, or stop
cww coding fetch         # fetch every repository's mirror now
```

`setup` asks one thing at a time, says what it's about to do before it does it, and prints each step's state as it goes (`ok`, `needs you`, `skipped`, `failed: why`). Run again, it skips what's done. Every answer can be given as a flag, so the desktop app (and a person over SSH) can drive the same steps.

1. **Paired.** The computer is paired and the daemon runs, or it says to run `cww login`.
2. **Bot.** Which bot: it lists the bots you can read, each with its project and organization, and you type part of its name until one is left (letters in order are enough: "isfx" finds "Issue fixer"). `--bot` takes a name, part of one, or an ID; with `--yes` or no terminal, a name that matches more than one bot is refused, saying which. An ID is that bot, without listing them (`--name` names it here).
3. **Folder.** Pick a shared folder or make one (default `~/bots`, `%USERPROFILE%\bots` on Windows). It shares it, allows changes, commands and the network (Claude Code needs the network), and lends it to the bot, each after saying what it allows, as `cww roots allow-commands` does. It asks separately whether the bot's commands there may start **unattended**; the default is no, so each asks you in Chat with Work.
4. **Claude Code.** It looks for `claude` where you'd run it (your `PATH`, then the native installer's `~/.local/bin`, Homebrew, npm's global programs) and runs `claude --version`. Missing, it offers Anthropic's own installer (`curl -fsSL https://claude.ai/install.sh | bash`, or `irm https://claude.ai/install.ps1 | iex` on Windows) after showing the command, or prints it to run yourself. It remembers the program it found by its full path.
5. **Sign-in.** It checks that Claude Code is signed in on this computer the normal way (you ran `claude` and signed in). If not, it says to run `claude` once and sign in, then setup again. Then it asks, once, on this computer: "Let bots' coding runs use this computer's Claude Code sign-in?", Yes or No (No by default, yes with `--yes`), after saying what that gives the bot's runs (below). Nothing is copied: the runs use the sign-in where Claude Code keeps it.
6. **Repository.** It asks for the repository's address and clones it the way [Repositories](#repositories) says: a mirror only Desktop writes, fetched with your own Git credentials, and a checkout in the folder that fetches from the mirror.
7. **Toolchain.** It reads the checkout for what it needs (`mise.toml` or `.tool-versions`, `Gemfile`, `package.json` and its lockfile, `Cargo.toml`, `pyproject.toml` with `uv.lock` or `requirements.txt`, `go.mod`) and lists what's missing on this computer. It offers `mise install` where the repository uses mise (and mise is here), and the dependency install the lockfile names (`bundle install`, `npm ci`, `pnpm install --frozen-lockfile`, `yarn install --immutable`, `uv sync`, `cargo fetch`). These run as you, outside the sandbox, and run the repository's own install scripts: it says so and asks first. They run in a private copy of the repository made from the mirror in Desktop's data folder, never in the checkout in the shared folder, whose install files a bot's commands could change before they run, and through the repository's mise environment where it uses mise. What carries over is what installs for the whole computer (mise's toolchains, Ruby gems, Cargo's registry); a project's own folders (`node_modules`, `.venv`) are made by the bot's commands in the checkout. Otherwise it says what to install.
8. **Proof.** It runs the repository's test command once, in the sandbox a bot's command gets, as that bot (`bin/rails test`, `npm test`, `cargo test`, `pytest`, `go test ./...`, or `mise run test`; you can change it), then `claude -p 'Reply with the word ready.' --max-turns 1` there with the bot's sign-in, which proves the right `claude` is found, signed in, and reaches Anthropic. Both print how they ended and the end of their output.

At the end it prints what Chat with Work will see: "NUC: Claude Code ready for Issue fixer in bots (app)".

## Claude Code's sign-in

**Where it lives.** Where Claude Code itself keeps it, for whoever runs `claude` on this computer: Desktop never stores, copies or asks for it. Claude Code keeps its account and preferences in `~/.claude.json` and everything else in `~/.claude`; its tokens are in `~/.claude/.credentials.json` (0600) on Linux and Windows, and in the login keychain (`Claude Code-credentials`) on macOS. Desktop looks only in those standard places, in the real home folder: never a folder `CLAUDE_CONFIG_DIR` or an account switcher points at. `config.toml` records only which program `claude` is, and per loan whether its runs use the sign-in:

```toml
[coding]
claude = "/home/carmine/.local/bin/claude"

[[roots]]
id = "bots"
# ...
[[roots.bots]]
id = "42"
name = "Issue fixer"
claude_login = true    # this bot's runs here use this computer's Claude Code sign-in
```

**Who gets it.** Only a command a bot runs, in a folder lent to that bot, whose loan says `claude_login = true`, which only the person sets on this computer: `cww coding setup` asks "Let bots' coding runs use this computer's Claude Code sign-in?", and `cww coding allow <root> <bot>` is the same yes. Never a person's own chats' commands, another bot's, or a folder not lent. Whoever starts the command (the supervisor on Linux and macOS, the daemon on Windows) checks the loan in `config.toml` itself; the worker's request only names the bot.

**How the sandbox maps it.** A run keeps its own home and temporary folder, as every run does. For a run with the sign-in, the sandbox also lets it read and write exactly Claude Code's own paths, where they really are, because Claude Code refreshes its tokens there:

| | What the run reads and writes | How Claude Code finds it |
|---|---|---|
| Linux (Bubblewrap; Landlock where Bubblewrap can't run) | `~/.claude` and `~/.claude.json`, bound where they are | links named `.claude` and `.claude.json` in the run's own home lead there |
| macOS (Seatbelt) | `~/.claude` and `~/.claude.json`, and the login keychain (MXC's `keychainAccess`: `securityd` and `~/Library/Keychains`) | the same links; the tokens come from the keychain item, as outside the sandbox |
| Windows (process container) | `%USERPROFILE%\.claude` (a process container grants folders only, so not `.claude.json`) | `CLAUDE_CONFIG_DIR`, Claude Code's own variable, names that folder |

Nothing else of the home folder is writable, and little else is there: not `~/.ssh`, not your other projects. What every run reads still holds, the toolchains' folders included, where any link in them leads (a `~/.config/mise` that links to your dotfiles reads all of them), and their settings can carry variables ([commands.md](commands.md#what-it-doesnt-protect-against)). Desktop leaves out a `~/.claude` that links to a folder not named `.claude` (`~/.ssh`, say) or to a broad one (the home folder, a drive), or that holds a shared folder or Desktop's own, and a `~/.claude.json` that's a link to another file; it reads `.credentials.json` only as a regular file of at most 64 KiB. A run also gets `DISABLE_AUTOUPDATER=1` (it can't write Claude Code's program folder) and `CLAUDE_CODE_TMPDIR` in its own temporary folder.

**The right `claude`.** For such a run the sandbox reads the folder holding the program setup recorded (for the native installer, `~/.local/share/claude/versions`, where `~/.local/bin/claude` leads), and `PATH` starts with a folder of the run's own holding only a link named `claude` to it (on Windows, the folder holding `claude.exe` goes first). mise's shims and everything else stay after it, so a `claude` that mise or npm also installed doesn't shadow it, and no wrapper is needed.

**Not signed in.** `cww coding status` and setup say "Claude Code isn't signed in on this computer: run `claude` once and sign in". The server's `roots` comes from `config.toml`, which doesn't know, so a run there hears it from Claude Code itself ("Not logged in").

**What it can't promise.** A command given the sign-in can use it, and a command can be any line of shell:

- **It can read the tokens**, refresh token included, in `~/.claude/.credentials.json` (or from the keychain item on macOS), and with the network, send them somewhere. Desktop removes the exact text of every token it can read there when the run starts from what the run prints before it leaves the computer (`run`, `run_status`, `run_output`, this computer's own windows), replacing it with `[claude-code-token]`, across chunk boundaries. That stops a token being read back by accident or by a plain `cat`; it doesn't stop a command that encodes it first, writes it to a file the bot then reads, or sends it out, and a token Claude Code refreshed during the run isn't known to Desktop.
- **It can read everything else in `~/.claude` and `~/.claude.json`**: your Claude Code history (`history.jsonl`, `projects/`), settings, and the MCP servers you configured.
- **It can change them, and what it writes there runs outside the sandbox later.** Claude Code's settings hold hooks, MCP servers, plugins and instructions (`~/.claude/settings.json`, `~/.claude.json`, `~/.claude/CLAUDE.md`), and the next `claude` you run yourself starts them as you, with no sandbox. A command given the sign-in can therefore reach your account through your own next Claude Code session.
- **On macOS it reaches the login keychain** as Claude Code does, through `securityd`: items whose access list allows the `security` command (those other command-line programs saved with it, too) can be read without a prompt.
- Claude Code passes its environment to the commands it runs itself.

So the sign-in is as safe as the commands you approve for that bot. Approving each one (the default) means you see every command line before it runs; an unattended loan means a compromised server, or a bot misled by what it read, could run one that takes the tokens or leaves a hook. If that may have happened, sign out of Claude Code (`claude`, then `/logout`) and revoke its access at claude.ai (Settings, Claude Code), check `~/.claude/settings.json` and `~/.claude.json` for hooks and MCP servers you didn't add, and sign in again. `cww coding allow <root> <bot> --off` stops the sign-in for that bot's next runs at once.

**From 0.13.1 and earlier.** Those versions kept a token from `claude setup-token`, pasted into `cww coding sign-in`, in this computer's secret store. That's gone with `cww coding sign-in` and `sign-out`: the first `cww coding` command after the update deletes the stored token and says so (unpairing deletes it too). Revoke it at claude.ai (Settings, Claude Code) if you no longer use it. A loan's old `claude_code = true` doesn't carry over: that yes covered one token, and this gives a run Claude Code's files, so it's asked for again. Until then the bot's `roots` says `not_allowed`, and its request brings up the dialog; on a server with no app, run `cww coding allow <root> <bot>`. Sign in to Claude Code on that computer too if you only ever used a token there.

## Repositories

The mirror and its fetching stay out of the sandbox's reach; the checkout the bot works in has no credentials at all.

- **The mirror** is a bare clone in Desktop's data folder (`<data>/repositories/<root>/<name>.git`), which no command can write (setup refuses, and Desktop never fetches a mirror, where a shared folder that allows changes holds the data folder): the sandbox reads it, for runs in that folder only. Its config is Desktop's alone, so fetching it runs nothing a bot wrote. It fetches branches and tags (`+refs/heads/*:refs/heads/*`), not pull requests' refs.
- **Fetching** is Desktop's: `git fetch --prune` on the mirror, with your own Git credentials (your SSH keys and `~/.ssh/config`, never the system's `/etc/ssh` config, which ssh refuses inside the background service's user namespace; your credential helper), hooks off, run by whoever starts commands (the supervisor, or the daemon on Windows) before a command starts in that folder if the last fetch is over a minute old, with a 60-second limit, and by `cww coding fetch`. A fetch that fails doesn't stop the command; `cww coding status` shows the last failure. Nothing else runs `git` outside the sandbox, and never in a shared folder.
- **The checkout** is `git clone <mirror> <name>`, run in the sandbox in the folder: its `origin` is the mirror's path, so `git fetch origin` in a command works without a network, keys or tokens, and a push fails. `config.toml` records the repository's address, its checkout and its folder:

```toml
[[coding.repositories]]
root = "bots"
name = "app"
url = "git@github.com:acme/app.git"
```

An address with a password or token in it (`https://user:token@...`) is refused: use an SSH key or a credential helper. For a deploy key (read-only, for this one repository), `--ssh-key ~/.ssh/deploy-key` fetches with only that key, never the SSH agent's or one your SSH config names (that config isn't read then); it must be outside every shared folder, and `config.toml` records only its path (`ssh_key`). Desktop writes nothing in the shared folder itself: the checkout is made by a command in the sandbox, as the bot (`git clone` from the mirror), which can change only that folder, so a link a command left there can't redirect it. A checkout already there is kept, and the same kind of command points its `origin` at the mirror and drops any push address. Changes reach GitHub only through Chat with Work's GitHub App, which commits the files the bot names from the computer and opens the pull request, each approved as the bot's policy says.

## Letting a bot's commands run unattended from its card

A bot's command approval card in Chat with Work (for example "Start fixing #12 with Claude Code?") can offer **Always allow**. That doesn't allow anything: the server calls `ask_unattended` for the folder (PROTOCOL.md 7.2.1), the card says "Asked on <computer>: allow it there", and its own Approve still covers only that one command. On the computer, the desktop app asks in a dialog of its own, named from this computer's settings: "Issue fixer asks to run commands in bots without asking each time. They still run in the folder's sandbox and are recorded. Allow?", with Allow and Not now. When the app isn't running, a system notification says to open it, and `cww roots requests` lists what's waiting: `cww roots allow <root> <bot>` allows it (recorded, as the dialog's Allow is), `cww roots decline <root> <bot>` doesn't. Allow is the same as sharing with unattended commands; nothing else, and no page, can answer. A request lasts 24 hours, and needs a folder lent to the bot with commands allowed.

## What Chat with Work sees

In a bot's `roots` (PROTOCOL.md 7.2.1), each lent folder says whether Claude Code is ready for that bot there, and which repositories it holds, by name:

```json
{ "id": "bots", "label": "bots", "unattended": false,
  "coding": { "claude_code": "ready", "repositories": ["app"] } }
```

`claude_code` is `ready`; `not_allowed` (this bot's runs here don't use the sign-in: the server asks, above); or `no_network` (this folder's commands have no network, which Claude Code needs). It's what `config.toml` says, checked again when a run starts; whether Claude Code is installed and signed in isn't in it (the worker that answers can't look in the home folder), so `not_installed` and `not_signed_in` come only from `cww coding status`, and a run hears them from the shell or Claude Code itself. No path, address, version or token.

The server keeps it with the folders it already remembers for the bot, and the bot's page says what the computer still needs, with the command that fixes it: "NUC hasn't given this bot its Claude Code sign-in in bots. On NUC, run `cww coding setup --bot 42`." The server asks the computer's `roots` for the bot when the page shows a computer it hasn't heard from in a while, as it does for a bot's own runs. The recipes that run Claude Code assume this setup: `claude` is found and signed in, `git fetch origin` works without credentials, and when `roots` says Claude Code isn't ready, the bot stops and tells its team what to run, rather than trying.

## Security

The rules in [SECURITY.md](../SECURITY.md) and [commands.md](commands.md) all still hold; this adds:

- **Only this computer decides.** The `[coding]` table, `claude_login` on a loan, and the repositories are changed only by `cww coding`, `cww roots`, the control API (the app's dialog), or editing `config.toml`, never by the server. The server can ask for unattended commands (`ask_unattended`) and for Claude Code (`ask_claude_code`), and the person answers here. Turning a folder's commands off, or making it read-only, ends its loans' `claude_code` as it ends `unattended`.
- **The sign-in never reaches the worker, the server or the audit log.** Only a run whose loan allows it reads Claude Code's own paths, and the tokens Desktop can read there are removed from the run's output. It's never copied: Desktop stores no token of its own.
- **Desktop runs `git` outside the sandbox only on its own mirror**, with fixed arguments, hooks off, and a config no command can write. It never runs anything outside the sandbox in a shared folder. The setup's toolchain step is the one exception, run by the person, in their terminal, after they agreed, before any bot has touched the checkout.
- **What a compromised server could do** in a folder lent with `claude_login` and commands: what it can already do there (run commands, as approvals or the unattended loan allow), plus use the person's Claude Code subscription, read their Claude Code history, with the network try to leak its tokens, and change Claude Code's settings so the person's own next `claude` runs something outside the sandbox ([What it can't promise](#claude-codes-sign-in)). It can't learn a token from `roots`, reach another folder, push to the repository, or change any of these Desktop settings.

## Testing

Tried for real on 2026-11-03, with the setup token 0.13.1 and earlier used, and the computer's own sign-in on 2026-11-05:

- **Linux (Arch, Bubblewrap), chalk, with the computer's own sign-in.** A side-by-side daemon built from this change, with the `solco` folder lent to bot 24 and `claude_code` on, started `claude -p 'Reply with the word ready.' --max-turns 1` as the bot through `coding_check`: Claude Code (mise's `claude`) answered "ready" with the account in `~/.claude.json`, and the run saw its own home with links to `~/.claude` and `~/.claude.json`, and not `~/.ssh` or `~/Work`. A person's run, and another bot's, in the same folder saw no `~/.claude`.
- **macOS 27 (Seatbelt), the Mac mini, with the computer's own sign-in.** The sandbox tests passed in Seatbelt (`CWW_TEST_SEATBELT=1`): the bot's run reads and writes `~/.claude` and `~/.claude.json` and nothing else of the home folder, other runs see none of it, and the tokens are taken out of what it prints. The real Claude Code, from an SSH session, said "Not logged in": an SSH session can't unlock the login keychain, outside the sandbox too (`security` answers 36, interaction not allowed). From the desktop session, where the background service runs, it hasn't been tried yet.

- **Linux (Fedora 43, Bubblewrap), Plenty's NUC.** Moved from the hand-made setup: `cww coding sign-in --stdin` from the old `.env` file, then `cww coding setup --bot 24 --repo ... --ssh-key ...`, which kept the existing checkout and pointed its `origin` at the new mirror. Through the daemon, as bot 24, `bin/rails test` passed in the sandbox with the network, and `claude -p` answered from it, with `claude` on mise's `installs/claude/latest` link. The daemon fetched the stale mirror itself before the run, with the deploy key. Trying it found two bugs, fixed before this was called built. Bubblewrap can't start with a link given as a folder to bind (the program's folder is now bound where it really is), and ssh refused the system's `/etc/ssh` config inside the service's user namespace (Desktop's fetches now use `~/.ssh/config`).
- **macOS 27 (Seatbelt), the Mac mini.** The real Claude Code answered from the sandbox with a setup token (`real_claude_code_answers_from_the_sandbox`). That run found Claude Code writing to `/tmp/claude-<uid>`, which Seatbelt refuses (now `CLAUDE_CODE_TMPDIR`), and waiting three seconds on an open stdin (now closed). The redaction test also ran there in Seatbelt, and `cww coding setup` ran side by side from nothing: folder, sign-in, mirror, checkout and status.
- **Windows 11 (26H2), the test VM.** The installer CI built, then `cww coding setup` from nothing over SSH. It installed Claude Code with Anthropic's installer and found it outside `PATH` in `~\.local\bin`, kept the token in the file store, and made the mirror and the checkout (`origin` is the mirror) and fetched it. Over SSH the Credential Manager isn't reachable (`ERROR_NO_SUCH_LOGON_SESSION`), and the setup says so. Commands can't run in a sandbox from an SSH session, so on Windows the run itself was tested in CI on Windows 11 on Arm, in a process container: the redaction test with a stand-in `claude.cmd` (the sign-in reaches only the allowed bot, with the token taken out of stdout, stderr and the output window). The real Claude Code from a Windows sandbox, which needs a desktop session, hasn't been tried.
- **Everywhere, in CI:** the unit tests, and on Linux the e2e test through the tunnel (`roots` says `ready`, the bot's run gets the sign-in redacted, a person's doesn't, and so does `coding_check`).

## Not built yet

- The desktop app's page for this ("Lend this computer to a coding bot" on a bot's computer pane), driving the same steps over the control API.
- Other coding agents' sign-ins (Codex, Gemini CLI). `[coding]` is shaped so each gets its own entry and its own `claude_code`-like switch on a loan.
- The sign-in for a person's own chats' commands: only bots' runs get it.

Other secrets a bot's commands need (a test suite's keys, a 1Password service account's token and the values `op read` gets with it) go in the folder's environment, `cww roots env` ([secrets.md](secrets.md)), which every command in the folder gets.
