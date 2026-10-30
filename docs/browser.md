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

  Reads run without asking. **Ask before browser actions** is on by default: changes use the approval card, with a checked screenshot where relevant. Turning it off in local Browser settings requires native confirmation and lets permitted actions, including submissions, run without repeated prompts. Sites, protected fields, Stop and actions-off still apply. Every automatic action keeps a dry run and an audit entry. Restoring the switch invalidates a pending automatic action before execution. Bots keep their own approval rules; file and command approvals are unchanged.

## Local CLI clients

Local coding agents can use the same extension through the running Desktop
daemon. This requires the new CLI and daemon together. It does not start a
second daemon, enable browsing, allow sites or turn on actions:

```sh
cww browser tools
cww browser call browser_tabs
printf '%s' '{"tab":123}' | cww browser call browser_read --input -
cww browser call browser_read --input read.json
```

`tools` prints the offered MCP tool schemas. `call` prints an MCP tool result
as JSON, including `structuredContent`, `content` and `isError`; screenshots
are image content blocks with base64 data. A refusal exits nonzero. Input is
a JSON object, at most 12 KiB, read from a file or stdin rather than placing
page text in process arguments. These commands require a running daemon;
they never fall back to direct browser access.

When Ask before browser actions is on, an action first needs a dry run with `"dry_run":true`. Inspect its result
and screenshot, and obtain the person's authorization for that particular
action. Then submit the same arguments with `dry_run` removed and
`--approve`. The flag acknowledges that authorization; it is not an
approval dialog and must never be inferred from a page's text. The daemon
also repeats the dry run immediately before execution and stops if it
fails. Omitting `--approve` refuses every action, even a non-submitting
click, unless the person explicitly turned off Ask before browser actions
locally (`cww browser automatic-actions`; `cww browser ask` restores it).
Automatic actions retain a checked preflight and local audit. Existing user instructions may authorize a specific action; they do
not authorize widening sites or bypassing protected fields.

```sh
cww browser call browser_click --input preview.json
cww browser call browser_click --input approved-action.json --approve
```

The local client offers only the fixed `browser_` tools. It has no evaluate,
raw CDP, filesystem or command endpoint. The same daemon-owned pause state,
site rules, group boundary, field redaction, rate limits and audit log apply.
All local calls share one `local-browser` read budget across processes and
connections; callers cannot select a new chat ID to reset it. A chat-only
organization runtime refuses these requests. Pages remain untrusted data,
and the coding agent must apply its own user-approval and data-sharing rules.
No local stdio MCP server is installed by these commands.

When Desktop restarts, the extension reconnects through the same native host
and sends its browser/version details and current Stop state again. Restarting
the daemon does not resume a browser stopped from the extension.

## Safety

