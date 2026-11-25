# The terminal

`cww tui` and bots from the terminal. Back to the [README](../README.md).

## Terminal UI

```sh
cww        # or cww tui
```

Sign in to another organization with `cww login --organization`, then list
saved sign-ins with `cww organizations` (`--json` for scripts). Open its chats
with `cww tui --organization "Akamai Robotics"`, using its name or sign-in ID.
Sign-in IDs may start with a hyphen; use the complete ID for both `tui` and `logout`.
Names must be unambiguous. Chat requests, notifications and question history
stay with that sign-in. Shared folders, daemon settings and dictation model
preferences remain on this computer's original connection. To remove only one
sign-in, use `cww logout --organization ID`. The desktop sidebar also switches
between signed-in organizations; This computer > Account adds or removes them.

In the desktop chat sidebar, your name opens Account settings. Profile names
and the answer-copying preference save to the selected organization. Rich text
copies bold, lists, links, and tables for document editors, plus the original
Markdown for plain editors. Markdown copies the source text alone. Copying
preferences save separately from unsaved profile names. Sign-in credentials,
account deletion, and profile pictures still open the website.

Run in a terminal, `cww` opens a terminal UI laid out like the desktop app's chat page. On the left: New chat, the search, Recent (your chats, the one asked in last first, as on the web, so asking in an older chat brings it back to the top), and Pinned under it as on the web: the projects you pinned (picking one shows its chats and starts a new chat in it; picking it again shows every chat), then the chats you pinned, with your name at the foot, and your credits under it when they run low. The list stays current as chats start, change or go, on the web too. On the right, with no title over it, as on the web: the open chat and the composer, with the model on its edge. Your name, `,` or `/settings` open the terminal’s local settings: Shared folders (with their index state), Excluded files (the deny list, which only `config.toml` changes), Activity (every request from Chat with Work, colour-coded by decision), Account (the pairing, the connection, whether chats are allowed here, your organization and credits, and the web's own settings pages, which open in the browser) and General (the daemon, pausing, and where its files are). Nothing else stays on screen: a single word at the top right says when something needs you (the daemon stopped, offline, connecting, paused, not paired, revoked, or the open chat not live), and clicking it opens the page that helps.

In the Chat with Work window, dictate with the microphone beside the composer's paperclip, as on the web. In the terminal, use `Ctrl-R` / `/dictate`, and choose its transcription in the desktop app's **General → Dictation** settings: **Cloud transcription** uses your paired server in the EU and credits; **On this computer** uses Whisper locally with no audio upload or credits. There's nothing to install: the Whisper model that suits this computer (Tiny, Base or Small, by its memory and cores; you can pick another) downloads from Hugging Face when you choose On this computer, or while you speak the first time you dictate, with its progress shown, and then works offline. Models support multiple languages, are checked against their SHA-256 before use, can be removed, and are not bundled in releases. Local failures never fall back to cloud. The terminal shares this choice. Words remain editable before you send them. Stop with the microphone button or Escape; Escape during transcription discards the result. The terminal also accepts Ctrl-C to discard a recording. Recordings stay in memory, stop after five minutes, and silence is never uploaded. Dictation needs the desktop app installed beside `cww`; terminal-only archives have no microphone helper. System microphone permission may be requested on first use.

In the terminal, `/web on` and `/web off` choose web search for the current chat. The first use explains the rate and privacy note. The choice requires the organization's permission and a model that supports web search. Scheduled chats show their schedule context and a clock in the list. Code runs show their source and output under `/steps`; files they produce stay visible with the steps folded. `/download` lists files and `/download <number> <path>` saves one. Local file approvals include the proposed text, diff or document preview and any warning from the computer.

