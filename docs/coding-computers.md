# Lending this computer to a coding bot

A coding bot in Chat with Work (the Issue fixer with Claude Code, the Dependency upgrader on a computer, and bots like them) runs commands on a computer lent to it: it fetches the repository, has Claude Code (`claude -p`) make a change in a fresh worktree, runs the tests, and then commits the changed files through Chat with Work's GitHub App. This page is how a person gets a computer ready for that in one guided step, on Linux, macOS and Windows, and what the computer then holds to.

Status: designed 2026-11-03. Built in `cww coding` ([below](#the-guided-setup)); the desktop app's page for it is not built yet ([Not built yet](#not-built-yet)).

**Nothing here widens what the server can do.** Every choice is made on this computer: which folder, which bot, whether its commands run unattended, whether it gets your Claude Code sign-in. The server learns only whether a lent folder is ready, never a token, a path or a repository's address.

## What it took by hand

Setting up Plenty's NUC took six steps a customer couldn't do: share a folder with changes, commands and the bot; install Claude Code; run `claude setup-token` and put the token in a `.env` file the sandbox could read through a `claude` wrapper; work around the command `PATH`, which found mise's `claude` before the wrapper; clone the repository and keep it fresh with a timer and a deploy key; install Ruby and the gems.

Two of those were unsafe, and this design removes them:

- **The token sat in a file in the lent folder.** The deny list kept it from Chat with Work's tools, but not from the commands the bot runs there ([commands.md, What it doesn't protect against](commands.md#what-it-doesnt-protect-against)), any of which could print it.
- **The mirror was fetched, outside the sandbox, from inside the lent folder.** A command the bot runs can write that mirror's `.git/config` (`core.sshCommand`, `core.fsmonitor`, a credential helper), and the timer's next `git fetch` would run it as the person, outside every sandbox.

## The guided setup

```
cww coding setup
cww coding setup --bot 42 --repo git@github.com:acme/app.git --folder ~/bots
cww coding setup --bot 42 --repo git@github.com:acme/app.git --ssh-key ~/.ssh/app-deploy
cww coding status [--bot 42] [--json]
cww coding sign-in       # store a token from `claude setup-token`
cww coding sign-out      # forget it
cww coding fetch         # fetch every repository's mirror now
```

`setup` asks one thing at a time, says what it's about to do before it does it, and prints each step's state as it goes (`ok`, `needs you`, `skipped`, `failed: why`). Run again, it skips what's done. Every answer can be given as a flag, so the desktop app (and a person over SSH) can drive the same steps.

1. **Paired.** The computer is paired and the daemon runs, or it says to run `cww login`.
2. **Bot.** Which bot: the number in its address in Chat with Work (`/bots/42`), and its name, shown only here. With `cww bots` allowed it lists the organization's bots that have a computer and lets you pick.
3. **Folder.** Pick a shared folder or make one (default `~/bots`, `%USERPROFILE%\bots` on Windows). It shares it, allows changes, commands and the network (Claude Code needs the network), and lends it to the bot, each after saying what it allows, as `cww roots allow-commands` does. It asks separately whether the bot's commands there may start **unattended**; the default is no, so each asks you in Chat with Work.
4. **Claude Code.** It looks for `claude` where you'd run it (your `PATH`, then the native installer's `~/.local/bin`, Homebrew, npm's global programs) and runs `claude --version`. Missing, it offers Anthropic's own installer (`curl -fsSL https://claude.ai/install.sh | bash`, or `irm https://claude.ai/install.ps1 | iex` on Windows) after showing the command, or prints it to run yourself. It remembers the program it found by its full path.
5. **Sign-in.** It runs `claude setup-token` in your terminal, which signs in with your own Claude subscription in the browser and prints a long-lived token, then asks you to paste it, without echoing it. The token goes in this computer's secret store (below), never in a file in a shared folder, never on the screen again. It then asks whether this bot may use it in this folder.
6. **Repository.** It asks for the repository's address and clones it the way [Repositories](#repositories) says: a mirror only Desktop writes, fetched with your own Git credentials, and a checkout in the folder that fetches from the mirror.
7. **Toolchain.** It reads the checkout for what it needs (`mise.toml` or `.tool-versions`, `Gemfile`, `package.json` and its lockfile, `Cargo.toml`, `pyproject.toml` with `uv.lock` or `requirements.txt`, `go.mod`) and lists what's missing on this computer. It offers `mise install` where the repository uses mise (and mise is here), and the dependency install the lockfile names (`bundle install`, `npm ci`, `pnpm install --frozen-lockfile`, `yarn install --immutable`, `uv sync`, `cargo fetch`). These run as you, outside the sandbox, and run the repository's own install scripts: it says so and asks first. Otherwise it says what to install.
8. **Proof.** It runs the repository's test command once, in the sandbox a bot's command gets, as that bot (`bin/rails test`, `npm test`, `cargo test`, `pytest`, `go test ./...`, or `mise run test`; you can change it), then `claude -p 'Reply with the word ready.' --max-turns 1` there with the bot's sign-in, which proves the right `claude` is found, signed in, and reaches Anthropic. Both print how they ended and the end of their output.

At the end it prints what Chat with Work will see: "NUC: Claude Code ready for Issue fixer in bots (app)".

## Claude Code's sign-in

**Where it lives.** The token is a secret of this computer, kept like the device key ([src/auth/secrets.rs](../src/auth/secrets.rs)): the Keychain on macOS, the Credential Manager on Windows, the Secret Service (libsecret) on Linux, and, where none works (a headless server), the 0600 secrets file in Desktop's config folder, which no command's sandbox reads. `config.toml` records only that one is stored and which program `claude` is:

```toml
[coding]
claude = "/home/carmine/.local/bin/claude"
claude_signed_in = true

[[roots]]
id = "bots"
# ...
[[roots.bots]]
id = "42"
name = "Issue fixer"
claude_code = true     # this bot's commands here get the sign-in
```

**Who gets it.** Only a command a bot runs, in a folder lent to that bot, whose loan says `claude_code = true`. Never a person's own chats' commands, another bot's, or a folder not lent. Whoever starts the command (the supervisor on Linux and macOS, the daemon on Windows) reads the token from the store for that one run and passes it as `CLAUDE_CODE_OAUTH_TOKEN`, Claude Code's own variable for these tokens, with `DISABLE_AUTOUPDATER=1`. The confined worker never holds it. The worker's request to the supervisor carries the bot's ID; the supervisor checks the loan in `config.toml` itself.

**The right `claude`.** For such a run the sandbox reads the folder holding the program setup recorded (for the native installer, `~/.local/share/claude/versions`, where `~/.local/bin/claude` leads), and `PATH` starts with a folder of the run's own holding only a link named `claude` to it (on Windows, the folder holding `claude.exe` goes first). mise's shims and everything else stay after it, so a `claude` that mise or npm also installed doesn't shadow it, and no wrapper is needed.

**What it can't promise.** A command that's given the token can use it, and a command can be any line of shell:

- The daemon removes the token's exact text from what a run prints before it leaves the computer (`run`, `run_status`, `run_output`, this computer's own windows), replacing it with `[claude-code-token]`, across chunk boundaries. That stops it being read back by accident or by a plain `env`; it doesn't stop a command that encodes it first, writes it to a file the bot then reads, or, where the folder allows the network, sends it somewhere.
- Claude Code passes its environment to the commands it runs itself.

So the token is as safe as the commands you approve for that bot. Approving each one (the default) means you see every command line before it runs; an unattended loan means a compromised server, or a bot misled by what it read, could run one that leaks it. If that happens, revoke the token at claude.ai (Settings, Claude Code) and run `cww coding sign-in` again. `cww coding sign-out` deletes it from the store at once; the next run gets nothing.

## Repositories

The mirror and its fetching stay out of the sandbox's reach; the checkout the bot works in has no credentials at all.

- **The mirror** is a bare clone in Desktop's data folder (`<data>/repositories/<root>/<name>.git`), which no command can write: the sandbox reads it, for runs in that folder only. Its config is Desktop's alone, so fetching it runs nothing a bot wrote. It fetches branches and tags (`+refs/heads/*:refs/heads/*`), not pull requests' refs.
- **Fetching** is Desktop's: `git fetch --prune` on the mirror, with your own Git credentials (your SSH keys and `~/.ssh/config`, never the system's `/etc/ssh` config, which ssh refuses inside the background service's user namespace; your credential helper), hooks off, run by whoever starts commands (the supervisor, or the daemon on Windows) before a command starts in that folder if the last fetch is over a minute old, with a 60-second limit, and by `cww coding fetch`. A fetch that fails doesn't stop the command; `cww coding status` shows the last failure. Nothing else runs `git` outside the sandbox, and never in a shared folder.
- **The checkout** is `git clone <mirror> <folder>/<name>`: its `origin` is the mirror's path, so `git fetch origin` in a command works without a network, keys or tokens, and a push fails. `config.toml` records the repository's address, its checkout and its folder:

```toml
[[coding.repositories]]
root = "bots"
name = "app"
url = "git@github.com:acme/app.git"
```

An address with a password or token in it (`https://user:token@...`) is refused: use an SSH key or a credential helper. For a deploy key (read-only, for this one repository), `--ssh-key ~/.ssh/deploy-key` fetches with only that key, never the SSH agent's; it must be outside every shared folder, and `config.toml` records only its path (`ssh_key`). A checkout already in the folder is kept, and its `origin` pointed at the mirror with `git config --file`, which runs nothing the checkout's config says. Changes reach GitHub only through Chat with Work's GitHub App, which commits the files the bot names from the computer and opens the pull request, each approved as the bot's policy says.

## What Chat with Work sees

In a bot's `roots` (PROTOCOL.md 7.2.1), each lent folder says whether Claude Code is ready for that bot there, and which repositories it holds, by name:

```json
{ "id": "bots", "label": "bots", "unattended": false,
  "coding": { "claude_code": "ready", "repositories": ["app"] } }
```

`claude_code` is `ready`; `not_installed` (setup found no `claude`); `not_signed_in` (no token stored); `not_allowed` (stored, but not for this bot here); or `no_network` (this folder's commands have no network, which Claude Code needs). It's what `config.toml` says, checked again when a run starts. No path, address, version or token.

The server keeps it with the folders it already remembers for the bot, and the bot's page says what the computer still needs, with the command that fixes it: "Claude Code isn't signed in on NUC for this bot. On NUC, run `cww coding setup --bot 42`." The server asks the computer's `roots` for the bot when the page shows a computer it hasn't heard from in a while, as it does for a bot's own runs. The recipes that run Claude Code assume this setup: `claude` is found and signed in, `git fetch origin` works without credentials, and when `roots` says Claude Code isn't ready, the bot stops and tells its team what to run, rather than trying.

## Security

The rules in [SECURITY.md](../SECURITY.md) and [commands.md](commands.md) all still hold; this adds:

- **Only this computer decides.** The `[coding]` table, `claude_code` on a loan, and the repositories are changed only by `cww coding`, `cww roots`, the control API, or editing `config.toml`, never by the server. Turning a folder's commands off, or making it read-only, ends its loans' `claude_code` as it ends `unattended`.
- **The token never reaches the worker, the server, the audit log, or a file a command can read.** It's read for each run by the process that starts it, and removed from the run's output.
- **Desktop runs `git` outside the sandbox only on its own mirror**, with fixed arguments, hooks off, and a config no command can write. It never runs anything outside the sandbox in a shared folder. The setup's toolchain step is the one exception, run by the person, in their terminal, after they agreed, before any bot has touched the checkout.
- **What a compromised server could do** in a folder lent with `claude_code` and commands: what it can already do there (run commands, as approvals or the unattended loan allow), plus use the person's Claude Code subscription, and with the network, try to leak its token. It can't learn the token from `roots`, reach another folder, push to the repository, or change any of these settings.

## Testing

Each system is tried for real before this is called built on it: setup from nothing, a bot's `claude -p` run through the tunnel, the token absent from output, from the audit log and from the run's home, a push from the checkout refused, and the mirror fetched with the person's own credentials.

## Not built yet

- The desktop app's page for this ("Lend this computer to a coding bot" on a bot's computer pane), driving the same steps over the control API.
- Other coding agents' sign-ins (Codex, Gemini CLI). `[coding]` is shaped so each gets its own entry and its own `claude_code`-like switch on a loan.
- A token for a person's own chats' commands: only bots get one.
