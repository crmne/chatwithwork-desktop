# Using the browser

Status: **built: reading and acting, the desktop app's settings page and tray switch, and the server side; tested on Linux with Chromium. The extension is on the [Chrome Web Store](https://chromewebstore.google.com/detail/ehjfihpogoacjodjnanclodinjnnpmcp), where the desktop app's Get extension button leads.** Chat with Work can use the person's own Chromium-family browser (Chrome, Chromium, Edge, Brave, Vivaldi), with their sign-ins, through the Chat with Work browser extension and Chat with Work Desktop. The extension's source is [`browser/`](../browser); the wire details are in [PROTOCOL.md](../PROTOCOL.md) and [CONTROL.md](../CONTROL.md).

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
  | `browser_read` | read | A tab's accessible text (roles, names, values, `[ref=…]` for what can be clicked or typed in, including `clickable` elements a page made clickable without a role or name, `[to=…]` for where a link to an allowed site goes), never raw HTML; paged, at most 32,000 characters a page, counted against any hourly read limit set |
  | `browser_screenshot` | read | A JPEG of the tab, cut to fit a tool result |
  | `browser_scroll` | read | Scroll a tab, or bring a `ref` into view |
  | `browser_wait` | read | Wait up to 15 seconds for text to appear, or for the page to settle |
  | `browser_hover` | read | Point at a spot of the latest screenshot, or turn the wheel there |
  | `browser_open` | write | Open an allowed URL in a new tab in the group, or in one of its tabs |
  | `browser_back` | write | Go back in a tab |
  | `browser_select` | write | Choose options in a `<select>` |
  | `browser_type` | write; destructive with `submit: true` | Type into a field, and press Enter with `submit` |
  | `browser_click` | destructive; write with `submits: false` | Click a `ref` |
  | `browser_close` | write | Close a tab in the group |
  | `browser_download` | write | Save a link's file into a shared folder that allows changes |
  | `browser_upload` | destructive, always asks | Hand files from a shared folder to a page's upload field, or drop them on a drop zone |
  | `browser_click_at` | destructive; write with `submits: false` | Click (or double-click) a spot of the latest screenshot |
  | `browser_drag` | destructive; write with `submits: false` | Drag from one spot of the latest screenshot to another |
  | `browser_press` | destructive; write with `submits: false` | Press one key (Tab, Enter, Escape, Space, arrows, Home, End, Page Up and Down, Backspace, Delete, with Shift) in what has focus |

  The reads are offered while the browser is on; the rest only while the person also allows actions on the computer (`cww browser allow-changes`), like a folder's "Allow changes", `browser_download` only while a folder allows changes, and `browser_upload` while a folder is shared. Each action takes `dry_run`, which checks it and describes it without doing it.

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

