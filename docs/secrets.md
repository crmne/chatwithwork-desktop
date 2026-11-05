# A folder's environment

A shared folder that allows commands can give every command run in it a few environment variables: an API key for a test suite, a database password, a 1Password service account's token. The person sets them on this computer, as `NAME=value` lines; the computer keeps them in its secret store, works out each value when a run starts, and takes the values out of everything the run prints. Chat with Work never sees them.

Status: built in `cww roots env` and tested on Linux (Bubblewrap); the desktop app's field for it is not built yet ([Not built yet](#not-built-yet)).

## The list

```
API_KEY=your-key
DB_PASSWORD=$(op read op://Vault/Item/password)
```

One variable a line, read as dotenv reads them:

- `NAME=value`. Blank lines and lines starting with `#` are skipped, and `export ` before a name is allowed. A name is letters, digits and `_`, not starting with a digit.
- A bare value runs to the end of the line, without the spaces around it or a ` #` comment after it.
- `'single quotes'` keep exactly what's in them, over several lines too, `$(` included.
- `"double quotes"` can span lines, and read `\n`, `\t`, `\"`, `\\` and `\$`.
- `$(command)` in a bare or double-quoted value runs the command when a run starts, and puts what it printed there, less the newlines at its end (`"pw-$(cmd)-x"` works too). Parentheses nest, and quotes inside it hold what they quote.
- `$NAME` and `${NAME}` are kept as text: there's no expansion of other variables. A `$( )` sees the variables above it in its environment, so `$(op read ...)` finds an `OP_SERVICE_ACCOUNT_TOKEN` set on an earlier line.

Each mistake is named by its line, and a list with any is refused whole:

```
line 2: no = (write NAME=value)
line 3: "MY KEY": a name has no spaces
line 4: $( isn't closed
line 5: the " isn't closed
```

