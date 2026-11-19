# Browser scripts (proposed)

A proposal for `browser_script`: a short Playwright script that Chat with Work runs in its own tab group in the person's browser, for a flow of many steps on pages it has already looked at. **Nothing here is built.** This page is the design to review before the code, what it would add to what a compromised server could do, and the spike that comes first.

Single actions stay the way to do one step ([browser.md](browser.md): refs, and since 0.18.0 positions and keys on a screenshot). A script is for what takes a dozen of them: fill a long form from a table, page through a list collecting rows, repeat an edit on every item of a board. It's what [Playwriter](https://github.com/remorses/playwriter) offers agents, without its parts that reach past the tab group.

## What Carmine decided (2026-11-19)

- **Under the driver's Allow everything, a script runs without a card,** as a command does: audited `tool_call.auto_approved` with `via: "mode"`. The organization's opt-in, its cap on modes, and the computer's own "Allow scripts" switch still apply. In every other mode each script asks, showing the whole script, and is never allowed for the rest of a chat.
- **Bots, agents, guests and runs are never offered scripts.**
- **Scripts never change outgoing requests.** Listing the page's requests, read-only, stays.
- **Scripts never upload.** `setInputFiles`, a file chooser, and a drop of files are refused inside a script in every mode, pointing to `browser_upload`, which keeps its own card every time ([browser.md](browser.md), uploads). No file reaches a site from a script.

## Why it needs its own review

The bundled engine ([browser.md](browser.md#bundled-full-engine)) runs the unchanged Playwright that Playwriter uses, but behind a fixed API: click, type, select, on one element the extension already checked, with no way to run code. That promise is what lets the engine drive the person's signed-in browser safely, and a script tool ends it. Everything the single actions enforce in the extension (the tab group, the allowed sites, password and payment fields, files, Stop) has to hold for whatever CDP commands a script's Playwright sends. So the enforcement moves to one place a script can't get around: the extension's relay of CDP for the script's tabs.

## Where a script runs

- A fresh Node process from the bundled engine's verified runtime, started through the **command sandbox** ([commands.md](commands.md)), not the daemon's own confinement, which can read shared folders and reach the server.
- No folders but a private scratch folder, no network, the commands' memory cap, and a clock (30 seconds by default, at most 120).
- Its only way out is its pipes to the daemon, which relays CDP for the script's tabs to the extension, as the engine helper's does today. It has no `require` of Node modules, no `fetch`, no `process`.
- Stopped by Stop in the chat (the commands' `run_stop`), by the extension's Stop and the tray's, by pausing the computer, and by its clock.

## The policy layer on the relay

The extension serves a script's CDP only through a filter written for scripts, and refuses anything it doesn't know:

- **Targets**: only tabs in the "Chat with Work" group, on allowed sites. `Target.*` is answered from the group alone, so `context.pages()` lists only those tabs and `newPage()` opens in the group. `Browser.*`, `SystemInfo`, `Storage` across sites, `Extensions`, service workers, other extensions and `chrome://` pages are refused.
- **Navigation**: every navigation and every new document is checked against the allowed sites as it starts, like the single actions' settling. A tab that leaves them gets nothing more from the script.
- **Files**: `DOM.setFileInputFiles`, `Page.setInterceptFileChooserDialog` and `Input.dispatchDragEvent` with files are refused (decision above).
- **Requests**: `Fetch.*` and the request-modifying parts of `Network.*` are refused. Reading requests and responses the page made stays.
- **Protected fields**: typing, `Input.insertText`, key events and `DOM.setAttributeValue` or `value` writes aimed at password, payment or one-time code fields are refused, judged the way the single actions judge them, and so are reads of their values.
- **Debugger domains**: no `Debugger`, `Profiler`, `HeapProfiler`, `Tracing` or `Emulation` that changes the device. A script gets what Playwright needs for its page API, and nothing else.

What the filter can't see: code the script runs inside an allowed page (`page.evaluate`, `locator.evaluate`) runs as the page does, with the person's sign-in on that site. It can read what the page shows, change it, and send requests the page itself could send. That is why every script outside Allow everything asks with the whole script on the card, and why the card says so in one line.

## The card, the switches, the audit

- **The organization** turns scripts on, off by default, like commands on computers.
- **The person** turns on "Allow scripts" on Desktop's Browser page, on this computer, off by default. Nothing on the server can turn it on.
- **The card** shows the whole script, highlighted, the tabs and sites it acts on, the purpose the model must give, and: "A script can run its own code inside these pages, which the single actions' checks can't see."
- **Effect**: destructive, never allowed for the rest of a chat. After outside text (the taint rule) it asks again, except under Allow everything.
- **Audit**, without contents: the script's length and lines, how many sites, how long it ran, how it ended, how many screenshots it took and the bytes it returned. Never the script, the page, or what it returned.

## Results

- The script's return value as JSON, capped like a tool result; more spills into the chat's files.
- Its console output and the page's errors, with the failing line when it throws.
- Screenshots the script takes come back as pictures the model sees, and are saved to the chat's files.
- Everything that comes back is outside text, fenced like a page's.
- `state` kept between a chat's scripts, in the daemon's memory, values only (no live handles), cleared after 30 idle minutes: a script remembers a tab's id, not the tab.

## What it adds for a compromised server

Today a compromised server could ask for single actions on allowed sites, each checked by the extension, and, where the person turned off asking before browser actions, run them unasked. With scripts it could also, wherever the person allowed scripts and the driver chose Allow everything:

- run code in any allowed site's pages as that site, with the person's sign-in, reading what those pages show and sending what they could send;
- do so many times a minute, within the clock and the hourly limits.

It still couldn't leave the tab group or the allowed sites, read or type into protected fields through the relay, upload a file, change a request, reach the computer's files or network, or turn any of the switches on.

## The spike first

Before any of the above: can the engine's runtime, started under the command sandbox on Linux, drive one tab in the group through the extension, with the clock and Stop ending it at once, and with the filter refusing a `Browser.*` call and a file set? On macOS and Windows the same, once Linux works. Measured: start time, the cost of the filter per command, and whether Playwright's page API works through a filtered target list.

## Open questions

- Which of Playwright's page API works with the filter as written. Locators and waits need `Runtime` and `DOM`; tracing and emulation don't.
- Whether reads of protected fields' values can be refused reliably through `Runtime` results. If not, the card's warning has to say a script could read them.
- Whether a script's clicks should be judged "may submit" one by one, or whether the card covers them (proposed: the card covers them).