| Key | Does |
|---|---|
| `Tab` | Move between the chats and the composer |
| `↑` `↓`, `j` `k`, `Enter` | Select and open a chat |
| `n` | New chat |
| `/` | Search chats |
| `e` | Show every tool step under its activity line |
| `o` | Open the chat in the browser, or ask to use your chats here |
| `,` | Open the settings, or go back to the chats |
| `Tab`, `←` `→`, `1` to `5` | In the settings: the next, previous or numbered page |
| `l` | The activity log (`PgUp`, `PgDn`, `End` to follow) |
| `a` | Share a folder (prefilled with your Documents folder if it isn't shared) |
| `r` | In Shared folders, rename the selected folder; elsewhere, look for the daemon again |
| `d`, `x`, `Delete` | In Shared folders, stop sharing the selected folder, after asking; in Account, disconnect this computer |
| `p` | Pause or resume answering Chat with Work |
| `c` | Pair this computer: shows the code, opens the approval page in your browser, and waits |
| `s` | Start the daemon in the background (`cww daemon install`) when it isn't running |
| `Esc` | Back to the chats |
| `?` | All the keys and commands |
| `q` | Quit |
| `Ctrl-C` | Clear what's typed; with nothing to clear, twice in a row quits |
| `Ctrl-L` | Draw the screen again |

In the composer, keys work as in Claude Code:

| Key | Does |
|---|---|
| `Enter` | Send, or run a `/` command |
| `Shift-Enter`, `Alt-Enter`, `Ctrl-J`, or `\` then `Enter` | A new line (`Shift-Enter` where the terminal tells it apart from `Enter`) |
| `↑` `↓` | Move between lines; from the first or last line, earlier questions |
| `←` `→`, `Home` `End`, `Ctrl-A` `Ctrl-E` | Move along the line |
| `Ctrl-U`, `Ctrl-K`, `Ctrl-W` | Delete to the start of the line, to its end, or the word before |
| `Esc` | Close the command list; stop an answer being written; or go back to the chats |

Typing `/` lists the commands and narrows them as you type; `↑` `↓` select, `Tab` completes and `Enter` runs one. `//` at the start asks something that begins with a slash.

| Command | Does |
|---|---|
| `/new` (`/clear`) | Start a new chat |
| `/resume` (`/chats`) | Pick a recent chat from a list you can filter |
| `/search <text>` | Search your chats in the sidebar |
| `/model [name]` | Pick the model for this chat, or the next new one, from what your plan offers; models you can't use now say why |
| `/project [name]` | Start the next chat in a project (`/project none` for none) |
| `/attach <path>`, `/detach` | Attach a file to your next question (up to 25 MB), or remove the attached files |
| `/download [number path]` | List files in this chat, or save one to a chosen path without overwriting |
| `/dictate` or `Ctrl-R` in the composer | Start or stop dictation using credits; Escape stops and transcribes, Ctrl-C discards |
| `/copy` | Copy the last answer to the clipboard |
| `/open` | Open the chat in the browser |
| `/retry`, `/branch`, `/rename [title]`, `/delete` | Answer the last question again, continue the chat in a new one, rename it, or delete it (after asking), where Chat with Work allows it |
| `/share-chat`, `/unshare-chat` | Make a public link to the chat and copy it, or stop sharing it |
| `/steps` | Show or hide every tool step |
| `/commands` | Follow or stop the commands Chat with Work runs on this computer (shown while any ran) |
| `/settings` | Open the settings |
| `/folders`, `/share [path]`, `/unshare [folder]` | Show your shared folders, share one, or stop sharing one (after asking) |
| `/pause`, `/resume-sharing` | Refuse Chat with Work's requests for now, or answer them again |
| `/log`, `/status` | Show the activity log, or say how this computer is connected |
| `/login`, `/logout` | Pair this computer, or disconnect it by forgetting its pairing (after asking) |
| `/help`, `/exit` (`/quit`) | All the keys and commands; quit |

When an answer stops at a change that needs your approval (posting to Slack, creating an issue), the chat shows what it will do and asks: `↑` `↓` and `Enter`, or `y` to approve, `a` to approve it for the rest of the chat where that's allowed, `n` to deny, or deny and say what to do instead. When a connected service asks you something while its tool runs, the chat shows its question as a small form: `↑` `↓` move between the fields, `←` `→` go through a field's choices, `Enter` types a value, then Send or Decline; a service that needs you to open a page offers to open it in the browser. In a project chat, it says whose answer the chat waits for.

The mouse works too: click a chat, the search, your name, a settings page, a folder, the composer, a command, a picker's row, an answer to an approval, or a link in an answer; the wheel scrolls the chat, the lists and the activity log. While the TUI has the mouse, hold `Shift` (or your terminal's own modifier, such as `Option` in iTerm2) and drag to select text, or set `[tui] mouse = false` in `config.toml` to leave the mouse to the terminal.

Questions you ask, and the commands you run, are kept in `~/.local/state/cww/history` (only readable by you, the newest 500) so `↑` recalls them in the next run. Files you attach are read by `cww` itself, not the sandboxed daemon, and uploaded through the daemon only when you attach them; the clipboard gets text through your terminal (OSC 52), so it also works over SSH where the terminal allows it.

A first run is three keys: `c` to pair, `s` to start the daemon if the installer didn't, and `y` to share your Documents folder, which it offers when nothing is shared and never shares on its own. Until the daemon runs, the UI works from `config.toml` and the audit file, and changes are saved there, like the `cww roots` commands. `CWW_SERVER=https://chat.example.com cww` pairs with a self-hosted server.