A name the sandbox sets or reads itself is refused, compared without case, as Windows compares them: `PATH`, `HOME`, `TMPDIR`, `TEMP`, `TMP`, `LANG`, `TERM`, `SHELL`, `USER`, `PWD`, `IFS`, `ENV`, `BASH_ENV`, the Windows profile and system folders (`USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `SYSTEMROOT`, `COMSPEC`, `PATHEXT`, ...), the toolchains' own (`RUSTUP_HOME`, `CARGO_HOME`, `GOPATH`, ...), and anything starting `LD_`, `DYLD_`, `XDG_`, `MISE_`, `LC_`, `CWW_` or `CLAUDE_CODE_` ([src/commands/env.rs](../src/commands/env.rs)). Should a toolchain add one the list doesn't name, the run's own value still wins: the folder's values are added last, and never over a variable the run already has.

## Setting it

Only on this computer, never from the server:

```
cww roots env app --stdin < app.env   # replace app's list
cww roots env app                     # the names, never the values
cww roots env app --clear             # forget it
```

`--stdin` reads the whole list, refuses it with every mistake by line, or keeps it and prints the names. An empty list (or only comments) forgets it. A folder that doesn't allow commands yet can have one; it's used once commands are allowed.

## Where it lives

The list, values and commands alike, is one entry of this computer's secret store ([src/auth/secrets.rs](../src/auth/secrets.rs)), named for this install and the folder (`folder-env:<install>:<id>`, the install being a hash of its config folder, since every install on the account shares the keychain), whether or not the computer is paired: the Keychain on macOS, the Credential Manager on Windows, the Secret Service on Linux, or the 0600 secrets file in Desktop's config folder where none works, which no command's sandbox reads. Never `config.toml`, which notes only which folders have one (`folder_env = ["app"]`), so a run in a folder without one never reads the store, and a run in one whose list has gone from the store stops rather than run without it. Never the audit log, the server or a file in a shared folder. Windows' Credential Manager holds at most 2,560 bytes an entry, so a longer list is refused there with the store's error; elsewhere a list is at most 16 KiB. Removing the folder (`cww roots remove`, or the app) forgets its list (a keychain that refuses is logged, never keeps the folder shared), and so does `cww purge`, which deletes only this install's entries. Setting a list saves `config.toml` after the store and puts the old list back if that fails; clearing one saves `config.toml` first.

## A run

1. **Read** by whoever starts the run (the supervisor on Linux and macOS, the daemon on Windows) from the store, never by the confined worker. A list that can't be read, or that reads with a mistake, stops the run with that said, rather than running without it.
2. **Worked out** in order, before the command starts. Text is used as it is. Each `$( )` runs as a command of its own **in the run's own sandbox**, with the folder's allowances: the same folder, home, network (none unless the folder allows it), limits and clock, in `sh` on Linux and macOS and PowerShell on Windows, with the variables above it in its environment. What it prints on its output is the value (64 KiB at most, text without NUL); it never reaches the run's output. It reads nothing on its input.
3. **Given** to the command as variables, after everything the run sets itself, and never on a command line, where any user of this computer could read them (`ps`). Bubblewrap takes a run's environment as its own arguments (`--setenv NAME VALUE`), so there the values go in a file in the run's own home instead (0600, in Desktop's private data folder), which the run's shell reads and deletes before the command (and each `$( )`) starts. Landlock, Seatbelt and process containers give the environment to the process itself, which only this user can read.
4. **Taken out** of what the run prints before it leaves the computer (`run`, `run_status`, `run_output`, this computer's own windows): each value's exact text becomes `[redacted]` (never its name, which Chat with Work isn't told either), across chunk boundaries, longest first, so one value that begins another never leaves the longer one's end behind.

A `$( )` that exits with anything but 0, prints too much, or prints something that isn't text stops the run before the command starts: `DB_PASSWORD's $( ) exited with 1`, with the end of what it printed on its error stream in the run's error stream, the values known by then taken out. A stop or the run's clock ends it as it ends a run.

## Who gets it

Every command in the folder: a person's own chats' and those of the bots it's lent to, approved or unattended as each loan says. A folder is the unit Desktop shares and sandboxes, and a variable a test suite needs is needed whoever runs the tests. Lend a separate folder to a bot that shouldn't have a folder's variables. Claude Code's sign-in stays per bot ([coding-computers.md](coding-computers.md)).

## Security

**`$( )` runs in the sandbox, not as you.** A list can come from a file someone else wrote, and the commands it names run on every run, unattended runs included: running them outside the sandbox would give whatever wrote the list, or whatever replaced the program it names in a folder a bot can change, your whole account. In the sandbox a `$( )` can do no more than the folder's commands already do. So a password manager's command works only with what the sandbox gives it: its program in a folder every run reads (`/usr/bin/op`, a toolchain), the network where the folder allows it, and a token in an earlier line (a 1Password service account's `OP_SERVICE_ACCOUNT_TOKEN`, read-only on a vault of its own), never your signed-in desktop app.

**What it can't promise**, as for Claude Code's token:

- A command that's given a value can use it, and a command can be any line of shell. Taking values out of the output stops them being read back by accident or by a plain `env`; it doesn't stop a command that encodes one first, writes it to a file a bot then reads, or, where the folder allows the network, sends it somewhere. Every variable here is as safe as the commands that run in the folder: approving each (the default) means seeing every command line first; an unattended loan means a compromised server, or a bot misled by what it read, could run one that leaks them.
- A value shorter than a few characters, or one that's an everyday word, is taken out wherever it appears, which can garble output. Use real secrets.
- A token given to `$( )` is in the run's environment too. Prefer one that can only read what the folder needs (a service account on a vault of its own), and rotate it if the folder's commands may have leaked it.

**What Chat with Work sees**: nothing. Not the names, the values or the commands; `roots` doesn't say a folder has a list. Its approval cards show the command line as asked, never a value: a value is added on this computer after approval.

**What a compromised server could do** in a folder with a list and commands: what it can already do there (run commands, as approvals or unattended loans allow), and through them use or, with the network, try to leak the values. It can't read or change the list, add a `$( )`, or learn a value from anything Desktop sends it.

## Testing

Unit tests read lists as dotenv does and name each mistake by its line ([src/commands/env.rs](../src/commands/env.rs)), take several values out across chunk boundaries ([src/coding/tests.rs](../src/coding/tests.rs)), and keep the list in the store, never in `config.toml`. On Linux, in Bubblewrap or Landlock, `no_secret_goes_on_a_command_line` checks no process on the computer has a value in its arguments while a run sleeps, and `a_folders_environment_reaches_its_commands_and_never_prints` sets a token and a value worked out from it by `$( )` in the sandbox, checks both reach the command and neither prints, and that a failing `$( )` stops the run naming its variable, with its error stream redacted ([src/commands/tests.rs](../src/commands/tests.rs)). Not yet run on macOS (Seatbelt) or Windows (process containers, where `$( )` is PowerShell's).

1Password's CLI in the sandbox, tried on Arch Linux (Bubblewrap, 2026-11-04) without a token: `/usr/bin/op` 2.41.1 is there (the system is read), runs (its setgid bit does nothing in the sandbox, which only the desktop app's sign-in needs), and makes its config folder in the run's own home; `op whoami` answers "no account found". A real `op read` with a service account's token, which needs the folder's network, hasn't been tried.

## Not built yet

- **The desktop app's field**, in a folder's settings, with the same rules as the CLI: an empty text area whose placeholder shows two example lines,

  ```
  API_KEY=your-key
  DB_PASSWORD=$(op read op://Vault/Item/password)
  ```

  and one quiet line under it: "One per line. $( ) runs a command on this computer when a run starts." It checks as you type, naming the line: a line without `=`, a name with spaces, an unclosed `$(` or quote, a name the sandbox sets. No paragraphs. It needs a control request that sets, lists the names of and clears a folder's list, as `cww roots env` does.
- **Naming the variables on approval cards.** Chat with Work's card could say "gets DB_PASSWORD, API_KEY" if Desktop told it the names. Today nothing about the list leaves the computer; naming them would mean adding the names (only) to a folder's `roots` entry (PROTOCOL.md 7.2.1) and the server showing them on the card.
- Expanding `$NAME` inside values.
