# Lending this computer to a coding bot

A coding bot in Chat with Work (the Issue fixer, the Dependency upgrader on a computer, and bots like them) runs commands on a computer lent to it: it fetches the repository, has a coding agent (`claude -p`, `codex exec`, `opencode run`, `gemini -p` or `pi -p`) make a change in a fresh worktree, runs the tests, and then commits the changed files through Chat with Work's GitHub App. This page is how a person gets a computer ready for that in one guided step, on Linux, macOS and Windows, and what the computer then holds to.

Status: one yes in the app's own window and a first run that prepares itself ([below](#one-yes)), and the guided `cww coding setup` for servers. Claude Code, Codex, OpenCode, Gemini CLI and pi ([Which agents](#which-agents)).

**What the server can do here.** The person chooses on this computer which bot, which agent, and whose repositories (the [one Allow](#one-yes), or `cww coding`). The server can then name a repository of an owner they allowed, and a run prepares it asking nobody ([The first run](#the-first-run)), and a card's Always allow from the computer's owner makes a folder already lent to that bot unattended ([below](#letting-a-bots-commands-run-unattended-from-its-card)). It can't lend a folder, turn commands or the network on, or give a sign-in. It learns whether a lent folder is ready, and the names of the repositories a run named, never a token, a path or an address.

## What it took by hand

Setting up Plenty's NUC took six steps a customer couldn't do: share a folder with changes, commands and the bot; install Claude Code; run `claude setup-token` and put the token in a `.env` file the sandbox could read through a `claude` wrapper; work around the command `PATH`, which found mise's `claude` before the wrapper; clone the repository and keep it fresh with a timer and a deploy key; install Ruby and the gems.

Two of those were unsafe, and this design removes them:

- **The token sat in a file in the lent folder.** The deny list kept it from Chat with Work's tools, but not from the commands the bot runs there ([commands.md, What it doesn't protect against](commands.md#what-it-doesnt-protect-against)), any of which could print it. Desktop now uses Claude Code's own sign-in where Claude Code keeps it ([below](#claude-codes-sign-in)), never a copy in a shared folder.
- **The mirror was fetched, outside the sandbox, from inside the lent folder.** A command the bot runs can write that mirror's `.git/config` (`core.sshCommand`, `core.fsmonitor`, a credential helper), and the timer's next `git fetch` would run it as the person, outside every sandbox.

## One yes

A coding bot needs nothing typed on the computer. When it lacks what it needs, Chat with Work shows a card, **Set up on chalk**, which opens the bot's page in the app (a link only moves the window). The page asks through `computer.setUpCoding`, and the app shows a window of its own, a bundled page no remote page can see or answer (`app/src/coding_setup.rs`):

- **Let Issue fixer (Bot 34) code here?**, the bot named by the server to this device, which also says this person lent it this computer.
- What it checked: the agent installed and signed in; your Git login reaches the repository (`git ls-remote`, SSH then https, asking for nothing); and where it works, a new folder (`~/bots`, or `~/bots-2` where something of yours is in that one, or the folder already lent to it).
- What the repository's code reaches, where this computer has its copy already (a run prepared it here before): "It can reach PostgreSQL and Redis on this computer, and the internet to install gems (from docker-compose.yml and Gemfile)" ([What it reaches](#what-it-reaches)).
- One short line: its commands reach this computer's services, the internet, and your sign-in, and can read that sign-in; they can't reach your SSH keys or other projects; you approve each one. Another: the first run on each repository of that owner copies it with your Git login, then installs and tests it in the sandbox.
- **Allow**, only while all of that holds, otherwise **Check again**, and **Cancel**.

Allow makes the folder, then does what `cww coding setup` asks one by one (the control API's `coding_allow`, audited `root_bot_coding_allowed`): shares it with changes, commands, local services and the internet (the agents need the internet to reach their own service), adds each local-network address the repository's files name to the folder's `reach` (read again by the computer from its copy, never taken from the window), lends it to the bot, gives its runs that agent's sign-in, and records the repository's owner on the loan. A later repository of another owner brings the card back, and one more Allow. The page hears only `{ready: true}` or `{declined: true}`, and the server asks the computer again. The older dialog ("Let Issue fixer run Claude Code in bots?", `ask_claude_code`) stays for servers that use it.

A run then finds the program itself where no setup recorded one, or the recorded one is gone (the program mise installed rather than its shim, the `PATH` of whoever starts runs, the native installer's `~/.local/bin`, Homebrew, npm's global programs), never one in a shared folder a command could change. It only looks: finding it runs nothing outside the sandbox.

## The first run

The first run on each repository prepares it with no question: a bot's run names `owner/name` (`prepare_repository`, [PROTOCOL.md 7.2.1](../PROTOCOL.md)), and the repository comes from the bot's own run, never typed. Whoever starts commands (`src/coding/prepare.rs`; the supervisor on Linux and macOS, the daemon on Windows) reads the config from disk and refuses anything but a folder lent to that bot with commands and the network, the agent its loan allows, and an owner its loan names or a repository already recorded here. Then:

1. **Copies it.** The [mirror](#repositories), with your own Git credentials, at the address your login reaches (SSH, then https; never one the server gives), recorded in `config.toml` with `github = "owner/name"`, and the checkout from it, in the sandbox. A folder of yours with that name is never taken over: the checkout is then `owner-name`.
1. **Reads what its code reaches** ([below](#what-it-reaches)). If the folder doesn't let its commands reach that, it stops there, before installing or running anything, and answers `needs_setup` with `need: "network"`, the services and whether it needs the internet: the server's **Set up on chalk** card brings the person back to the one Allow, which now says them and gives them.
2. **Installs what it needs**, read from a private copy of the mirror, in the sandbox: mise's toolchain, then the dependency install its lockfile names ([detect.rs](../src/coding/detect.rs)).
3. **Runs its tests once**, in the sandbox.

Install and test code run as a person's command would, with **no agent's sign-in**, and with the network on: that's the repository's own code, which the person's Allow covered. A run's home goes with it, so what must outlast one lives in the folder, `.cww-tools/` (package caches, `BUNDLE_PATH`, `CARGO_HOME`, `GOPATH`, and mise's installs where mise can't write this computer's, beside links to what this computer has). It runs in the sandbox only, and nothing runs from it outside.

### What it reaches

`src/coding/needs.rs` reads the repository's files from its mirror with `git ls-tree` and `git show`, never by running its code, and finds:

- **Services**, each with the name a person reads, where the code connects (`host`, `port`), where that is (`this_computer`, `local_network` or `internet`, placed from this computer's own interfaces, `localhost` and Compose's service names being this computer's), and the file that says so (`from`):
  - `docker-compose*.yml` and `compose*.yaml` at the top: services whose image (or name) is PostgreSQL, MySQL or MariaDB, Redis or Valkey, MongoDB, Memcached, Elasticsearch or OpenSearch, or RabbitMQ, on `localhost` at the host port they're published on (`ports:`, short or long syntax), else on their own port by their service name.
  - `config/database.yml`: the adapter, host and port, read line by line (ERB's default value stands for one, and ERB without one means `localhost`; a socket path is left out).
  - `.env`, `.env.example`, `.env.sample`, `.env.development`, `.env.test`, `Procfile` and `Procfile.dev`: service URLs such as `DATABASE_URL=postgres://...` and `REDIS_URL=redis://...`, and `<NAME>_HOST` beside `<NAME>_PORT`.
  - `package.json`'s scripts: service URLs.
- **The internet for installs**: a `Gemfile` (gems), `package.json` with dependencies (npm packages), `Cargo.toml` with dependencies (crates), `requirements*.txt` or `pyproject.toml` (Python packages), `go.mod` with requirements (Go modules).

A host name that is none of those is looked up, and one that doesn't resolve is left out. A folder gives the code what it reaches when its commands have local services for this computer's services (on Windows, which can't give them, they're never asked for), the exact address in `reach` or the internet for the local network's, and the internet for an internet service or installs ([commands.md](commands.md)). The answer to `prepare_repository` carries `services` and `internet` when it's ready too.

The steps go back as they change (`prepare_status`), and a chat shows them live. A failed step ends it, and the bot's chat gets one card naming the step: Try again, and for the tests, Go on without them. Once done, a mirror's `cww-prepared.json` says so, and later runs answer at once. A repository set up with `cww coding setup` needs none of this. It also answers who pays, from the sign-in (`plan`, `api_key`, or `unknown`), for the run's card.

## The guided setup (for scripts)

```
cww coding setup
cww coding setup --bot "Issue fixer" --repo git@github.com:acme/app.git --folder ~/bots
cww coding setup --bot 42 --repo git@github.com:acme/app.git --ssh-key ~/.ssh/app-deploy
cww coding setup --bot 42 --agent codex
cww coding status [--bot 42] [--json]
cww coding allow <root> <bot> [--off] [--agent codex] [--owner acme]   # this bot's runs there use the agent's sign-in, or stop; and its first runs may prepare acme's repositories
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

**Where it lives.** Where Claude Code itself keeps it, for whoever runs `claude` on this computer: Desktop never stores, copies or asks for it. Claude Code keeps its account and preferences in `~/.claude.json` and everything else in `~/.claude`; its tokens are in `~/.claude/.credentials.json` (0600) on Linux and Windows, and in the login keychain (`Claude Code-credentials`) on macOS. Signed in with an Anthropic Console account instead, it keeps that key in `~/.claude.json` (`primaryApiKey`): that counts as signed in, billed by use (`api_key`), and the key is taken out of what a run prints like a token. Desktop looks only in those standard places, in the real home folder: never a folder `CLAUDE_CONFIG_DIR` or an account switcher points at. `config.toml` records only which program `claude` is, and per loan whether its runs use the sign-in:

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
| macOS (Seatbelt) | `~/.claude` and `~/.claude.json`, and the login keychain (MXC's `keychainAccess`: `securityd` and `~/Library/Keychains`) | the same links, and `Library/Keychains` in the run's home, which is where `security` looks; the tokens come from the keychain item, as outside the sandbox |
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

## Codex's sign-in

The same mapping for `~/.codex` as `~/.claude`, with its own switch on a loan (`codex_login = true`, `[coding] codex`), and one Allow or `cww coding setup --agent codex` / `cww coding allow <root> <bot> --agent codex` sets it. Every promise and limit above holds for it, `~/.codex/config.toml`, `AGENTS.md`, hooks and history included.

| | What the run reads and writes | How Codex finds it |
|---|---|---|
| Linux (Bubblewrap) | `~/.codex`, bound where it is | a link named `.codex` in the run's own home |
| macOS (Seatbelt) | Not yet: in Seatbelt Codex stops at "Failed to synchronize managed preferences", also with `com.apple.cfprefsd.*` looked up, and Desktop doesn't loosen the sandbox for it. The window says so, and a bot that needs Codex there gets the card | |
| Windows (process container) | `%USERPROFILE%\.codex` | `CODEX_HOME`, Codex's own variable |

**Tokens.** Every token in `auth.json` (`OPENAI_API_KEY`, `tokens.*` but its account's id) is taken out of what the run prints, as `[codex-token]`, as for Claude Code, with the same limits. A Codex set to keep its sign-in in the system keyring (`cli_auth_credentials_store`) reads as not signed in here: use its file store.

**The program.** `.cww-bin/codex` links to the program, and its helpers beside it (`codex-code-mode-host`, `codex-exec-server`) are read and linked too. A run uses `codex exec --sandbox danger-full-access`: its own sandbox can't start inside this one, which already holds it to the folder. That's Codex's flag for its own, not a way past Desktop's.

## OpenCode's sign-in

OpenCode 2 keeps its sign-ins and keys (a ChatGPT, Copilot or OpenCode Zen sign-in, or a provider's API key) as rows of its database, `~/.local/share/opencode/opencode.db`, beside its sessions. A loan's `opencode_login = true` (`--agent opencode`) lends that data folder, read and write, as `~/.codex` is lent; on Windows `OPENCODE_DB` names the database.

- **Never its config folder.** `~/.config/opencode` holds its plugins (npm packages it runs) and the password of its background service, which runs as you outside every sandbox: a run that knew it could have that service run anything. A run gets its own config folder, and only the model you chose there (the top-level `model` of `opencode.json` or `.jsonc`), as `OPENCODE_CONFIG_CONTENT`. Without one it uses OpenCode's own default, as `opencode run` would.
- **Its own server, never yours.** A run starts it with `opencode run --standalone --auto '<task>'`: a private server inside the sandbox, never the background service. A run that leaves `--standalone` out finds no service password in its own home, and its service ends with the run.
- **Tokens.** Desktop reads the credentials from the database and its write-ahead log, a regular file of at most 256 MiB each (a bigger one isn't lent, so no token goes unread), without opening them with SQLite: it puts every row back together from its pages, overflow pages included, as the file format says, and searches the files' bytes too, and takes every string of 20 characters or more in each out of what the run prints, as `[opencode-token]`. An old value a page still holds goes too.
- **What it can't promise**, as for Claude Code: a run can read and change your OpenCode sessions in that database, and its snapshots' Git folders there, which OpenCode later runs `git` on as you.
- **Not on macOS:** its server must listen on a local port, which Seatbelt doesn't give a run, and Desktop doesn't loosen the sandbox for it. The window says so.

## Gemini CLI's sign-in

`~/.gemini`, with a Google sign-in's tokens in `oauth_creds.json` (Google AI Pro or Ultra, Code Assist, or the free tier), or an API key in its `.env` (`GEMINI_API_KEY`), lent by `gemini_login = true` (`--agent gemini`). On Windows `GEMINI_CLI_HOME` names the folder that holds it. A run gets `GEMINI_SANDBOX=false`: its own sandbox can't start inside this one. Every string of 20 characters or more in `oauth_creds.json`, and the key in `.env`, leave what the run prints as `[gemini-token]`. A sign-in kept in the system keychain (`GEMINI_FORCE_ENCRYPTED_FILE_STORAGE`) reads as not signed in here. A run starts it with `gemini -p '<task>' --approval-mode yolo --skip-trust`. Its settings (`settings.json`: hooks, MCP servers) run outside the sandbox the next time you start it, as Claude Code's do.

## pi's sign-in

`~/.pi/agent`, whose `auth.json` holds a sign-in per provider (a ChatGPT or Copilot plan, among others) or an API key, lent by `pi_login = true` (`--agent pi`); on Windows `PI_CODING_AGENT_DIR` names it. Every string of 20 characters or more in `auth.json` leaves what the run prints as `[pi-token]`. A run gets `PI_SKIP_VERSION_CHECK=1` and starts it with `pi -p --no-session '<task>'`. The folder holds its extensions (TypeScript it runs), and a key in `auth.json` may be a shell command it runs to get one: what a run writes there runs outside the sandbox the next time you start pi.

An agent installed with npm whose program is a script (`gemini`, or pi from `npm install -g`) is read with its whole package folder, its dependencies included, never more.

## Which agents

Claude Code, Codex, OpenCode, Gemini CLI and pi: the most used command-line coding agents that run headless on a person's own subscription or key, with a sign-in in a file this computer can lend as it is. Left out, for now:

- **GitHub Copilot CLI** keeps its token in the system keychain, and in a file only after an interactive yes; Seatbelt that blocks `com.apple.SecurityServer` breaks its HTTPS. Lending it would mean passing a token in, which Desktop never does.
- **Cursor CLI** keeps its sign-in in the macOS keychain unless told otherwise, and we couldn't sign in to try it.
- **Amp** and **Factory Droid** keep theirs in the keyring (Droid's file is encrypted with a key there), so only an API key would work, not the person's plan.
- **Goose** uses the keyring unless told otherwise; **Aider** takes API keys only, and its last release was in August 2025; **Crush** runs shell from its config as it loads.
- **T3 Code** isn't an agent: it's an app that drives Claude Code, Codex, OpenCode and others, which a bot here uses directly.

## Who pays

A run on this computer uses the sign-in's own plan or key: Chat with Work charges nothing for it. Where an agent holds a plan for one provider and a key for another (OpenCode, pi), which pays depends on the model, so it says `unknown`. The answer to `prepare_repository` says which (`plan`, `api_key` or `unknown`), read from the sign-in (Claude Code's `claudeAiOauth.subscriptionType`, an OAuth sign-in or a key in the others' files), so the run's card says "Uses your Claude plan on chalk". Each provider's own terms and limits apply to a plan used this way.

**From 0.13.1 and earlier.** Those versions kept a token from `claude setup-token`, pasted into `cww coding sign-in`, in this computer's secret store. That's gone with `cww coding sign-in` and `sign-out`: the first `cww coding` command after the update deletes the stored token and says so (unpairing deletes it too). Revoke it at claude.ai (Settings, Claude Code) if you no longer use it. A loan's old `claude_code = true` doesn't carry over: that yes covered one token, and this gives a run Claude Code's files, so it's asked for again. Until then the bot's `roots` says `not_allowed`, and its request brings up the dialog; on a server with no app, run `cww coding allow <root> <bot>`. Sign in to Claude Code on that computer too if you only ever used a token there.

## Repositories

The mirror and its fetching stay out of the sandbox's reach; the checkout the bot works in has no credentials at all.

- **The mirror** is a bare clone in Desktop's data folder (`<data>/repositories/<root>/<name>.git`), which no command can write (setup refuses, and Desktop never fetches a mirror, where a shared folder that allows changes holds the data folder): the sandbox reads it, for runs in that folder only. Its config is Desktop's alone, so fetching it runs nothing a bot wrote. It fetches branches and tags (`+refs/heads/*:refs/heads/*`), not pull requests' refs.
- **Fetching** is Desktop's: `git fetch --prune` on the mirror, with your own Git credentials (your SSH keys and `~/.ssh/config`, never the system's `/etc/ssh` config, which ssh refuses inside the background service's user namespace; your credential helper), hooks off, run by whoever starts commands (the supervisor, or the daemon on Windows) before a command starts in that folder if the last fetch is over a minute old, with a 60-second limit, and by `cww coding fetch`. A fetch that fails doesn't stop the command; `cww coding status` shows the last failure. Nothing else runs `git` outside the sandbox, and never in a shared folder.
- **The checkout** is `git clone <mirror> <name>`, run in the sandbox in the folder: its `origin` is the mirror's path, so `git fetch origin` in a command works without a network, keys or tokens, and a push fails. `config.toml` records the repository's address, its checkout, its folder, and whether Desktop made the checkout ([Trusting its checkouts](#trusting-its-checkouts)):

```toml
[[coding.repositories]]
root = "bots"
name = "app"
url = "git@github.com:acme/app.git"
cloned = true
```

An address with a password or token in it (`https://user:token@...`) is refused: use an SSH key or a credential helper. For a deploy key (read-only, for this one repository), `--ssh-key ~/.ssh/deploy-key` fetches with only that key, never the SSH agent's or one your SSH config names (that config isn't read then); it must be outside every shared folder, and `config.toml` records only its path (`ssh_key`). Desktop writes nothing in the shared folder itself: the checkout is made by a command in the sandbox, as the bot (`git clone` from the mirror), which can change only that folder, so a link a command left there can't redirect it. A checkout already there is kept, and the same kind of command points its `origin` at the mirror and drops any push address. Changes reach GitHub only through Chat with Work's GitHub App, which commits the files the bot names from the computer and opens the pull request, each approved as the bot's policy says.

### Trusting its checkouts

Claude Code and Codex each keep whether a folder is trusted, and in one that isn't they still run (`claude -p`, `codex exec`) but leave out the repository's own settings: Claude Code its `.claude/settings.json` and `.claude/settings.local.json` permissions, Codex its `.codex/config.toml`. Claude Code also says so on every run ("Ignoring 19 permissions.allow entries from .claude/settings.local.json: this workspace has not been trusted"), and on 2026-11-09 an issue fixer read that as something to ask its team about. A git worktree counts as the checkout it was made from: Claude Code reads only the checkout's entry.

So before a bot's run that gets Claude Code or Codex, Desktop, as you, marks as trusted each checkout in that folder that it made itself: Claude Code's `projects["<checkout>"].hasTrustDialogAccepted` in `~/.claude.json` (on Windows the `.claude.json` in `~/.claude`, which `CLAUDE_CONFIG_DIR` names for a run), and Codex's `[projects."<checkout>"] trust_level = "trusted"`, added at the end of `~/.codex/config.toml`, which keeps your comments and layout. That covers every worktree the bot makes from it for an issue. It writes only what's missing, one run at a time, never an agent's settings that are inside a shared folder, and nothing else in either file changes. On Windows it makes that `.claude.json` when Claude Code hasn't yet. A level you set yourself (`trust_level = "untrusted"`) stays.

- **Made by Desktop** means cloned from the mirror, in the sandbox, as the bot, when nothing of that name was there: `config.toml` records it with the repository (`cloned = true`). For a repository set up before 0.16.1, which has no record, the checkout's first reflog entry says whether it was cloned from this mirror, read once, before the first run in that folder after the update, and recorded then, in `config.toml` and, for that checkout's path, in the mirror (`cww-checkout-settled`, where no command writes), so nothing a command writes there later changes it. A run whose folder changed meanwhile, or whose answer can't be recorded, doesn't start. Desktop reads it through no link inside the checkout, and an agent's settings only as a regular file that isn't a link.
- **A checkout that was already there**, which Desktop only pointed at its mirror (`cloned = false`), is yours: it keeps asking as before, and trusting it stays yours (run Claude Code there once and accept its dialog).
- **Trust doesn't widen the sandbox.** It decides which of the repository's own settings the agent reads; what a run may read, write and reach stays what the folder allows. It's in your own settings, though, so your own Claude Code or Codex in that checkout doesn't ask either.

## Letting a bot's commands run unattended from its card

A bot's command approval card in Chat with Work (for example "Start fixing #12 with Claude Code?") offers its computer's owner **Always allow on chalk**. Pressing it means it: Chat with Work calls `allow_unattended` for the folder (PROTOCOL.md 7.2.1), the loan is unattended at once, and that command runs. The daemon checks, in the same edit, that the folder is already lent to that bot with commands on, and changes nothing else: this can't share a folder, lend one, turn commands or the network on, or give a sign-in. It writes `root_bot_unattended_from_card` to the audit log and shows a notice, no question. Turn it off in Shared folders or with `cww roots share <folder> <bot>` at any time; after that only a new press sets it again, never anything by itself. It's the same loan as `--unattended`.

**What this widens.** A compromised server could flip a folder already lent with commands on to unattended. It could already run commands there, since this computer can't tell a real approval from a forged one; afterwards it could keep doing so without cards, with the notice and the audit entry here as what shows it.

`ask_unattended` stays for servers before this: the app asks in a dialog of its own, named from this computer's settings, with Allow and Not now, or a system notification and `cww roots requests` (`cww roots allow <root> <bot>`, `cww roots decline <root> <bot>`). A request lasts 24 hours, and needs a folder lent to the bot with commands allowed.

## What Chat with Work sees

In a bot's `roots` (PROTOCOL.md 7.2.1), each lent folder says whether Claude Code is ready for that bot there, and which repositories it holds, by name:

```json
{ "id": "bots", "label": "bots", "unattended": false,
  "coding": { "claude_code": "ready", "agents": { "claude_code": "ready", "codex": "not_allowed", "opencode": "not_allowed", "gemini": "not_allowed", "pi": "not_allowed" },
              "prepares": true, "owners": ["acme"], "repositories": ["app"], "named": { "acme/app": "app" } } }
```

`claude_code` is `ready`; `not_allowed` (this bot's runs here don't use the sign-in: the server asks, above); or `no_network` (this folder's commands have no network, which Claude Code needs). It's what `config.toml` says, checked again when a run starts; whether Claude Code is installed and signed in isn't in it (the worker that answers can't look in the home folder), so `not_installed` and `not_signed_in` come only from `cww coding status`, and a run hears them from the shell or Claude Code itself. No path, address, version or token.

`agents` says it per agent; `claude_code` stays for servers that know only that one. `prepares` says the computer prepares what a run names, and `named` names only repositories a run named, which the server knows already. The server keeps it with the folders it already remembers for the bot, and the bot's page says what the computer does ("chalk · Claude Code ready · acme/app · asks before each command") or "Needs Claude Code on chalk" with Set up, and `cww coding setup --bot 42` for SSH and an older Desktop. The server asks the computer's `roots` for the bot when the page shows a computer it hasn't heard from in a while, as it does for a bot's own runs. The recipes that run an agent assume this: it's found and signed in, `git fetch origin` works without credentials, and when it isn't ready, the bot gets the card from the app, rather than trying.

## Security

The rules in [SECURITY.md](../SECURITY.md) and [commands.md](commands.md) all still hold; this adds:

- **This computer decides, with two named exceptions.** The `[coding]` programs, each agent's `<agent>_login` on a loan (`claude_login`, `codex_login`, `opencode_login`, `gemini_login`, `pi_login`), and `repository_owners` are changed only by `cww coding`, `cww roots`, the control API (the app's window), or editing `config.toml`, never by the server. The server can name a repository of an owner the person allowed, which a run records and prepares ([The first run](#the-first-run)), and a card's Always allow makes a loan unattended ([above](#letting-a-bots-commands-run-unattended-from-its-card)). It can ask for Claude Code (`ask_claude_code`), and the person answers here. Turning a folder's commands off, or making it read-only, ends its loans' `claude_code` as it ends `unattended`.
- **The sign-in never reaches the worker, the server or the audit log.** Only a run whose loan allows it reads Claude Code's own paths, and the tokens Desktop can read there are removed from the run's output. It's never copied: Desktop stores no token of its own.
- **Desktop runs `git` outside the sandbox only on its own mirror**, with fixed arguments, hooks off, and a config no command can write, plus `git ls-remote` on an address it built itself and a private copy of the mirror to read what it needs. It never runs anything outside the sandbox in a shared folder. The first run's install and test code runs in the sandbox. The setup's toolchain step is the one exception, run by the person, in their terminal, after they agreed, before any bot has touched the checkout.
- **What a compromised server could do** in a folder lent with an agent's sign-in and commands: what it can already do there (run commands, as approvals or the unattended loan allow), plus use the person's Claude Code subscription, read their Claude Code history, with the network try to leak its tokens, and change Claude Code's settings so the person's own next `claude` runs something outside the sandbox ([What it can't promise](#claude-codes-sign-in)). Beyond that it can have a run copy, install and test any repository of an owner the person allowed, with the network on, and make a lent folder's commands unattended. It can't learn a token from `roots`, reach another folder or another owner's repositories, push to the repository, lend a folder, or give a sign-in.

## Testing

**The one yes and the first run, 2026-11-06, on chalk (Bubblewrap), with a side-by-side daemon, a test home holding copies of the sign-ins, and synthetic repositories through Git's `insteadOf`:** `coding_look` and `coding_allow` over the control socket; preparing with real steps, a failing test's and an unreachable repository's card, another owner refused; mise's fallback installing `jq` into `.cww-tools` from the sandbox; a real model's run driving a real Claude Code and a real Codex to edit a checkout; a card's Always allow turning a loan unattended and approving its command, with the audit entry. See the server's pull request notes for what wasn't tried.

Tried for real on 2026-11-03, with the setup token 0.13.1 and earlier used, and the computer's own sign-in on 2026-11-05:

- **Linux (Arch, Bubblewrap), chalk, with the computer's own sign-in.** A side-by-side daemon built from this change, with the `solco` folder lent to bot 24 and `claude_code` on, started `claude -p 'Reply with the word ready.' --max-turns 1` as the bot through `coding_check`: Claude Code (mise's `claude`) answered "ready" with the account in `~/.claude.json`, and the run saw its own home with links to `~/.claude` and `~/.claude.json`, and not `~/.ssh` or `~/Work`. A person's run, and another bot's, in the same folder saw no `~/.claude`.
- **macOS 27 (Seatbelt), the Mac mini, with the computer's own sign-in.** The sandbox tests passed in Seatbelt (`CWW_TEST_SEATBELT=1`): the bot's run reads and writes `~/.claude` and `~/.claude.json` and nothing else of the home folder, other runs see none of it, and the tokens are taken out of what it prints. The real Claude Code, from an SSH session, said "Not logged in": an SSH session can't unlock the login keychain, outside the sandbox too (`security` answers 36, interaction not allowed). From the desktop session, where the background service runs, it hasn't been tried yet.

- **Linux (Fedora 43, Bubblewrap), Plenty's NUC.** Moved from the hand-made setup: `cww coding sign-in --stdin` from the old `.env` file, then `cww coding setup --bot 24 --repo ... --ssh-key ...`, which kept the existing checkout and pointed its `origin` at the new mirror. Through the daemon, as bot 24, `bin/rails test` passed in the sandbox with the network, and `claude -p` answered from it, with `claude` on mise's `installs/claude/latest` link. The daemon fetched the stale mirror itself before the run, with the deploy key. Trying it found two bugs, fixed before this was called built. Bubblewrap can't start with a link given as a folder to bind (the program's folder is now bound where it really is), and ssh refused the system's `/etc/ssh` config inside the service's user namespace (Desktop's fetches now use `~/.ssh/config`).
- **macOS 27 (Seatbelt), the Mac mini.** The real Claude Code answered from the sandbox with a setup token (`real_claude_code_answers_from_the_sandbox`). That run found Claude Code writing to `/tmp/claude-<uid>`, which Seatbelt refuses (now `CLAUDE_CODE_TMPDIR`), and waiting three seconds on an open stdin (now closed). The redaction test also ran there in Seatbelt, and `cww coding setup` ran side by side from nothing: folder, sign-in, mirror, checkout and status.
- **Windows 11 (26H2), the test VM.** The installer CI built, then `cww coding setup` from nothing over SSH. It installed Claude Code with Anthropic's installer and found it outside `PATH` in `~\.local\bin`, kept the token in the file store, and made the mirror and the checkout (`origin` is the mirror) and fetched it. Over SSH the Credential Manager isn't reachable (`ERROR_NO_SUCH_LOGON_SESSION`), and the setup says so. Commands can't run in a sandbox from an SSH session, so on Windows the run itself was tested in CI on Windows 11 on Arm, in a process container: the redaction test with a stand-in `claude.cmd` (the sign-in reaches only the allowed bot, with the token taken out of stdout, stderr and the output window). The real Claude Code from a Windows sandbox, which needs a desktop session, hasn't been tried.
- **OpenCode, Gemini CLI and pi, 2026-11-07, on chalk (Bubblewrap) and the Mac mini (macOS 27, Seatbelt), with test homes holding API keys, never a person's sign-in:** through `real_agent_answers_from_the_sandbox` (`CWW_TEST_AGENT`, `CWW_TEST_PERSON_HOME`), each real agent ran a shell tool in the checkout and answered from the sandbox, the run's home holding only links to its own folder; `cat` of each one's sign-in printed `[opencode-token]`, `[gemini-token]` and `[pi-token]`. OpenCode 2.2.6, Gemini CLI 0.67.0 and pi 1.5.0 on Linux; Gemini CLI and pi on macOS, where OpenCode's private server couldn't listen (above). A plan's sign-in (ChatGPT, Google) has the same files but wasn't tried. Windows wasn't tried for these three.
- **Everywhere, in CI:** the unit tests, and on Linux the e2e test through the tunnel (`roots` says `ready`, the bot's run gets the sign-in redacted, a person's doesn't, and so does `coding_check`).

## Not built yet

- The agents [left out](#which-agents), and OpenCode and Codex on macOS.
- The sign-in for a person's own chats' commands: only bots' runs get it.
- Toolchains on Windows: mise's fallback into `.cww-tools` is Linux and macOS only; on Windows `mise install` must be able to write, or that step's card says so.

Other secrets a bot's commands need (a test suite's keys, a 1Password service account's token and the values `op read` gets with it) go in the folder's environment, `cww roots env` ([secrets.md](secrets.md)), which every command in the folder gets.
