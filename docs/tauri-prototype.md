# Tauri prototype: is it fast enough to replace egui?

> The prototype measured here (a Tauri copy of the egui screens, in `desktop-tauri/`) was replaced by the app in [desktop-app.md](desktop-app.md), which takes this page's suggestion: a window on the server's real pages instead of a second copy of them. Its code is kept at the [`tauri-prototype-final`](https://github.com/crmne/chatwithwork-desktop/tree/tauri-prototype-final) tag.

A quick prototype, not a port, to see whether Chat with Work Desktop could move from egui to Tauri 2 and reuse web UI. It lives in [`desktop-tauri/`](../desktop-tauri), outside the Cargo workspace with its own lock file, so nothing in the daemon or the egui app changes except a measuring hook (below).

## What it does

- **The same daemon, the same socket.** It speaks the control channel ([CONTROL.md](../CONTROL.md)) directly over the Unix socket: `status`, `chats`, `chat`, `chat_send`, `chat_approve`, `chat_deny`, `roots_writable`, plus `subscribe` to `chat`, `chats` and `status`. The window can only send those requests. A real port would link `cww::control::Client` and `cww::paths` instead, which also cover Windows' named pipe.
- **Three screens:** the Folders page (what the egui app opens on once paired), the chat list, and a chat's transcript with its streaming answer, the activity line, citation chips and approval cards (Approve, Deny, and "Allow for the rest of this chat").
- **Live Wire.** `ui/tokens.css` is the web app's `app/assets/tailwind/base/tokens.css` copied as is, with Geist and Geist Mono. The components are plain CSS after the web's (no Tailwind or daisyUI), no framework and no build step.
- **Rust does the parsing.** Answers' Markdown is rendered with pulldown-cmark in Rust (raw HTML shown as text), and each streamed chunk arrives in the page as the answer's HTML so far. Logos and file icons come through the daemon's `asset` request on a `cwwasset://` URI scheme.

Try it against the egui app's demo daemon:

```sh
cargo build --release -p cww-app --features demo
(cd desktop-tauri && cargo build --release)
target/release/cww-app --demo --background &   # the stand-in daemon, in the tray
CWW_HOME=/tmp/cww-demo-<its pid>-0 desktop-tauri/target/release/cww-desktop-tauri
```

`CWW_OPEN_CHAT=12` opens the streaming chat, `5` the one waiting for approval. On NVIDIA with Hyprland, add `__NV_DISABLE_EXPLICIT_SYNC=1` (see Linux below).

## How it was measured

[`desktop-tauri/bench/bench.py`](../desktop-tauri/bench/bench.py) runs both apps against the egui app's `--demo` daemon, so they show the same chats. With `CWW_PAINT_MARK` set, each prints the time of its first frame with the daemon's data (the Folders page, or an open chat's transcript); the egui app's hook is in `SettingsApp::mark_painted`. It reports:

- **Start:** launch to that first frame, warm, and on Linux cold too (every file the app's processes mapped evicted from the page cache with `posix_fadvise`; pages other programs map, like libc and the GPU driver, stay).
- **Memory:** every process the app runs, after 10 s idle. Linux: RSS and PSS (PSS splits shared libraries fairly). macOS: RSS and the physical footprint Activity Monitor shows.
- **CPU:** idle for 10 s, and for 5 s while chat 12's answer streams.
- **Tauri in-page checks:** scroll a 27,000 px transcript top to bottom in 3 s, type 60 characters into the composer, approve a change. They use the page's own events, not the system's input, so they measure the page's work (script, layout, paint) and not the compositor's input path.

On Linux the windows run on Hyprland's hidden special workspace, so nothing opens where anyone works. A hidden window gets no frames, so there both apps mark the frame they lay out with the data rather than the one the compositor shows. An earlier run with visible windows (7 runs) is reported beside it; its Tauri mark waited two animation frames. Streaming CPU means nothing in a hidden window (nothing is drawn), so that number comes from the visible run. macOS ran with visible windows on the Mac mini (10 runs).

Both builds are release builds with the same profile (thin LTO, one codegen unit, stripped). The egui app was built with the `demo` feature.

## Results

### Linux: Omarchy (Hyprland 0.56), NVIDIA RTX 3090, WebKitGTK 2.54.1

| | egui | Tauri |
|---|---|---|
| Start to first frame with data, warm (hidden, n=3) | **98 ms** | 376 ms |
| Start, cold (hidden, n=3) | **188 ms** | 572 ms |
| Start, warm, visible window (n=7) | **170 ms** (158 to 237) | 1,261 ms (436 to 1,407) |
| Start, cold, visible window (n=7) | **262 ms** | 564 ms |
| Idle memory, RSS of all processes | **109 MiB** | 528 MiB |
| Idle memory, PSS of all processes | **74 MiB** | 287 MiB |
| PSS after streaming a chat | **87 MiB** | 321 MiB |
| Idle CPU | 0.0 to 0.1% | 0.1% (18.6% with one pulsing dot on screen) |
| CPU while an answer streams (visible, 5 s) | **0.62 s, 12% of a core** | 0.85 s, 17% of a core |
| Binary, stripped (gzip) | 19.3 MB (8.4 MB) | **6.8 MB (2.5 MB)** |
| Package | 18.5 MB `.deb` (with `cww`) | **2.5 MB `.deb`** (app only) |
| System libraries it needs | GL, PipeWire, ALSA | WebKitGTK 4.1 (141 MiB installed) and libsoup 3 |

Tauri's memory is three processes: the app (205 MiB RSS, 107 MiB PSS), `WebKitWebProcess` (219 / 131) and `WebKitNetworkProcess` (104 / 50), even though the page makes no network requests.

In the page: scrolling held one frame per refresh (181 frames in 3 s, median 17.0 ms, worst 18.0 ms, none over 25 ms), typing showed each character in the next frame (median 16 ms, worst 18 ms), and an approval card turned decided 17 ms after the click.

### macOS: Mac mini, Apple silicon, macOS 27.0.1 (WKWebView)

| | egui | Tauri |
|---|---|---|
| Start to first frame with data, warm (n=10) | **131 ms** (101 to 152) | 330 ms (304 to 1,128, the first launch) |
| Start on a chat to its transcript | **197 ms** | 405 ms |
| Idle memory, RSS of all processes | **150 MiB** | 191 MiB |
| Idle memory, footprint | **120 MiB** | 144 MiB |
| Footprint after streaming a chat | **135 MiB** | 223 MiB |
| Idle CPU | **0.1%** | 0.6% |
| CPU while an answer streams (5 s) | 1.04 s, 20% | 1.04 s, 20% |
| Binary (arm64) | 18.3 MB | **6.1 MB** |
| App bundle | 30 MB universal `.dmg` (with `cww`) | **6.0 MB `.app`** (arm64) |

Tauri's processes are the app (36 MiB footprint) and WebKit's GPU, WebContent and Networking services (15, 87 and 6 MiB). Scrolling: median 17.0 ms per frame, 2 of 180 frames over 25 ms (worst 34 ms). Typing: median 17 ms to the next frame.

Not measured: Windows (WebView2 is a separate engine with its own memory and start costs), Linux distributions other than Arch, and anything with a real paired account.

## How it feels

Once running, the page is smooth on both platforms: scrolling and typing stay within one frame, and the streaming answer reflows without hitches. The difference shows at the edges. The window takes a third of a second (Linux, warm) to half a second (cold) before its first frame with data, against a tenth of a second for egui, and a visible first launch on Linux took up to 1.4 s. I couldn't judge input latency through the compositor, since that means driving the input of a desktop someone is using; try both with the commands above.

Text, selection, scrolling physics, emoji, IME and right to left text come for free in the web view, and they look like a browser's rather than the platform's. The styles were quick: the web's tokens worked unchanged, and the approval card is the web's CSS nearly line for line.

## What surprised

- **WebKitGTK crashed on start** on this machine (Wayland protocol error 71 under NVIDIA's explicit sync on Hyprland) until `__NV_DISABLE_EXPLICIT_SYNC=1` was set. `WEBKIT_DISABLE_DMABUF_RENDERER=1` is the other known workaround, at a cost in rendering speed. Tauri apps ship one of these on Linux, and it's the kind of report that would come from users.
- **One CSS animation costs a fifth of a core.** A pulsing dot for a chat being answered kept WebKitGTK compositing every frame: 18.6% of a core while idle. egui draws it still. Every `infinite` animation in the web's components (the thinking ring, the flowing wire, the composer's rainbow) has this cost while it shows, so a port would keep them to moments that are live.
- **Linux carries the web engine twice over:** a 141 MiB dependency the egui packages don't need, and three processes for one window.

## Recommendation

Keep egui. Tauri is fast enough frame to frame, but not cheap enough at rest: on Linux it uses about four times the memory (287 against 74 MiB PSS) and starts three to four times slower, it adds WebKitGTK as a dependency, and it crashed on this NVIDIA desktop without a workaround. For an app that stays open in the tray all day, the memory matters more than how it scrolls. On macOS the gap is small (24 MiB more at rest, 200 ms slower to start, the same CPU while streaming), so the cost of Tauri is mostly a Linux cost.

What would change this:

- If reusing the web app's views is the goal, the bigger win is a window on the server's real pages rather than a second copy of them in Tauri: that is what would remove the parallel UI, and it needs sign-in and session work, not a Tauri port of the egui screens.
- A Tauri app could drop its web view while the window is closed and keep only the tray, which would bring idle memory with the window closed down near egui's. The open-window numbers above would stay.
- Measure Windows and a non-NVIDIA Linux before deciding either way for those platforms.
