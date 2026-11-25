# What the assistant can do

Every tool Chat with Work gets on this computer, and how fast it reads. Back to the [README](../README.md).

## What the assistant can do

| Tool | What it does |
|---|---|
| `roots` | Lists your shared folders by ID and label. The server never sees absolute paths. |
| `search` | Full-text search, 20 hits at most, with a snippet of up to 300 characters each. A live grep covers files that aren't indexed yet. |
| `list` | Lists one folder, 200 entries per page. |
| `read` | Returns the text of one file, 32 000 characters at a time. PDF, Word, PowerPoint and Excel files are converted to text. |
| `grep` | Searches code like ripgrep: lines matching a regular expression, with context lines, or the files that match, or how many lines in each. It skips what Git ignores, hidden files and binary files. |
| `glob` | Finds files by name (`**/*.rb`), with their size and modification time. |
| `read_bytes` | Copies one file, of any kind, up to 25 MiB, a chunk at a time, for Chat with Work to attach to a chat. Chat with Work uses it itself, never the model. |

In folders where you allow changes, and only while at least one folder does:

| Tool | What it does |
|---|---|
| `create` | Creates a text file. Never overwrites anything. |
| `write` | Replaces a text file's content, or adds to its end. The previous version goes to the trash. |
| `edit` | Replaces one exact passage of a text file. The previous version goes to the trash. |
| `mkdir` | Creates a folder. |
| `move` | Moves or renames a file or folder, within or between folders that allow changes. A file it replaces goes to the trash. |
| `delete` | Moves a file or folder to the system trash. |
| `create_document` | Creates a Word document from Markdown, or an Excel workbook from rows of values (never formulas). Replacing one puts the old file in the trash; existing documents are never edited in place. |
| `patch` | Edits several text files and creates new ones in one change, approved on one card: all of it or none. The previous versions go to the trash. |

Chat with Work asks before each change; "Allow for the rest of this chat" can cover writes, never a replace, a move over a file, or a delete. Programs, scripts that run when opened, shortcuts and Office files with macros are never made, no file gets an execute bit, and version control internals, shell startup files, autostart folders and editor task settings are never changed.

