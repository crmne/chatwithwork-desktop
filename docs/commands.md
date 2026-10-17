# Running commands

Status: **built on Linux, untested on macOS, off on Windows.** In a shared folder that allows changes, you can also allow commands. Chat with Work can then run one line of shell there, like `bin/rails test` or `npm run build`, each after you approve it. The wire details are in [PROTOCOL.md, section 8.15](../PROTOCOL.md#815-commands), and the switch in [CONTROL.md, `roots_commands`](../CONTROL.md#roots_commands).

## Where a command runs

The daemon itself can't run anything in a shared folder: its sandbox never gives a shared folder the right to execute (`sandbox.rs`), and on Linux Landlock also forbids the mounts a sandbox for a command needs. So the confined worker asks the process above it, the supervisor, which isn't confined, to start the command in a sandbox of its own, made for that one run with [MXC](https://github.com/crmne/microsoft-mxc) (Bubblewrap on Linux, Seatbelt on macOS):

- It **reads** the system (`/usr`, `/bin`, `/etc` and the rest of what Bubblewrap binds by default) and the language toolchains that exist here: mise (`~/.local/share/mise`, its config in `~/.config/mise`), rbenv, pyenv, rustup, `~/.cargo/bin` (never the rest of Cargo's home, which holds registry credentials), Volta, Bun, Deno, `~/.local/bin`, Nix, Homebrew, `/usr/local` and `/opt`. Never the rest of your home folder: no `~/.ssh`, no browser profiles, no other projects.
- It **writes** only its folder, and a home and temporary folder of its own (`<data>/runs/<id>`, removed when it ends).
- It has **no network**, unless the folder allows it ("Commands use the network"). Without it, the system resolver's socket is hidden too.
- It gets a **clean environment**: `PATH` from the toolchains above and the system, `HOME` and `TMPDIR` in its own folder, `LANG=C.UTF-8`, `TERM=dumb`, and what the toolchains need to find themselves (`MISE_DATA_DIR`, `RUSTUP_HOME`, ...). Never the daemon's variables or tokens. mise trusts the folder's own `mise.toml`.
- It has **limits**: CPU time and memory for each process it starts (600 CPU seconds and 4 GB by default), a wall clock (30 minutes), low priority, and at most 4 commands at once. They're `command_cpu_seconds`, `command_memory_mb`, `command_timeout_seconds` and `commands_running` under `[limits]` in `config.toml`.

The worker names the folder by its ID and the working folder by a path relative to it, never an absolute path. The supervisor reads `config.toml` again for every command, checks that the folder allows commands, and resolves the working folder itself, refusing one that leads outside. A folder that stops allowing commands stops the commands running in it; a worker that ends (to restart under new rules, or because the daemon stops) stops all of its own, and the supervisor waits for them before it goes on.

They talk over a socket pair the worker inherits as a file descriptor (`CWW_RUN_CHANNEL`), one JSON line each way per call (`commands::channel`). A daemon that runs unconfined (the sandbox off, or the tests) starts commands itself, the same way.

## What Chat with Work sees

`roots` says which folders allow commands (`allows_commands`) and whether they may use the network. While one does, four more tools are offered:

- `run` starts a command and waits up to 15 seconds for it (Chat with Work waits 20 for a tool call), reporting what it prints as MCP progress notifications (bytes so far, and its latest line). It answers with the exit code and the end of each stream (20 000 characters by default); a longer command keeps running and the answer says so. `background: true` answers at once, for a server or a watcher. `dry_run: true` says where and how it would run, with the folder's network setting and the limits, for the approval card, and runs nothing.
- `run_status` and `run_output` follow a command left running: how it stands, and what it printed from an offset (the latest megabyte is kept).
- `run_stop` stops one, and everything it started.

`run` is annotated destructive and open-world, so Chat with Work asks every time; `run_stop` is a plain write; the other two only read. Each call is in the activity log with how it ended and the command line ("exit 0 · bin/rails test"). That log stays on this computer.

## What it doesn't protect against

- **A command can change or delete anything in its folder**, and nothing in the trash brings that back: approve each one knowing that. Version control is the undo.
- **A command reads everything in its folder**, the deny list included (`.env`, keys): the deny list keeps files from Chat with Work's tools, not from programs run in the folder.
- **Toolchain settings are readable**: mise's config can hold environment variables.
- **No limit on the number of processes on Linux and macOS.** MXC enforces it only on Windows; CPU and memory are limited per process, so a fork bomb is held back by the time limit and the pid namespace, not by a count.
- **The network, when allowed, is the whole network.** Allowing it per host isn't built. On Linux it needs `slirp4netns` and the other tools MXC lists; without them, a command in a folder that allows the network fails to start and says why.
- **The systemd unit's own limits apply to commands.** The supervisor runs under `cww daemon install`'s unit, whose `MemoryDenyWriteExecute`, system call filter and `ProtectSystem=strict` carry over to everything it starts: JIT runtimes (Node, the JVM) may fail there until the unit is relaxed for them.

## Not built yet

- macOS: Seatbelt runs come from MXC, but haven't been tried on a Mac here.
- Windows: off, until it has a sandbox for the daemon.
- A list of background commands under the composer, and live output in the step, need the server to send them in the chat API.
