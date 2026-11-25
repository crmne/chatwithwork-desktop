# Configuration

`config.toml`, proxies and crash reports. Back to the [README](../README.md).

## config.toml

Everything lives in `~/.config/cww/config.toml`. The server can't change any of it.

```toml
paused = false
proxy = "http://proxy.example:3128"  # optional; see "Behind a proxy"
crash_reports = false                # see "Crash reports"; unset until you choose

[[roots]]
id = "work-docs"
label = "Work docs"
path = "/Users/carmine/Documents/Work"
follow_symlinks = false
writable = false      # true lets Chat with Work change files here (cww roots allow-changes)
# read_budget = 5000000  # raises the hourly read limit for this folder, if [limits] sets one (cww roots read-budget)

[deny]
extra = ["*.secret", "Clients/Confidential"]  # more patterns to block
remove = ["*.key"]                             # built-in patterns to drop (Keynote files use .key);
                                               # also drops paths from the never-changed list
allow_hardlinks = false                        # reads only: changes never touch hard links

[limits]
# Time-based limits are off: 0 means no limit. Set any of them for a local
# cap that holds whatever the server does (see SECURITY.md).
calls_per_minute = 0
read_chars_per_chat_hour = 0
read_chars_per_hour = 0
copy_bytes_per_hour = 0         # files read_bytes copies into chats
changes_per_minute = 0
changes_per_day = 0
change_bytes_per_hour = 0       # total bytes written by changes
# Size limits are always on.
read_chars_per_call = 32000     # longer reads page through with next_offset
max_change_file_bytes = 10485760 # largest file a change writes or copies
max_change_entries = 1000        # most files a folder move or delete may carry

[index]
enabled = true
watch = true          # follow file events; the index sleeps between them
rescan_secs = 1800    # rescan interval for folders not watched in full (watching off, or past the watch limit)

[sandbox]
enabled = true        # Landlock on Linux, Seatbelt on macOS

[tui]
mouse = true          # clicks and the wheel in cww tui; false leaves the mouse to the terminal

[desktop]
# reduce_motion = true  # no fading in of streamed text; unset follows the system's setting
```

Run `cww reload` after editing the file. The `cww roots` commands reload the daemon for you.

| Path | Contents |
|---|---|
| `~/.config/cww/` | `config.toml`, `dictation.json`, and `secrets.json` if no keychain is available |
| `~/.local/share/cww/speech-models/` | Whisper models for local dictation, downloaded on first use and verified before use (`0700`, files `0600`), removable in General settings |
| `~/.local/share/cww/ocr-languages/` | OCR languages added in the scan window on Linux (`<code>.traineddata`), verified before they are kept (`0700`, files `0600`) |
| `~/.local/share/cww/index/` | The search index (`0700`), with `cww-indexed-roots.json`, the folders whose first pass ended (their IDs, paths and counts) |
| `~/.local/share/cww/chat-assets/` | The logos and file-type icons the desktop app's chats show, as the paired server sent them, kept so each is fetched once (`0700`, files `0600`) |
| `~/.local/state/cww/audit.jsonl` | The audit log (`0600`, rotated at 10 MB, never uploaded) |
| `~/.local/state/cww/crash-reports.json` | When crash reports were sent in the last hour, for their limits (`0600`; only with crash reports on) |
| `~/.local/state/cww/history` | The questions and commands typed in `cww tui`, for `↑` (`0600`, never uploaded) |
| `$XDG_RUNTIME_DIR/cww/cww.sock` | The control socket for the CLI (`0600`) |
| `~/.local/share/Trash` (`$XDG_DATA_HOME/Trash`) | Where changes put old versions and deleted files on Linux: the desktop's own trash, or `.Trash-$uid` at the top of another filesystem. `~/.Trash` (or a volume's `.Trashes`) on macOS, the Recycle Bin on Windows. |