In folders where you also allow commands (`cww roots allow-commands <root>`, or the folder's switch in the app or the terminal UI), on Linux, macOS, and Windows 11 24H2 with the August 2026 update or later:

| Tool | What it does |
|---|---|
| `run` | Runs one line of shell in the folder (Windows PowerShell on Windows), like `bin/rails test`, in a sandbox of its own that can change only that folder, reaching this computer's own services (its databases and development servers, in Docker or not) but not the local network or the internet unless you allow them, with CPU and time limits (and memory on Linux and Windows). Its output comes as it's printed; a long command keeps running. |
| `run_status`, `run_output`, `run_stop` | Follow a command left running, read what it printed, or stop it. |

Each command is approved in Chat with Work (on the web or in its apps, never on this computer), or runs without asking in a chat where you chose "Allow everything" as the permission mode, as file changes do there; guests, scheduled and event runs, and bots' and agents' chats always ask. This computer decides where commands may run at all: only in folders where you allow them, and how far they reach there: "None", "Local services" (the default) or "Local services and the internet". When the sandbox refuses a command a connection or a path, its result says which, and the app asks you before allowing one. See [docs/commands.md](commands.md).

### Sharing folders with bots

A Chat with Work bot (an assistant your organization sets up to work by itself, like one that fixes issues) reaches this computer only through the folders you share with it, here: `cww roots share <root> <bot-id>`, with the number in the bot's address in Chat with Work. Its chats then reach that folder as the folder allows, and no other, and each of its commands asks you first in Chat with Work. Where the folder allows commands, `--unattended` lets the bot's commands there start without anyone approving each one, in the same sandbox and limits, which is how a bot runs tests on a server or a spare Mac by itself. `cww roots unshare <root> <bot-id>` stops sharing, and stopping commands in a folder ends its unattended commands. Chat with Work can ask for unattended commands (a bot's "Always allow"), never turn them on: the desktop app asks you in its own dialog, or `cww roots requests` lists what's waiting. See PROTOCOL.md 7.2.1.

For a coding bot (one that runs Claude Code, Codex, OpenCode, Gemini CLI or pi and your tests, like the Issue fixer), there's nothing to type: have the agent installed and signed in the normal way, press **Set up** on its card, and the app shows what it checked and asks once. Allow lends it a new folder, with this computer's own sign-in for that agent. The first run on each repository of that owner then copies it from a mirror only Desktop fetches, installs its toolchain, and runs its tests in the sandbox, by itself. For scripts and servers, `cww coding setup --bot "Issue fixer"` does the same in one guided step, picking the bot by name. See [docs/coding-computers.md](coding-computers.md).

### Thinking with your own ChatGPT plan (not offered yet)

A chat or bot will be able to use your own ChatGPT plan (Codex), signed in on this computer, as its model: the agent only thinks, in a sandbox with no files of yours and no tools but Chat with Work's, which keep running on the server. Chat with Work can only ask; you allow it here, for your chats or a bot, with `cww plans allow codex` or in the app's dialog. It hasn't been tried on a real ChatGPT sign-in yet, so it's offered to nobody unless `[plans] offer_codex = true` is set by hand in `config.toml`. See [docs/own-plans.md](own-plans.md).

### Your browser

With the [Chat with Work browser extension](https://chromewebstore.google.com/detail/ehjfihpogoacjodjnanclodinjnnpmcp) in Chrome, Chromium, Edge, Brave or Vivaldi, Chat with Work can also use your own browser, with your sign-ins, on sites you allow: `cww browser enable`, then `cww browser allow github.com`. It works only in its own tab group, "Chat with Work", in the background, so you keep using your other tabs. It reads pages as accessible text (`browser_tabs`, `browser_read`, `browser_screenshot`, `browser_scroll`, `browser_wait`). With `cww browser allow-changes` it also opens pages, clicks, types, chooses, closes its tabs and saves files into a folder that allows changes, each asking you first in Chat with Work; anything that may send, buy, post or delete shows you a screenshot of what will be clicked. It never types into or reads password and payment fields, and stops at once with Stop in the extension's menu, Stop Using the Browser in the tray, or `cww browser stop`. The desktop app's settings have a Your Browser page for all of it. See [docs/browser.md](browser.md).

Paths always look like `work-docs:plans/q3.pdf`.

## Performance

The daemon does nothing until something happens. There is no polling: the index follows file events (inotify, FSEvents, ReadDirectoryChangesW) and waits for a folder to be quiet for a moment before it updates, and only the directories that changed are rescanned. The only regular wake-ups are the server's WebSocket pings.

The index watches only the folders it indexes: never what Git or `.ignore` ignores, hidden folders such as `.git`, dependencies (`node_modules`, Python virtual environments), build output in a software project (`target`, `build`, `dist`, `vendor`, `tmp` and the like), caches marked with a `CACHEDIR.TAG`, linked Git worktrees, anything on the deny list, or a link out of the folder. None of those are indexed either. On Linux each watched folder takes a kernel watch from a pool every program on the computer shares, so the daemon holds at most 8,192 of them, or a tenth of the system's limit (`fs.inotify.max_user_watches`) when that is less, the shallowest folders first. Folders past that are brought up to date when a search asks (at most once a minute) and by the periodic rescan, and the log says so once. On macOS and Windows one watch covers each shared folder, and events from folders the index skips are dropped before they cost a rescan.

Measured on real Documents folders, connected to a Chat with Work server:

| | Linux | macOS | Windows |
|---|---|---|---|
| Machine | Ryzen 9 9900X, SATA SSD, Arch | Mac mini M4, macOS 27 | Zenbook S16, Ryzen AI 9 HX 370, Windows 11 |
| Folder | 15 GB, 10,504 files indexed | 37 GB, 172,974 files indexed | 427 MB, 8,417 files indexed |
| First index | 107 s from a cold disk, 15 s cached; 17 CPU s | 256 s cold, 115 s cached; 55 CPU s | 156 s cold (Defender scans each file once), 1.5 s cached; 9 CPU s |
| Index on disk | 160 MB | 242 MB | 2 MB (mostly binary files, indexed by name) |
| Search, 20 hits with snippets | 1.6 to 11 ms, median 5.6 ms | 3.6 to 52 ms, median 12 ms | 4 to 5 ms |
| Idle CPU over 5 minutes | under 0.01 s | 0.01 s | 0.17 s (small folder) |
| Memory while idle | 77 MB resident, 55 MB heap | 29 to 66 MB footprint | 31 MB working set |

A search takes about 100 ms end to end through Chat with Work, most of it on the server. When the index has no answer, a live grep runs as well; over a folder of thousands of files that aren't text, that can take a second or more (at most 5 seconds). Indexing peaks at a few hundred megabytes while large PDFs and spreadsheets are extracted, and the memory is handed back when the pass ends.

Opening a saved index runs in the background too: settings and status are available while a folder says `pending`, and searches use live grep until its index is ready. Changing shared folders cancels the previous scan; an interrupted traversal never removes files just because it had not reached them yet. A sandbox restart waits at most half a second for indexing to stop. If a synchronous parser is still finishing one file, the old worker process ends it, and the new worker resumes from the last committed index. Folder permissions still change through the same confined-worker restart.

After an upgrade, the app checks that a running background agent uses the installed version, including when a package manager installed it. Its supervisor and confined worker must be the same version: a new worker refuses an old supervisor, allowing the service manager to restart the complete agent before it handles settings or commands. The app leaves a stopped agent stopped; for a foreground terminal session, restart `cww daemon run` after upgrading.