All of it holds on the computer, whatever the server sends. The extension enforces every rule below; the daemon enforces again the ones it can check from its side (the kill switch, the sites on every answer's address and in the tab list, bots, and the budgets), so a fault in one doesn't open those:

- **Sites.** Settings, This computer, Browser offers **Only these sites** (the default, with no access until a site is added) and **All sites except…** (an explicit local choice). The allowed list is kept when changing modes. Blocked sites win in both modes, even over an allowed entry. A bare allowed host (`github.com`) covers it and its subdomains over HTTPS on the default port. A bare blocked host covers HTTP and HTTPS, every port and subdomain, including a trailing-dot spelling. An entry with its scheme (`http://localhost:3000`, `https://example.com`) matches exactly that scheme, host and port. Neither mode permits non-web URLs or URLs containing a username or password. The extension refuses to open, read, screenshot or act on disallowed pages (`site_not_allowed`); after a click or redirect leaves the allowed sites, only the destination host is returned, never its title, path or contents. Back and Close remain available. The daemon checks every returned URL again against the current rules. Selecting All sites or removing a block in the app requires its native confirmation, with account authorization checked before and after. A bot still needs its own browser grant.

  CLI equivalents: `cww browser allow HOST_OR_ORIGIN` adds to the allowed list; `disallow` removes an allowed entry. `allow-all` selects All sites except; `allow-listed` returns to Only these sites. `block HOST_OR_ORIGIN` and `unblock HOST_OR_ORIGIN` edit the blocked list. None of these enables the browser, allows actions or resumes Stop. These are local settings; no server or remote web page can change them.

  Both the daemon and extension must support the new policy. The extension advertises `site_policy_version: 2`; with an older extension, configuring All sites or any blocked entry refuses every browser call until the extension is updated. Existing restricted configurations continue to work.
- **Submitting.** `browser_click` is destructive unless the call says `submits: false`, and the extension refuses such a call (`would_submit`) when the target submits a form, is a submit button, or is named like an action that buys, pays, sends, posts, publishes, deletes, removes, confirms, transfers or signs (in English, German, Italian, French and Spanish). So a click that may send or buy something remains destructive, and asks with a screenshot while Ask before browser actions is on. A plain link or a tab is a write. The name check is a heuristic: a site can still hide an action behind an innocent name, which is why writes ask too unless the person allowed them for the chat or explicitly turned off browser prompts on the computer.
- **Passwords and payments.** The extension never types into, and never reads, password fields, card fields (`autocomplete="cc-*"`, card number, expiry, security code) or one-time codes (`sensitive_field`). `browser_read` leaves their values out, and screenshots blur them: a screenshot is refused (`page_error`) unless every such field it finds is blurred.
- **Outside text.** What a page says is outside text for the taint rule: after a chat reads a page, browser changes ask again unless the owner explicitly turned off Ask before browser actions on this computer. That local choice applies to the owner’s ordinary turn and its delegated sub-agents; independent bots, agent chats and triggered runs keep their own rules. Browser changes never count as "writes to the computer’s own files", since they leave the computer.
- **Downloads** go only into a shared folder that allows changes, through the same writer as file changes (no programs, the system's download mark, the size cap, the change budgets). A file is fetched only from an allowed address, with no redirect followed, not even within an allowed site, so the person's sign-in never goes anywhere unchecked. A download a page starts by itself while the extension acts on its tab (until it detaches, a minute after its last call) is cancelled and removed; the person's own downloads elsewhere are untouched.
- **Visible.** While the agent acts on a tab, the tab shows a "Chat with Work is using this tab" bar, the group's title says so, and the extension's icon has a badge. Chrome also shows its own "started debugging this browser" bar while the debugger is attached; the extension detaches after a minute without a call, so the bar doesn't stay.
- **Kill switch.** "Stop" in the extension's menu, "Stop Using the Browser" in the tray menu (shown while the browser is on), turning off Use your browser in the desktop app’s settings, and `cww browser stop` detach at once and refuse every browser call (`browser_stopped`) until the person resumes. Pausing the computer stops it too.
- **Bots.** A bot's chats reach the browser only when the person shares it on the computer (`cww browser share <bot>`), like a folder, and only the sites allowed there.
- **Audited** in the local audit log: the tool, tab, site (host only), outcome and duration, never page text.

## Background and foreground

A new tab is first grouped in its own normal browser window, then navigated
to the requested address. A focused app or popup never becomes the group's
destination. If grouping fails, only that new blank tab is removed and the
requested page is not loaded.

Tabs in the group can stay in the background while the person works elsewhere. What Chrome does with a background tab:

- **It still loads, runs and answers.** The debugger's commands (reading the accessibility tree, clicking and typing with `Input` events, which are trusted and count as a user gesture) work in a background tab. Screenshots of a background tab work through the debugger. The extension turns on focus emulation, so pages that check `document.hasFocus()` behave as if focused.
- **Timers are throttled**: to once a second when hidden, and chained timers to once a minute after five minutes hidden (intensive throttling). `requestAnimationFrame` doesn't run. Pages that animate or poll can be slow to settle; `browser_wait` gives them time.
- **Tabs aren't discarded**: the extension marks its tabs `autoDiscardable: false`, so Memory Saver keeps them.
- **The person is needed** for what only the person can do in front of the browser: file pickers (the extension never uploads), permission prompts (camera, microphone, notifications, location), passkeys and security keys, the password manager and autofill, payment sheets, print dialogs and extensions' own popups. A JavaScript `alert` or `confirm` in the group is answered by the extension (dismissed) and reported in the result.
- Popups a group tab opens join the group.

## Why its own extension

Ours, open source (in this repository, MIT or Apache-2.0 like the daemon), to be published on the Chrome Web Store as "Chat with Work", never relying on a third-party extension. [Playwriter](https://github.com/remorses/playwriter) (MIT; its extension started from Microsoft's Playwright MCP extension, Apache-2.0) was the reference for driving tabs through `chrome.debugger` and keeping them in a group, but its design is the opposite of what we need: it relays the whole DevTools Protocol to a local WebSocket for a Playwright client, so whatever holds the socket can run any script in any connected tab. Ours exposes a dozen fixed operations, runs no code it's sent, and enforces the rules above itself, so the extension was written from scratch.

It lives in this repository rather than one of its own because the extension, the native messaging host and the daemon's tools are one protocol that changes in one pull request, the end-to-end tests need the `cww` binary, and the security model is one document. It has its own build (TypeScript, no runtime dependencies) and its own Web Store release tags (`browser-v*`).