Chats in the terminal are a separate permission from sharing folders. `cww login` asks for both, and the approval page lets you decline chats (`cww login --no-chats` doesn't ask). A computer paired without them asks when you press `o`, and you allow it in Chat with Work under Settings, Computers; you can turn it off there at any time. The TUI never holds a token: the daemon asks Chat with Work for it, and answers stream over the daemon's connection. The logos and file-type icons chats show (in the desktop app) come from the paired server too: the daemon fetches only images under its `/assets/`, without the token, at most 1 MiB each and never through a redirect. [CHAT_API.md](../CHAT_API.md) explains the design.

Everything the sidebar does has a slash command too, so the chat can take the whole width: `[` or `/sidebar` hides it (and it stays hidden next time), and `/new`, `/new-bot`, `/decisions`, `/search`, `/chats`, `/bots`, `/projects`, `/open <number or name>`, `/pin`, `/settings` and `/logout` do the rest. `/org` (or `/organization`) switches to another organization you're signed in to on this computer (`cww login --organization`), by its name, its number in the list, or a picker, and `cww tui` opens there next time; `cww tui --organization` still picks one for a run. Pages update by themselves as chats answer and bots run, and catch up after a reconnect; `r` or `/refresh` reads the screen again by hand.

The UI uses no CPU while nothing happens and draws in your terminal's own colours: it never paints a background, so it looks the same as the rest of your terminal, light or dark, themed or not. It uses your foreground with bold and dim, reverse video for the selection, and the 16 ANSI colours only for state (green done, blue working, yellow waiting for you, red failed), which your theme restyles. With `NO_COLOR` set it uses no colour at all.

## Bots from the terminal

`cww bots` makes and manages your Chat with Work bots from a terminal, for you or for an agent running there, as many as you like. It does what the Bots pages do, as you, and nothing you couldn't do there.

It's a separate permission from folders and chats. Ask for it while pairing with `cww login --bots`, which adds an unticked choice to the approval page. On a computer that's already paired, the first `cww bots` asks, and you allow it in Chat with Work under Settings, Computers, where you can turn it off at any time. Anything running on this computer can use it once it's allowed. It uses the computer's own key, so the daemon doesn't need to be running.

```sh
cww bots list
cww bots show 42
cww bots recipes                       # what you can start from
cww bots recipes lead_triage           # every choice its page asks, and what's still needed
cww bots recipes blank                 # the same, from nothing
cww bots triggers invoice              # what can start a bot
cww bots triggers --app github         # one app's events and fields

cww bots create --recipe lead_triage --tool crm=hubspot
cww bots create --name "Morning" --instructions "Summarize my day." \
  --app schedule --frequency weekday --at 08:30
cww bots create --file bot.yaml --name "Overrides the file"

cww bots update 42 --name "Triage" --when "every day at 10:00" --notify never
cww bots update 42 --writes slack=automatic --policy calendar_delete=ask
cww bots pause 42
cww bots resume 42
cww bots delete 42                     # asks first; --yes for agents
cww bots run 42                        # a try
cww bots run --recipe lead_triage      # a try of one not made yet
cww bots run 42 --no-wait              # then: cww bots run --follow <try>
```

Add `--org NAME` to work in another organization you signed in to with `cww login --organization --bots`, as `cww organizations` lists them. Add `--json` for programs: every command prints the server's JSON, and a failure prints `{"error": {"code": "...", "message": "...", ...}}` on stdout and exits with status 1. `delete` without a terminal needs `--yes`.

A bot starts as New bot would start it: from the recipe's own choices, with only what you say on top, replying as drafts and asking before changes. Anything the page requires is a flag or a setting in the file, and a missing one is refused with what's wrong (`cww bots recipes KEY` lists it as "Still needed"). Nothing gets wider than the page allows: `--writes automatic` is the page's own choice, only the person an integration acts as can let its changes run without asking, and a destructive tool set to automatic also needs `--yes`.

`--file` takes plain settings in YAML, the same names as the flags with `tools`, `integrations`, `monthly_credits_cap`, and `schedule` (for `update`, `when`, `tell`, `changes`, `reply_mode`, `writes`, `policies`):

```yaml
recipe: lead_triage
name: Lead triage
tools:
  crm: hubspot
integrations: [notion]
schedule:
  frequency: weekday
  at: "08:30"
```

Secrets never go through `cww bots`: it refuses any flag or file key that names one before anything is sent, and it never prints one. A webhook's address and secret, a channel's token, and a bot's API keys are on the bot's page.

Making a bot uses no credits by itself. `cww bots run` tries it once, changing nothing outside, and uses your credits like a chat, which it says first. Every create, update, pause, delete, and try is audited, naming this computer.
