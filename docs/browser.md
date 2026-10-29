# Using the browser

Status: **built: reading and acting, the desktop app's settings page and tray switch, and the server side; tested on Linux with Chromium. Not yet on the Chrome Web Store.** Chat with Work can use the person's own Chromium-family browser (Chrome, Chromium, Edge, Brave, Vivaldi), with their sign-ins, through the Chat with Work browser extension and Chat with Work Desktop. The extension's source is [`browser/`](../browser); the wire details are in [PROTOCOL.md](../PROTOCOL.md) and [CONTROL.md](../CONTROL.md).

## Shape

```
Chat with Work ──tunnel (MCP)──▶ cww daemon ──control socket──▶ cww (native messaging host) ──stdio──▶ extension ──chrome.debugger──▶ tabs in its group
```

- **Its own tab group.** The extension acts only on tabs in tab groups named "Chat with Work", in the person's real browser. Chrome keeps a group in one window, so there may be one such group per window, and naming a group "Chat with Work" shares its tabs. It opens its tabs in the first of them, in the background, so the person keeps using their other tabs. A tab the person drags into the group is shared with it; dragged out, it's theirs again. Everything else in the browser is out of reach: the extension never attaches to, reads or lists a tab outside the group.
- **Native messaging.** The browser starts `cww` as its native messaging host (`com.chatwithwork.desktop`) when the extension connects. That process only relays: it connects to the running daemon over the control socket (`browser_attach`) and passes messages both ways. `cww browser enable` writes the host manifest for every browser it finds (per user: the `NativeMessagingHosts` folders on Linux and macOS, the `HKCU\Software\...\NativeMessagingHosts` keys on Windows), naming only our extension's ID in `allowed_origins`. Flatpak and Snap browsers can't start a host outside their sandbox and aren't supported.
- **Tools, like the files.** While the person has turned the browser on (on this computer, never from the server), the daemon lists browser tools beside the file tools, with the same annotations, `writeWhen`, approval and audit model. The reads are built; the tools that change something, from `browser_open` on, come with changes, next:

  | Tool | Effect | Does |
  |---|---|---|
  | `browser_tabs` | read | The group's tabs: id, title, URL, whether its site is allowed |
  | `browser_read` | read | A tab's accessible text (roles, names, values, `[ref=…]` for what can be clicked or typed in), never raw HTML; paged, capped, counted against the read budget |
  | `browser_screenshot` | read | A JPEG of the tab, cut to fit a tool result |
  | `browser_scroll` | read | Scroll a tab, or bring a `ref` into view |
  | `browser_wait` | read | Wait up to 15 seconds for text to appear, or for the page to settle |
  | `browser_open` | write | Open an allowed URL in a new tab in the group, or in one of its tabs |
  | `browser_back` | write | Go back in a tab |
  | `browser_select` | write | Choose options in a `<select>` |
  | `browser_type` | write; destructive with `submit: true` | Type into a field, and press Enter with `submit` |
  | `browser_click` | destructive; write with `submits: false` | Click a `ref` |
  | `browser_close` | write | Close a tab in the group |
  | `browser_download` | write | Save a link's file into a shared folder that allows changes |

  The reads are offered while the browser is on; the rest only while the person also allows actions on the computer (`cww browser allow-changes`), like a folder's "Allow changes", and `browser_download` only while a folder allows changes. Each action takes `dry_run`, which checks it and describes it without doing it.

  Reads run without asking. Writes ask on the approval card and can be allowed for the rest of a chat. Destructive calls always ask, and their card shows a screenshot of the page with what will be clicked outlined (from a dry run, as for file changes).

## Safety