When Ask before browser actions is on, an action first needs a dry run with `"dry_run":true`. An upload's approved call also passes the dry run's `fingerprint` as `expected_sha256`, so it uploads only the files the person saw. Inspect its result
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
site rules, group boundary, field redaction, any rate limits set, and audit log
apply. All local calls share one `local-browser` read budget across processes
and connections, when one is set; callers cannot select a new chat ID to reset
it. A chat-only
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
- **Submitting.** `browser_click` is destructive unless the call says `submits: false`, and the extension refuses such a call (`would_submit`) when the target submits a form, is a submit button, or is named like an action that buys, pays, sends, posts, publishes, deletes, removes, confirms, transfers or signs (in English, German, Italian, French and Spanish), or is a control the page drew without a role whose name doesn't say what it does (a bare `×` or an icon). So a click that may send or buy something remains destructive, and asks with a screenshot while Ask before browser actions is on. A plain link or a tab is a write. The name check is a heuristic: a site can still hide an action behind an innocent name, which is why writes ask too unless the person allowed them for the chat or explicitly turned off browser prompts on the computer.
- **Links.** `browser_read` shows where a link goes only as far as `browser_open` could go anyway: for a link to a site allowed here, its path after its ref (`- link "Style" [ref=e12] [to=/admin/theme/style]`, with the origin in front when it's another allowed site than the page's), and nothing for a link anywhere else, not even its host. Never its query, fragment or a username and password, so reset and sign-in links, signed downloads and tracking parameters stay on the computer, and links that aren't web addresses (`javascript:`, `mailto:`, `tel:`, `data:`) show nothing. A path segment that looks like a token shows as `…`: a UUID (anywhere in the segment, with or without hyphens), 16 or more letters and digits mixed (or hex), base64 (with `+` or `=`, or a `/` encoded as `%2F`), base64url (20 or more letters and digits mixing upper and lower case, even split by `-` or `_`), a JWT's parts, 24 or more letters in mixed case, any 32-character run, or a number longer than 19 digits. Words, hyphenated slugs, dates and record numbers stay. Each segment between literal `/`s is judged on its own, so a token written across several short segments still shows. The rule is a heuristic: a short token in a path still shows, and some harmless ids (a commit's hash, a UUID that's only a record's id) show as `…`, which a click still follows. Characters that would read as markup (`[`, `]`, quotes, spaces) are percent-encoded, then the path is cut at 200 characters. It is a preview: `browser_open` takes a full address, so a bare path is opened with the tab's origin in front, and one with `…` (masked or cut short) leads somewhere else, so it is followed only by clicking its ref. Links come from the page's own document and its shadow roots, resolved against its `<base>`, not from frames.
- **Passwords and payments.** The extension never types into, and never reads, password fields, card fields (`autocomplete="cc-*"`, card number, expiry, security code) or one-time codes (`sensitive_field`). `browser_read` leaves their values out, and screenshots blur them: a screenshot is refused (`page_error`) unless every such field it finds is blurred. Each screenshot marks its own blur, so the late cleanup of one whose tab stalled never takes off the blur a later screenshot still needs.
- **Outside text.** What a page says is outside text for the taint rule: after a chat reads a page, browser changes ask again unless the owner explicitly turned off Ask before browser actions on this computer. That local choice applies to the owner’s ordinary turn and its delegated sub-agents; independent bots, agent chats and triggered runs keep their own rules. Browser changes never count as "writes to the computer’s own files", since they leave the computer.
- **Downloads** go only into a shared folder that allows changes, through the same writer as file changes (no programs, the system's download mark, the size cap, any change limits set). A file is fetched only from an allowed address, with no redirect followed, not even within an allowed site, so the person's sign-in never goes anywhere unchecked. A download a page starts by itself while the extension acts on its tab (until it detaches, a minute after its last call) is cancelled and removed; the person's own downloads elsewhere are untouched.
- **Uploads** come only from shared folders, each one asked for: `browser_upload` is destructive with no exception, so it always asks, is never allowed for the rest of a chat, and never runs under Ask before browser actions off or a permission mode on the server. The card names each file (name, size, kind, the folder it's in), the site and the field. The daemon resolves each path like a read (the folder's symlink rule, the deny list, no hard links, only a regular file, up to 200 MB), takes where the open file really is, checks it's inside the folder and that the path still names that same file, and hashes it, copying exactly those bytes into a private, read-only file under the approved name (in cww's own state folder, kept a day, since a page reads a chosen file only when it sends it); the browser gets only that copy's path, never the shared file or a path the page or the model gave. The approval is bound to each file's path, size, modification time and SHA-256, and a file that changed after it was approved isn't uploaded (`conflict`). The extension never opens a file, and reads back only the names and sizes the field holds, to check it got what was approved. It refuses a file the field's `accept` doesn't allow, several for a field that takes one, password, card and one-time code fields, and a button whose press may send something. Only a file in a shared folder can be uploaded. A file made in a chat (by code, an attachment) or brought into it from another computer (that computer's `copy_to_chat`) is saved here first with `copy_from_chat` (PROTOCOL.md 8.7, a change with a card of its own, into a folder that allows changes), then uploaded from that path: two steps, two cards. The local audit records the upload's site and outcome, never file names or paths.

  The browser hands files to pages for an extension only while its **Allow access to file URLs** switch is on: in `chrome://extensions`, open Chat with Work's **Details** and turn it on (off for a Chrome Web Store install until the person does; unpacked builds start with it on). Without it uploads fail with `file_access_off` and those steps, the extension's menu says so with a button to the Details page, and so does Desktop's Browser page. Turning it on reloads the extension, which reconnects by itself.

  How a field is found: `browser_read` marks every `<input type=file>`, in shadow roots and same-process frames too, hidden ones included, with `[upload]` and its `accept` and `multiple`; the labels and buttons that go with one (a label for it, or a button within three levels of a hidden one) carry the same mark; buttons named for uploading (in English, German, Italian, French and Spanish) carry `[upload]` though their field doesn't exist yet; and an element that says to drag and drop files on it reads as `drop zone` with `[upload drop]`. A field, or a label or button with a known field, gets the files set on the field itself, without a click. A button whose field the page makes only when pressed is pressed once, with the browser's file chooser intercepted for that one press, and the files go to the field that asked (the person's own pickers in the tab are never intercepted). With `drop: true`, the files are dropped on the element, as a drag from the computer would. Cross-origin frames aren't reached.

- **Visible.** While the agent acts on a tab, the tab shows a "Chat with Work is using this tab" bar, the group's title says so, and the extension's icon has a badge. Chrome also shows its own "started debugging this browser" bar while the debugger is attached; the extension detaches after a minute without a call, so the bar doesn't stay.
- **Kill switch.** "Stop" in the extension's menu, "Stop Using the Browser" in the tray menu (shown while the browser is on), turning off Use your browser in the desktop app’s settings, and `cww browser stop` detach at once and refuse every browser call (`browser_stopped`) until the person resumes. Pausing the computer stops it too.
- **Bots.** A bot's chats reach the browser only when the person shares it on the computer (`cww browser share <bot>`), like a folder, and only the sites allowed there.
- **Audited** in the local audit log: the tool, tab, site (host only), outcome and duration, never page text.

## By position and keyboard

Most pages give `browser_read` an element to name for everything that matters, and refs stay the way to act there. Some don't: a canvas editor, a drawn calendar or grid, a custom slider, a board whose cards move by dragging. `browser_read` says so ("this page has … with no element references"), and the model then works from a picture:

- `browser_screenshot` (from extension 0.1.5) answers with the picture and a `capture` id, which records where the page stood when it was taken: its address and document, its scroll, its size and its zoom.
- `browser_click_at`, `browser_drag` and `browser_hover` take a point of that picture, in its pixels, with its `capture`. The extension refuses the call when the page navigated, scrolled, resized or zoomed since ("the page changed, look again"), maps the point to the page (device pixel ratio and browser zoom included), and checks what is under it with the same rules as a ref's element: password, payment and one-time code fields, file fields (that's `browser_upload`), download links (`browser_download`), links to sites that aren't allowed, and frames are refused, and something drawn over the point is reported, never clicked through.
- What a click may do is read from the element under the point, as for a ref: a submit button, a form's last field, or a name like Send or Pay may submit, and so does anything the page draws itself (a canvas, a custom element with no role), since nothing tells what it does. `submits: false` is refused there (`would_submit`). The approval card shows the picture with the point marked; a drag shows both ends, joined.
- The approved call acts only on what the card showed: the same element under the point (a drag's two ends, the focused element for a key). If the page put something else there while the person looked, nothing happens and the model is told to look again. A drag presses only if its start still holds that element once the pointer is over it (a hover can change a page), and lets go at its end only over the checked target or the dragged element itself; otherwise it lets go where it started.
- `browser_press` presses one key from a fixed list in whatever has focus, and answers what has focus after, so Tab moves through a page that gives no refs. Enter or Space in a form field, on a button or on a drawn control may submit, Delete or Backspace anywhere but a text field may delete what's selected, and any key but Tab may do anything while nothing has focus; a sensitive field takes only Tab and Escape, and focus inside an embedded frame takes no keys. `browser_type` without a `ref` types into what has focus, with the same refusals as a ref's field.
- Every action answers with a fresh `look` (its own picture and `capture`), so the model always points at the page as it is now. One action per call: a sequence is several calls, each checked.

## Background and foreground

A new tab is first grouped in its own normal browser window, then navigated
to the requested address. A focused app or popup never becomes the group's
destination. If grouping fails, only that new blank tab is removed and the
requested page is not loaded.

Tabs in the group can stay in the background while the person works elsewhere. What Chrome does with a background tab:

- **It still loads, runs and answers.** The debugger's commands (reading the accessibility tree, clicking and typing with `Input` events, which are trusted and count as a user gesture) work in a background tab. Screenshots of a background tab work through the debugger. The extension turns on focus emulation, so pages that check `document.hasFocus()` behave as if focused.
- **Timers are throttled**: to once a second when hidden, and chained timers to once a minute after five minutes hidden (intensive throttling). `requestAnimationFrame` doesn't run. Pages that animate or poll can be slow to settle; `browser_wait` gives them time.
- **Tabs aren't discarded**: the extension marks its tabs `autoDiscardable: false`, so Memory Saver keeps them.
- **The person is needed** for what only the person can do in front of the browser: a site's File System Access picker (`showOpenFilePicker`, which no extension can answer; ordinary upload fields and drop zones are `browser_upload`'s), permission prompts (camera, microphone, notifications, location), passkeys and security keys, the password manager and autofill, payment sheets, print dialogs and extensions' own popups. A JavaScript `alert` or `confirm` in the group is answered by the extension (dismissed) and reported in the result.
- Popups a group tab opens join the group.
- **A page that stops responding** (a script stuck in a loop, often right after a click) fails the call that hit it with a clear `page_error` within the debugger's 15-second command limit, saying that what was already sent may or may not have taken effect. Later calls to that tab then fail at once for a few seconds, and after that each starts with a two-second check, so they answer well inside Chat with Work's wait instead of timing out one command at a time. Other tabs keep working, and `browser_close` closes the stuck tab. A cleanup that couldn't reach the stuck page, such as taking a screenshot's blur off its fields, runs again by itself once the page answers (checked every two seconds for two minutes); one tied to the attachment it was made in, such as a click guard, is dropped once that attachment ends, so it never attaches again after Stop. An attachment that completes after Stop detaches at once, whether or not a call still waits for it.

