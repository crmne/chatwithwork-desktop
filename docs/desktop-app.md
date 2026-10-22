# Chat with Work Desktop

`cww-app` is the desktop app: the Chat with Work web app in a window of its own, the tray item for the `cww` daemon, and the background agent's settings. It lives in `app/` and is built with [Tauri 2](https://tauri.app) on the system's web view: WebKitGTK 4.1 on Linux, WKWebView on macOS, WebView2 on Windows. The daemon doesn't need it: servers and people who prefer the command line keep using `cww` and `cww tui`.

Until 0.3.1 the app was native, drawn with egui, with a chat window of its own that had to follow every change to the web app. That code is kept at the [`egui-final`](https://github.com/crmne/chatwithwork-desktop/tree/egui-final) tag, with its docs (`docs/settings-app.md`, `docs/chat-window.md`, `docs/native-parity.md`). The measurements that led here are in [tauri-prototype.md](tauri-prototype.md).

## Three parts

| | What | Talks to |
|---|---|---|
| **Chat with Work window** (`main`, and `page-N` for pages it opens in a new window) | The real web app, loaded from the server | The server, like a browser. The app only through the bridge below |
| **Settings window** (`settings`) | Pages bundled in the binary (`app/ui`): welcome, shared folders, always private, activity, account (pairing), notifications, general | The daemon, through `local.rs`'s allow-list, and the app's own settings |
| **Tray item** | The daemon's state, Open Chat with Work, Pause Sharing, Settings, Quit | The daemon's status, followed by one thread |

While no window is open the app is its tray item and that thread: a window's web view, and on Linux and macOS its web processes, go with the window. Closing the last window never quits; Quit in the tray or in General does. On macOS the app has a Dock icon only while a window is open.

The same binary is also three pipe-only helpers the daemon and `cww tui` start, before any window or tray exists: `--notification-broker` (native notifications, through fastframe-notifications in `notification-broker/`), `--record-audio` / `--record-audio-progress` (the terminal composer's microphone, through fastframe-audio), and `--transcribe-local <model>` (Whisper on this computer). They keep the name `cww-app`, so the daemon, `cww tui`, the packages and existing start-at-login entries find them where they were.

## The Chat with Work window

- **Which server.** The server this computer is paired with (`config.toml`'s `server.url`), so staging and self-hosted installs open their own site; chatwithwork.com before pairing; `--server <url>` for development (plain `http` only on localhost). When the computer is paired with another server, the window follows.
- **Where it opens.** `/chats/new`, which leads into the organization used last, as the installed web app does. A notification's chat (`--open-chat`, as the daemon starts it) opens at `/<account>/chats/<number>`, only on that same server.
- **Staying signed in.** Cookies live in the web view's own data directory (`~/.local/share/com.chatwithwork.local-agent` on Linux, the app's WebKit store on macOS, `%LOCALAPPDATA%\com.chatwithwork.local-agent` on Windows). A web view drops session cookies when the app quits, so the server signs the desktop in with "remember me" (see the server contract).
- **Who it is.** The user agent is the platform web view's plus `(Chat with Work Desktop; platform=linux; version=0.3.1; build=301; bridge-components: [external preferences auth-session downloads])`, in parentheses, as a comment: WebKitGTK refuses a user agent that is not RFC 7231 products and comments (`user_agent.rs`). It never matches the phone apps' `Chat with Work; platform=ios|android`, so the server shows the desktop the web app's full layout.
- **Leaving the server.** Links and `GET` forms to other sites open in the default browser (the bridge script intercepts them before the web view does); `mailto:` opens the mail app; other schemes go nowhere. A page from elsewhere that loads anyway as the window's page (a redirect, a script) is stopped and opened in the browser. Other sites may load in frames, since the web views give the navigation handler only a URL for the page and its frames alike (the MCP Apps sandbox, the support widget and payment forms are frames). `target="_blank"` on the server's own pages opens another app window, which shares the sign-in (`policy.rs`).
- **Signing in with Google, Slack or Dropbox, and connecting services.** Google refuses OAuth in web views, and passkeys don't work there, so these run in the default browser and come back through a loopback callback (RFC 8252): the page asks the bridge to `listen()`, which answers `http://127.0.0.1:<port>/<32 random characters>` for ten minutes; the server sends the browser there at the end with a one-time token, which the page redeems with the PKCE verifier only it holds. Anything else that reads the token can't use it. The browser shows "You can close this tab". See the server contract for the flows.
- **Files.** Dropped on the window, they reach the page as an HTML drop, so the composer takes them as uploads (Tauri's own drop handler is off). Downloads go to the Downloads folder under their own name, never replacing a file there, and the folder is shown.
- **Dictation.** The microphone is allowed for the server's pages only, so the composer's microphone works as on the web (macOS asks once, with the bundle's `NSMicrophoneUsageDescription`). Nothing else is granted to any page.
- **Offline.** When the server can't be reached (a TCP connection to its host and port within four seconds, name lookup included, checked before the window opens or moves to the server, and skipped behind a proxy from config.toml or the environment), the window shows a page bundled in the app instead of the web view's error page: which server, Retry, Open Settings, Open Anyway (for a system proxy the check can't see), and a check of its own every 15 seconds that goes back where the window was headed once the server answers (`offline.rs`, `ui/offline.html`). On Linux a server page that fails to load later is checked the same way; WKWebView and WebView2 don't report failed loads to Tauri, so there the check happens when the window opens or a notification or the settings move it. The page gets three commands of its own (`offline_info`, `offline_retry`, `offline_settings`), through a capability for bundled pages in that window only.
- **Notifications.** The daemon already shows native notifications, with the window open or closed, with generic text, and the web page tells the server which chat is on screen, so the window adds none of its own. Web Push in the web view isn't used.
- **Shortcuts.** Ctrl+, (Cmd+, on macOS) opens the settings; Ctrl and plus or minus zoom.
- **Linux on NVIDIA.** Under Wayland with NVIDIA's driver, WebKitGTK can't draw with explicit sync (the window stays empty or the GPU process dies with protocol error 71), so the app sets `__NV_DISABLE_EXPLICIT_SYNC=1` for itself, only when the driver is loaded and the session is Wayland, and never over a value already set (`linux.rs`).

## The bridge

The web app's pages get exactly three commands, and nothing else Tauri offers (no events, windows, files or shell):

| Command | Page API (`window.ChatWithWorkDesktop`) | Does |
|---|---|---|
| `open_external` | `openExternal(url)` | Opens an `http`, `https` or `mailto` URL in the default app |
| `open_preferences` | `openPreferences()` | Opens the settings window |
| `listen` | `listen()` | Starts the loopback callback; resolves `{ callback }`, and later dispatches `chatwithwork-desktop:callback` with `{ callback, params }` |

Two checks enforce it. A capability added at start (`desktop.rs`), for the server's origin as a remote URL and the web app's windows only, allows these three commands; and each command checks again that its caller is a page of the server in one of those windows (`bridge.rs`). The bridge script (`bridge.js`) defines `window.ChatWithWorkDesktop` only on pages of the server.

## The settings pages and the daemon

The settings window shows `app/ui`, plain HTML, CSS and JavaScript without a framework or build step, styled with the web app's Live Wire tokens (`ui/tokens.css`, copied from the web app) and Geist. They work without a network and before pairing. Only they reach the daemon:

- The capability in `capabilities/local.json` names the `settings` window and no remote URL. Each command in `local.rs` checks again that its caller is a bundled page in that window.
- `control` sends a control request (CONTROL.md) only if it is on `local::ALLOWED`: status, pause and resume, shared folders and their changes and commands, lending them to bots (`roots_bot`), the deny list, the activity log, and notification permission. Never chats, uploads, `reload` or `shutdown`.
- The rest are the app's own: pairing (it runs `cww login --json` and reports its code and outcome as `pairing` events), the folder picker, starting the daemon (`cww daemon install`), start at login, crash reports, dictation preferences and Whisper models for `cww tui`, opening the config file or the logs, and opening the Chat with Work window on one of the server's pages.
- The daemon's status arrives as a `status` event, which the app hands to the page by calling its `window.cwwEvent` (so the pages need none of Tauri's own APIs, not even its events); the activity log's sentences are made in Rust (`activity.rs`), as the egui app made them.

Organizations, people, notification preferences, models and templates are in the web app's Settings, in the Chat with Work window.

## Lending folders to bots

Each shared folder lists the Chat with Work bots it is lent to (`status`'s `roots[].bots`), with Lend to a Bot (by the bot's ID, the number in its address in Chat with Work, and an optional name to recognize it), a switch per bot for unattended commands where the folder allows commands, and Stop Lending. All of it is one `roots_bot` request. As CONTROL.md asks, lending says what the bot's chats can then do in the folder (read, change where changes are allowed, run commands where commands are allowed, each asking in Chat with Work unless the bot's own settings let it run) and asks first, and so does turning unattended commands on; stopping, and turning unattended off, happen at once.

## Testing and pictures

`cargo test -p cww-app` covers the parts that decide something: the navigation rules, the server and its pages, the user agent, the loopback callback, the download names, the allow-list, the activity log's sentences, the tray's labels, and the NVIDIA rule.

`cargo run -p cww-app --features demo -- --demo` runs against a stand-in daemon with sample folders and activity (`--demo-fresh` for a first run). On Linux, `CWW_APP_SNAPSHOT=<dir>` makes a demo build save each settings page as a PNG and quit (`CWW_APP_SNAPSHOT_PAGES`, `CWW_APP_SNAPSHOT_THEME`; `main` is the Chat with Work window). WebKitGTK paints the pictures itself, so they work with the window on a hidden workspace.

## Not done yet

- The settings pages are not covered by automated UI tests; the egui app's snapshot tests went with it.
- On macOS and Windows, losing the network while a page is open shows the web view's own error page until the window is reopened or moved; only Linux reports failed loads.
- The tray menu can't hide its detail line, so with nothing to say it is an empty, greyed-out line.