All of it holds on the computer, whatever the server sends. The extension enforces every rule below; the daemon enforces again the ones it can check from its side (the kill switch, the sites on every answer's address and in the tab list, bots, and the budgets), so a fault in one doesn't open those:

- **Sites.** The person allows sites on the computer (`cww browser allow github.com`, or Settings, This computer, Browser in the desktop app, which also turns the browser on, allows actions; bot sharing is selected from the bot’s computer integration). A bare host (`github.com`) allows the host and its subdomains over HTTPS; an entry written with its scheme (`http://localhost:3000`, `https://example.com`) allows exactly that scheme, host and port, without subdomains. The extension refuses to open, read, screenshot or act on a tab whose page isn't on the list (`site_not_allowed`); a tab that navigates off the list is left alone until it comes back. The daemon checks the URL every answer carries and drops the content of one that isn't allowed.
- **Submitting.** `browser_click` is destructive unless the call says `submits: false`, and the extension refuses such a call (`would_submit`) when the target submits a form, is a submit button, or is named like an action that buys, pays, sends, posts, publishes, deletes, removes, confirms, transfers or signs (in English, German, Italian, French and Spanish). So a click that may send or buy something always asks, with the screenshot. A plain link or a tab is a write. The name check is a heuristic: a site can still hide an action behind an innocent name, which is why writes ask too unless the person allowed them for the chat.
- **Passwords and payments.** The extension never types into, and never reads, password fields, card fields (`autocomplete="cc-*"`, card number, expiry, security code) or one-time codes (`sensitive_field`). `browser_read` leaves their values out, and screenshots blur them: a screenshot is refused (`page_error`) unless every such field it finds is blurred.
- **Outside text.** What a page says is outside text for the taint rule: after a chat reads a page, every change asks again, and browser changes never count as "writes to the computer's own files", since they leave the computer.
- **Downloads** go only into a shared folder that allows changes, through the same writer as file changes (no programs, the system's download mark, the size cap, the change budgets). A file is fetched only from an allowed address, with no redirect followed, not even within an allowed site, so the person's sign-in never goes anywhere unchecked. A download a page starts by itself while the extension acts on its tab (until it detaches, a minute after its last call) is cancelled and removed; the person's own downloads elsewhere are untouched.
- **Visible.** While the agent acts on a tab, the tab shows a "Chat with Work is using this tab" bar, the group's title says so, and the extension's icon has a badge. Chrome also shows its own "started debugging this browser" bar while the debugger is attached; the extension detaches after a minute without a call, so the bar doesn't stay.
- **Kill switch.** "Stop" in the extension's menu, "Stop Using the Browser" in the tray menu (shown while the browser is on), Stop in the desktop app's settings, and `cww browser stop` detach at once and refuse every browser call (`browser_stopped`) until the person resumes. Pausing the computer stops it too.
- **Bots.** A bot's chats reach the browser only when the person shares it on the computer (`cww browser share <bot>`), like a folder, and only the sites allowed there.
- **Audited** in the local audit log: the tool, tab, site (host only), outcome and duration, never page text.

## Background and foreground

Tabs in the group can stay in the background while the person works elsewhere. What Chrome does with a background tab:

- **It still loads, runs and answers.** The debugger's commands (reading the accessibility tree, clicking and typing with `Input` events, which are trusted and count as a user gesture) work in a background tab. Screenshots of a background tab work through the debugger. The extension turns on focus emulation, so pages that check `document.hasFocus()` behave as if focused.
- **Timers are throttled**: to once a second when hidden, and chained timers to once a minute after five minutes hidden (intensive throttling). `requestAnimationFrame` doesn't run. Pages that animate or poll can be slow to settle; `browser_wait` gives them time.
- **Tabs aren't discarded**: the extension marks its tabs `autoDiscardable: false`, so Memory Saver keeps them.
- **The person is needed** for what only the person can do in front of the browser: file pickers (the extension never uploads), permission prompts (camera, microphone, notifications, location), passkeys and security keys, the password manager and autofill, payment sheets, print dialogs and extensions' own popups. A JavaScript `alert` or `confirm` in the group is answered by the extension (dismissed) and reported in the result.
- Popups a group tab opens join the group.

## Why its own extension

Ours, open source (in this repository, MIT or Apache-2.0 like the daemon), to be published on the Chrome Web Store as "Chat with Work", never relying on a third-party extension. [Playwriter](https://github.com/remorses/playwriter) (MIT; its extension started from Microsoft's Playwright MCP extension, Apache-2.0) was the reference for driving tabs through `chrome.debugger` and keeping them in a group, but its design is the opposite of what we need: it relays the whole DevTools Protocol to a local WebSocket for a Playwright client, so whatever holds the socket can run any script in any connected tab. Ours exposes a dozen fixed operations, runs no code it's sent, and enforces the rules above itself, so the extension was written from scratch.

It lives in this repository rather than one of its own because the extension, the native messaging host and the daemon's tools are one protocol that changes in one pull request, the end-to-end tests need the `cww` binary, and the security model is one document. It has its own build (TypeScript, no runtime dependencies) and its own Web Store release tags (`browser-v*`).