The same layout is used on macOS. On Windows everything lives under `%LOCALAPPDATA%\cww` (`config`, `data`, `state`), and the control channel is a named pipe. `CWW_HOME=/some/dir` puts all of it under one directory.

## Behind a proxy

Some networks only let traffic out through an HTTP proxy. `cww` sends pairing, token refreshes, the chats, the images they show and the WebSocket through one with HTTP `CONNECT`, so TLS still runs end to end between `cww` and Chat with Work: the proxy sees the server's host name and nothing else.

The daemon runs as a background service (systemd, launchd, or a Windows logon task), and services don't inherit your shell's environment. So set the proxy in `config.toml`, at the top of the file, before any `[section]`:

```toml
proxy = "http://proxy.example:3128"
# or with credentials, sent as Basic auth:
proxy = "http://alice:s3cret@proxy.example:3128"
```

Then run `cww reload` (or `cww login` if you haven't paired yet). `proxy = "none"` connects directly whatever the environment says. `config.toml` is readable only by you (`0600`), but a proxy password in it is stored in plain text; percent-encode special characters in the user name or password (`@` as `%40`). An `https://` proxy URL also encrypts the hop to the proxy. SOCKS proxies and NTLM or Kerberos authentication aren't supported.

Without `proxy` in the config, `cww` reads the environment as curl does: `https_proxy` or `HTTPS_PROXY` (`http_proxy` or `HTTP_PROXY` for a plain `http://` development server), then `all_proxy` or `ALL_PROXY`, unless the server matches `no_proxy` or `NO_PROXY` (comma-separated host names, which also match their subdomains; `.example.com` and `*.example.com` work too, as do IP addresses, CIDR ranges like `10.0.0.0/8`, `host:port`, and `*` for everything). That covers `cww login` and `cww daemon run` in a shell. `cww login` and `cww daemon install` warn when a proxy variable is set in the shell but not in `config.toml`.

`cww status` (and `--json`, and the terminal UI) show the proxy in use and where it came from, never the password:

```
Server:     https://chatwithwork.com
Proxy:      http://alice:***@proxy.example:3128 (from config)
```

`cww` doesn't read the proxy settings of macOS or Windows (System Settings, PAC files, WPAD); copy the proxy's address into `config.toml`. A proxy that inspects TLS needs its CA in the operating system's trust store, as described under [Pairing](install.md#pairing).

## Crash reports

Crash reports are off until you turn them on. The desktop app asks once, on its welcome page, and `cww tui` asks the first time it starts. Change your answer under General in either, or in `config.toml`:

```toml
crash_reports = true   # or false; the apps ask while it's unset
```

`CWW_CRASH_REPORTS=1` or `CWW_CRASH_REPORTS=0` in the environment overrides the file, for the daemon, the CLI and both apps. The daemon and the CLI never ask; they follow the setting. Every running part of the background agent reads it again when it crashes, so a change applies at once.

When it's on and the background agent panics (a crash in its Rust code), it sends one report to our own error tracker, a Bugsink server we run at `errors.plenty.is` in Germany, with no third party involved. A report holds:

- where in the source code it panicked (file, line and column);
- the panic's message, but only text written in the source code, or the start of one of Rust's own messages such as "index out of bounds", never text formed while it ran;
- the release (`cww@` and the version), whether it is a development build, which part crashed (`daemon`, `tui`, `cli` or `app`), and your operating system's name, kernel version and architecture.

It never holds your user or computer name, the command line, environment variables, your configuration, file names or contents, chats, or tokens, and paths in it start at `~` instead of your home folder. Native crashes (a segfault in a library) aren't reported yet.

A report is sent from a thread of its own while the crashed process waits up to five seconds, through the proxy you set (see [Behind a proxy](#behind-a-proxy)). At most five reports of the same crash and thirty in all go out an hour, even across restarts, and thirty per run. The terminal says whether it was sent.

Builds made with `CWW_CRASH_REPORTS_DSN=""` (see [PACKAGING.md](../PACKAGING.md)) can't send reports and never ask.