Typing with `clear` (the default) into a rich-text editor focuses it, selects its contents, deletes them, then types, as a person replacing the text would. Typing over the selection instead gave the new text the old text's formatting, so replacing a bold line came out bold. Fields and text areas, which have no formatting, are filled as before. Line breaks in the text are typed as line breaks, never as Enter, which some editors take as Send.

Some pages draw a control as a bare element with a click handler, such as a
`×` that removes a row or closes a banner, with no role, name or tabindex. The
reader finds these in a snapshot of the page's own document (a click handler
the browser knows of, `onclick`, `tabindex`, or the start of a `cursor: pointer`
region; not controls, what's inside them, or wrappers around them) and shows
each as `- clickable "×" [ref=e52] [near="Row 3"]`: its text, title or label as
its name, and, when that is only a glyph or nothing, the text just before it.
Clicking one goes through every check any click does. Since its role says
nothing, it is judged as a button: an action name makes it `would_submit`, and
so does a name that is only a glyph, so it asks with a screenshot.

Click checks adapt Playwright's actionability machinery from Playwriter's
engine. They clip content quads to the visible viewport and walk the document
and each shadow root, including closed roots, slotted labels and
`display: contents`. They wait up to four seconds for the control to become
visible, enabled and stable, trying different scroll alignments (the middle
of the window second) for sticky headers and fixed banners. Geometry is
sampled with bounded timers because background tabs can suspend animation
frames. Previews perform these checks too; a blocked control fails with
"Nothing was clicked" and the obstruction's tag and short ARIA label, plus the
fixed or sticky layer it belongs to and where that sits ("a fixed `<div>`
along the bottom of the window"), never its markup, text or field values.
Subframe clicks fail closed rather than mixing document coordinates.

After hovering, the extension rechecks the hit target and installs a trusted
pointer-event guard against a layout change at dispatch. It never force-clicks
through an overlay, never retries after sending a press, and cleans up the
guard even after Stop. Site access and the local action setting are checked
again while waiting. A custom element's host alone is neither an obstruction
nor permission to click: the painted point must reach the intended control.
The guard remains bound to the document that received the click. If a completed
click replaces that document, losing its isolated context is expected; the
result describes the navigation without repeating input or inspecting a blocked
destination. Other debugger failures and surviving guard refusals remain errors.

Styled native radios and checkboxes may use a visible associated label as
their activation surface, including when CSS hides the input. The browser's
own `label.control` association must name that exact input, both in the
preview and throughout the trusted click. An unrelated or nested label,
other interactive content, a real overlay or a disabled control still refuses
the action. A shadow host painted inside an alternate label also refuses,
including closed roots that can hide a different control; directly targeted
shadow controls keep their normal hit checks.
The preview outlines the visible label and describes the original control;
it never selects the option. Other fields are not retargeted through labels.

## Bundled full engine

Supported packages also bundle the unchanged Playwriter Playwright engine
behind click, type and select. The daemon verifies a private Node runtime and
module tree, then uses the full upstream action pipeline through the existing
extension. No separate runtime installation is required. The fixed tools,
local permissions, protected fields, own tab group, Stop and approval checks
remain the same. This does not expose Playwriter's arbitrary-code executor or
a remote CDP service.

Route selection happens before an operation. Older extension capabilities and
platforms without a supported bundled runtime retain the released implementation
described above; a failed or possibly dispatched full-engine action never falls
back to another implementation. Preview input is suppressed in transport,
including hover, while upstream actionability checks still run. Background
operations do not activate the tab or change the page clock.

Before the engine starts a click, the extension runs the overlay part of the
checks above for up to four seconds: it scrolls the control, or one of its
labels, until a click at its point would reach it, clear of fixed and sticky
layers. Where something else is drawn over the control's middle but part of
it shows (a fixed bar's own handle across its links), the click goes to a
visible point of the control instead, found by the same hit test, as a person
would click it; the engine presses that point. One still covered everywhere is
refused then, before any input, with the same
certain "Nothing was clicked" error naming what covers it. The engine's own
retries would only have ended in the uncertain "did not finish" answer. A
control that is hidden, disabled or moving is left to the engine's own waits.

See the [helper contract and test coverage](../browser/engine/README.md) for
reference binding, cancellation, runtime support and complete license provenance.

The installed private directory contains `current.json` and an immutable payload
named by the SHA-256 of its manifest. The daemon verifies the manifest and every
file before confinement, then keeps that exact payload path until restart.
An upgrade cannot redirect an in-flight operation into a new module tree. A
portable runtime inside a writable shared folder is not admitted; ordinary
browser tools keep the legacy path. Sharing its location with write access later
restarts the daemon so the same admission rule applies.

At daemon startup, one confined helper imports its modules and reports ready,
without an extension lease or browser input. Preparation has a 45-second bound
for first-launch OS inspection. Until it succeeds, admission selects legacy.
Once ready, each tool request has a shared 18-second limit, including any
automatic preview, action and cleanup; each action is at most 14 seconds. A
subsequent slow helper start cannot send input after that request expires.
Reference admission consumes the same deadline. Cleanup never reattaches after
Stop, and terminal success waits for already-sent engine cleanup acknowledgements.

The helper uses inherited pipes, a fixed executable and entry, and a sanitized
environment. There is no PATH lookup, runtime download, public evaluation API or
new network endpoint. Its raw protocol, page events and upstream errors never
become tool results. Events and commands are bound to the current native
connection and operation lease; Stop, policy changes and disconnect cancel
the operation.

When the engine can't finish, the answer says what is known, from what the
extension itself recorded and checked (`engine_cancel`'s `outcome`, extension
0.1.6 and later), never the engine's own text:

- No input left (the extension marks the operation before its first input
  event goes to the page): "Nothing was clicked", with the extension's reason
  (the element didn't become ready in time because it was disabled, hidden or
  covered, the page changed first, the browser was stopped, changes were turned
  off, or the site was no longer allowed), and that
  nothing was sent.
- Only the pointer moved: the same, saying the pointer moved over the element
  but nothing was pressed or typed.
- Input left: that the click, text or choice was sent (or only part of it: the
  press without its release, or typing whose text isn't confirmed), then what the page looks
  like afterwards: same address and document, reloaded, gone to another page,
  closed, or not responding, whether a dialog opened (the extension dismisses
  every page dialog), and whether the element is still there.
- An older extension, or one that couldn't look: the old answer, that the
  result is uncertain.

Every answer asks the assistant to read the page before deciding what to do
next, never to repeat a possibly completed action blindly.

## Why its own extension

Ours, open source in this repository, is intended for the Chrome Web Store as
"Chat with Work". [Playwriter](https://github.com/remorses/playwriter) is the
reference for driving tabs through `chrome.debugger`, and its Playwright
engine supplies both the legacy adapted click algorithms and the bundled full action pipeline. Those portions retain their
Microsoft/Puppeteer Apache-2.0 attribution; both extension ZIPs include the
license and [third-party notices](../browser/static/THIRD-PARTY-NOTICES.txt).
Playwriter itself is MIT licensed. We reuse the click mechanics without its
general DevTools relay: Chat with Work exposes fixed operations, runs no code
it is sent, and enforces its own tab, site, field and approval rules.

It lives in this repository rather than one of its own because the extension, the native messaging host and the daemon's tools are one protocol that changes in one pull request, the end-to-end tests need the `cww` binary, and the security model is one document. It has its own build (TypeScript, no runtime dependencies) and its own Web Store release tags (`browser-v*`).
