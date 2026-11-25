# Install and set up

How to install Chat with Work Desktop, pair it, share folders and run its background agent. Back to the [README](../README.md).

## Install

The desktop packages include **Chat with Work**, the desktop app, **`cww`**, the terminal interface, and the background agent. Nothing is shared until you pair the computer and choose a folder. Linux and macOS run the daemon as `cww daemon run`; Windows also includes `cww-agent.exe` so the daemon runs without a console window.

These links always download the latest version:

| Your computer | Download |
|---|---|
| Mac (Apple silicon or Intel) | [Chat-with-Work-Desktop-macOS-universal.dmg](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg) |
| Windows | [Chat-with-Work-Desktop-Windows-x64.msi](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-x64.msi), on Arm [Chat-with-Work-Desktop-Windows-arm64.msi](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-arm64.msi) |
| Debian, Ubuntu | [Chat-with-Work-Desktop-Linux-x86_64.deb](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-x86_64.deb), on Arm [Chat-with-Work-Desktop-Linux-aarch64.deb](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-aarch64.deb) |
| Fedora | [Chat-with-Work-Desktop-Linux-x86_64.rpm](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-x86_64.rpm), on Arm [Chat-with-Work-Desktop-Linux-aarch64.rpm](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-aarch64.rpm), or COPR below |
| Any Linux | [Chat-with-Work-Desktop-Linux-x86_64.AppImage](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-x86_64.AppImage), on Arm [Chat-with-Work-Desktop-Linux-aarch64.AppImage](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-aarch64.AppImage) |

Each release also has these files with their version in the name (`Chat-with-Work-Desktop-X.Y.Z-macOS-universal.dmg`), which package managers and the app's updater use. Package managers and the one-line installers:

| Platform | Install | Open |
|---|---|---|
| macOS 13.5 or later | `brew install --cask crmne/tap/chat-with-work`, or download the [`.dmg`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg) or the `.pkg` from the [latest release](https://github.com/crmne/chatwithwork-desktop/releases/latest) | **Chat with Work** in Applications. With the `.dmg`, drag the app to Applications first; its `cww` command is inside `Chat with Work.app/Contents/MacOS/`. |
| Windows 10 and 11 | Download the installer from the [latest release](https://github.com/crmne/chatwithwork-desktop/releases/latest): [`Chat-with-Work-Desktop-Windows-x64.msi`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-x64.msi), or [`Chat-with-Work-Desktop-Windows-arm64.msi`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-arm64.msi) on Arm. Or run the PowerShell one-liner below. WinGet (`ChatWithWork.Desktop`) is [waiting for review](https://github.com/microsoft/winget-pkgs/pull/460095) and isn't available yet. | **Chat with Work** or **Chat with Work (Terminal)** in the Start menu. `cww` is also on your PATH. |
| Arch | `yay -S chatwithwork-desktop-bin` (or `chatwithwork-desktop` to build from source, `chatwithwork-desktop-git` for development builds) | **Chat with Work** in the applications menu, or `cww` in a terminal. |
| Debian, Ubuntu | Download the `.deb`, then `sudo apt install ./Chat-with-Work-Desktop-*.deb` | **Chat with Work** in the applications menu, or `cww`. |
| Fedora | `sudo dnf copr enable paolino/chatwithwork-desktop`, then `sudo dnf install chatwithwork-desktop` ([COPR](https://copr.fedorainfracloud.org/coprs/paolino/chatwithwork-desktop/), updated with `dnf upgrade`), or download the `.rpm` and `sudo dnf install ./Chat-with-Work-Desktop-*.rpm` | **Chat with Work** in the applications menu, or `cww`. |
| openSUSE Tumbleweed, Slowroll, Leap 16.0 | `sudo zypper addrepo https://download.opensuse.org/repositories/home:/plenty/openSUSE_Tumbleweed/home:plenty.repo` (or `.../openSUSE_Slowroll/...`, `.../16.0/...`), then `sudo zypper install chatwithwork-desktop` ([Open Build Service](https://build.opensuse.org/package/show/home:plenty/chatwithwork-desktop), updated with `zypper up`). The release's `.rpm` names Fedora's packages, so zypper can't install it. | **Chat with Work** in the applications menu, or `cww`. |
| Other Linux | The one-line installer below, or the portable app archive. | **Chat with Work** in the applications menu, or `cww`. |
| Any Linux, AppImage | Download `Chat-with-Work-Desktop-Linux-x86_64.AppImage` (or `-aarch64`), make it executable and run it. It uses your system's WebKitGTK 4.1 and GTK 3, and AppIndicator for its tray item when it's installed, like the `.deb`. AppImageUpdate and Gear Lever update it from its `.zsync` file. | The AppImage itself. See the note below about the background agent. |
| Any Linux, Flatpak bundle | Download `Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.flatpak` (or `-aarch64`), then `flatpak install --user ./Chat-with-Work-Desktop-*.flatpak`. It runs on the GNOME 51 runtime from Flathub. | **Chat with Work** in the applications menu. See the note below: the agent needs a terminal there. |
| NixOS, Nix (Linux, Apple silicon macOS) | `nix profile install github:crmne/chatwithwork-desktop` | `cww-app` or `cww`; the package also carries desktop integration. |
| Linux or macOS, Homebrew formula | `brew install crmne/tap/cww` | `cww-app` or `cww`. Add the Homebrew prefix's `share` directory to your desktop session's `XDG_DATA_DIRS` for menu discovery. |

**The Flatpak bundle and the AppImage can't start the background agent at login yet.** In the Flatpak, **Start Background Agent** and `cww daemon install` fail: the sandbox has no access to your systemd user session, and the unit would name a path only the sandbox sees. Run the agent yourself with `flatpak run --command=cww com.chatwithwork.desktop daemon run` (and `flatpak run --command=cww com.chatwithwork.desktop login` to pair); it reads and changes files in the folders you share, but it can't run commands, because Flatpak doesn't allow the bubblewrap sandbox commands run in inside its own. The AppImage's **Start Background Agent** registers the copy of `cww` inside the AppImage's temporary mount under `/tmp`, which the agent's hardened service can't see (it gets a private `/tmp`) and which is gone once the AppImage closes, so the agent can't start from it. Without an AppIndicator library on the system, the AppImage runs without its tray item. Use a package above, or the portable archive, for an agent that runs at every login. The plan is in [#138](https://github.com/crmne/chatwithwork-desktop/issues/138). There's no Snap.

Linux desktop binaries need glibc 2.39 or newer and WebKitGTK 4.1 (Ubuntu 24.04+, Debian 13+, Fedora 40+; Rocky and RHEL 10 have no WebKitGTK 4.1); the packages install the libraries they need. A tray that shows StatusNotifierItems, such as Waybar, KDE, or GNOME with the AppIndicator extension, keeps the app available when its windows close. The separate `Chat-with-Work-Desktop-X.Y.Z-Linux-<arch>-terminal-only.tar.gz` downloads remain available for terminal-only use on older distributions and servers.

**Starting the background agent**

| Installation | Startup |
|---|---|
| Windows MSI / PowerShell one-liner | The installer registers and starts the agent for your account, now and at every login. If that fails, the install still finishes (the one-liner warns): `cww status` says whether it runs, and `cww daemon install` or the app's **Start Background Agent** starts it. |
| macOS `.pkg` | The installer tries to register and start the agent for the logged-in console user. If nobody is logged in, finish setup after login. |
| Linux packages, Nix, Homebrew, portable app | Open the app and choose **Start Background Agent**, or run `cww daemon install` as your own user. System-wide package installation cannot choose which user account should run the agent. |
| macOS / Linux one-line installer | Tries `cww daemon install` for the installing user and prints recovery instructions if no user service manager is available. |

`cww daemon install` starts the agent now, enables it at future logins, and restarts an existing service to use the installed version. If automatic setup failed, run that command without `sudo`. On Linux, check it with `systemctl --user status cww.service`; after a package upgrade, restart it with `systemctl --user restart cww.service`. On any platform, `cww status` reports the agent's state. Without a user service manager, keep `cww daemon run` running in a terminal. Closing the desktop app does not stop the background service.

On Windows, uninstalling (Settings, Apps, or `msiexec /x`) closes the app and removes everything Chat with Work Desktop kept for you: shared folders and settings, the index, the activity log, keys and the app's sign-in, so installing again starts fresh. Elsewhere, `cww daemon uninstall --purge` removes the agent's part (shared folders and settings, the index, the activity log and keys) before you remove the app; the app's sign-in stays in its web view data (`~/.local/share/com.chatwithwork.desktop` on Linux, `~/Library/WebKit/com.chatwithwork.desktop` on macOS) until you delete it.

**Updates**

The Mac app from the `.dmg` and the Windows installer's app update themselves. At startup and once a day the app reads the latest release on GitHub, downloads it in the background, checks `checksums.txt` against its Ed25519 signature with the key built into the app and the download against `checksums.txt`, and installs it the next time the app starts, before any window opens, whatever its major version, when **Install updates automatically** (in **General**, on by default) is on; with the switch off, or for a release no newer than one you went back from, it waits for **Restart to Update** in the banner, the tray and **General**, which also installs one right away. Installing starts the new version, restarts the background agent when it still runs the old one (once no command is running), and puts the previous version back if the new one doesn't start within a minute. Nothing installs at a start while the agent is running a command. **Check Now** checks immediately. The AppImage updates itself the same way, from releases that sign it. A copy a package manager owns (Homebrew, the AUR, COPR, openSUSE's, `.deb` and `.rpm` packages, Nix, the Flatpak) or the Mac's `.pkg` never replaces itself, but still tells you when a new version is out: once the release's signed `checksums.txt` verifies, a banner in the Chat with Work window and **General** show the new version, its release notes, and how to update it the way you installed it: the command (for example `yay -Syu chatwithwork-desktop-bin`) with a Copy button, or for the `.pkg`, a link to the new one, and the tray offers **Update to X.Y.Z…** until you dismiss it. On the Mac and Windows the same banner offers **Restart to Update** once an update is downloaded. The app never runs the command itself. The check sends GitHub the app's name and version in its user agent and nothing else.

**One line, macOS or Linux, no root**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-install.sh | sh
```

It checks release checksums, installs `cww` and `cww-app` into `~/.local/bin`, and registers the desktop launcher and icon on Linux. On macOS the app goes in `~/Applications`. Set `CWW_INSTALL_DIR` to change the command directory. The installer uses `XDG_DATA_HOME` for Linux desktop files and `XDG_CACHE_HOME` for temporary downloads, defaulting to `~/.local/share` and `~/.cache`.

**One line, Windows**

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-install.ps1 | iex"
```

It checks that the background agent started, and warns when it didn't, with the reason the installer gave (over SSH, for example, the agent's logon task can't run without a desktop session).

Windows installers and programs are signed by PlentyLabs. While the signature builds a reputation, SmartScreen may still warn about a downloaded installer: choose **More info**, then **Run anyway**. The one-liner doesn't trigger it. macOS app downloads are signed with Developer ID and notarized; the `.pkg` needs the separate Installer signing identity.

**Portable Linux desktop app**

The `Chat-with-Work-Desktop-X.Y.Z-Linux-<arch>.tar.gz` archive (its folder inside is `cww-app-vX.Y.Z-<arch>-unknown-linux-gnu`) contains both executables, the service, launcher, icon, documentation and licenses. Put `cww-app` and `cww` on your PATH, `cww-app.desktop` in `~/.local/share/applications/`, and `cww-app.svg` in `~/.local/share/icons/hicolor/scalable/apps/`. Then run `cww daemon install`.

The terminal-only `Chat-with-Work-Desktop-X.Y.Z-Linux-<arch>-terminal-only.tar.gz` archive includes `cww` and `cww-notify`. Keep both executables together on your PATH so native notifications can work without the desktop app. Install its `cww-app.desktop` and `cww-app.svg` in the same locations above; this desktop entry is hidden from application menus. A headless server without a desktop notification service retains the browser notification fallback.

**From source** (Rust 1.98 or newer for the desktop app)

```sh
cargo install --locked --path .
cargo install --locked --path app
```

Run these commands from a checkout. On Linux the desktop app also needs CMake, clang, the ALSA and PipeWire development headers, and WebKitGTK 4.1, GTK 3 and AppIndicator (`cmake clang libclang-dev libasound2-dev libpipewire-0.3-dev pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libdbus-1-dev` on Debian and Ubuntu, `webkit2gtk-4.1 gtk3 libayatana-appindicator` besides the audio packages on Arch). They install both commands; the packages or one-line installer also provide desktop integration. The terminal agent alone needs Rust 1.90 or newer.

For a Linux terminal-only build with native notification support, install `notification-broker` instead of `app` in the second command. The notification helper needs Rust 1.98 or newer. On macOS, native notifications require the packaged application bundle, including its identity and signature; loose development binaries are insufficient.

Release downloads come with SHA-256 checksums (`checksums.txt`, signed for the desktop app's updater; GitHub also shows each file's SHA-256 on the release), and the archives, MSIs, `.pkg` and `.dmg` with GitHub artifact attestations. [PACKAGING.md](../PACKAGING.md#release-assets) lists every file. To check a download:

```sh
gh attestation verify Chat-with-Work-Desktop-0.20.0-Linux-x86_64.tar.gz --repo crmne/chatwithwork-desktop
```

[PACKAGING.md](../PACKAGING.md) explains packaging, signing, startup and removal. Open the desktop app or run `cww` to pair and choose folders, or follow the steps below.

## Pairing

```sh
cww login
```

`cww login` creates a key for this computer and prints a short code:

```
To connect "carmine-mbp" to Chat with Work:

  1. Open https://chatwithwork.com/device
  2. Enter the code WDJB-MJHT

Key fingerprint: 3sQ2…
Waiting for approval...
```

Open the link, sign in, check that the computer name matches, and approve. When nothing is shared yet, `cww login` then offers to share your Documents folder, and only does so if you answer yes. Chat with Work emails you whenever a computer is connected. For a self-hosted server, use `cww login --server https://chat.example.com`. `cww` trusts the certificates your operating system trusts, so a server behind a company CA or a local development CA works once that CA is installed; `SSL_CERT_FILE` points it at a different bundle.

The device key never leaves your computer. It is kept in the macOS Keychain, the Windows Credential Manager, or the Secret Service (GNOME Keyring, KWallet), or in a `0600` file on machines without any of them. To disconnect a computer, revoke it under **Settings ▸ Computers**; the daemon notices right away. Run `cww logout` to forget the pairing locally.

## Share folders

```sh
cww roots add ~/Documents/Work --label "Work docs"
cww roots list
cww roots allow-changes work-docs   # let Chat with Work change files there too
cww roots deny-changes work-docs    # read-only again
cww roots read-budget app 5000000   # where [limits] sets an hourly read limit, let a chat read more of a folder
cww roots env app --stdin < app.env  # NAME=value lines every command in the folder gets, kept in the secret store
cww roots remove work-docs
```

Keep a file like `app.env` outside every shared folder, and delete it once `cww roots env` has read it: the command copies it into the secret store and leaves the file where it is, readable by the assistant and the folder's commands if it's in a shared folder. See [docs/secrets.md](secrets.md).

Share narrow folders: anything inside a shared folder can be read by the assistant, except files on the deny list. `cww` refuses to share `/`, your whole home folder, or system directories unless you pass `--i-know`.

A shared folder is read-only until you allow changes in it (and a folder you share read-only stays read-only even inside one that allows changes), with `cww roots allow-changes` (or `--allow-changes` on `add`), the `w` key on the terminal UI's Shared folders page, or the "Allow changes" switch in the desktop app. Then the assistant can create, edit, move and delete files there. Chat with Work asks you before each change, the previous version of anything replaced and everything deleted goes to the system trash (the Trash on macOS, the Recycle Bin on Windows, your desktop's trash on Linux), and the activity log records every change. Nothing on the server can turn changes on. See [docs/file-changes.md](file-changes.md).

Symlinks are not followed. `--follow-symlinks` allows links that stay inside the folder; links that point outside are refused either way.

Reading has no hourly limit: a chat reads as much of your shared folders, and of pages in the browser, as its work needs, a page at a time. If you want a local cap, set `read_chars_per_chat_hour` or `read_chars_per_hour` under `[limits]` in the config file. With one set, a folder you work in a lot, such as code, can have a budget of its own: `cww roots read-budget <root> <characters>`, from that limit to 20,000,000 characters a chat an hour (all chats together twice it), counted for that folder alone; `default` puts it back on the shared limit. With none set, a folder's budget does nothing. Only you can set these, here or over the control API (`roots_read_budget`); the server can't.

## Run the daemon

`cww login` starts the daemon after pairing (pass `--no-daemon` to skip that).

```sh
cww daemon install     # register it again: systemd --user unit on Linux, LaunchAgent on macOS, logon task on Windows
cww status             # connection, roots and index state
cww pause              # refuse every request until...
cww resume
cww reload             # re-read config.toml after editing it
cww log -f             # follow the audit log
cww daemon stop        # stop it until the next logon
cww daemon uninstall   # add --purge to also delete keys, config, index and logs
```

`cww daemon run` runs in the foreground instead. Set `CWW_LOG=debug` for more output. With Homebrew, `brew services start cww` works too. On Windows the logon task runs `cww-agent.exe`, the same daemon without a console window, and it logs to `%LOCALAPPDATA%\cww\state\daemon.log`. If Task Scheduler denies access, setup uses your account's Startup folder instead and starts the agent immediately, without administrator rights. This fallback starts at login but does not restart the agent after a crash; open the app and choose **Start Background Agent** if needed. `cww daemon uninstall` removes either registration.

The CLI, the terminal UI and the settings app talk to the daemon over a local socket (a named pipe on Windows) that only your user can open. [CONTROL.md](../CONTROL.md) documents it for other clients.

## Desktop app

`cww-app` is Chat with Work Desktop: the Chat with Work web app in a window of its own, a menu bar (macOS), notification area (Windows) or tray (Linux) item for the daemon, and the background agent's settings. It is built with [Tauri](https://tauri.app) on the system's web view (WebKitGTK on Linux, WKWebView on macOS, WebView2 on Windows).

- **The Chat with Work window** loads the real web app from the server this computer is paired with (chatwithwork.com, staging, or your own), on `/chats/new`, which opens the organization you used last, the way the phone apps wrap the site. So it is always the web app as it is today. Links that leave the server open in your browser, signing in with Google, Slack or Dropbox and connecting services run in your browser and come back, files dropped on the window go to the composer, downloads go to your Downloads folder, and the microphone works for dictation. It keeps your sign-in, size and position between launches. Its pages can ask the app for very little: open a link in the browser, open the settings, wait for a sign-in in the browser, scan for the composer, and show and change this computer's shared folders in Chat with Work's own Settings ▸ This computer and on the new chat page ([docs/desktop-app.md](desktop-app.md)). What a computer shares is still decided on it: a page can make it share less at once, but anything that shares more waits for your yes in a dialog the app draws, which no page can answer, and folders are added only through the system's folder picker. Pages never see where a folder is on the computer, the activity log or the deny list.
- **Links** to chats, Decisions, bots and projects open in this window when a browser on this computer is set to send them here, through `chatwithwork://`, which the packages register. A link only moves the window ([docs/desktop-app.md](desktop-app.md)).
- **This computer in Settings** contains Shared folders, Browser, General, Notifications, Account and Activity in the same window. Its pages are bundled with the app and run in a separate native child view: local paths, excluded files and activity never enter the website’s DOM. Shared folders includes the local deny list and native folder picker; bot sharing is selected from the bot’s settings. The bundled navigation also works offline and before pairing.
- **The tray item** says whether Chat with Work can reach this computer (and, while there is more to say, what it's indexing or last read), and opens Chat with Work, opens the scan window, pauses and resumes sharing, opens the settings, and quits.
- **The scan window** scans paper with a scanner attached to the computer (through SANE on Linux and macOS, and the scanner's TWAIN driver on Windows), leaves out blank pages, lets you crop, turn, order and delete pages, and saves one searchable PDF on your computer (no account needed), puts it in Chat with Work's own composer (a new chat's, or the one whose attach menu asked for it, where you write your message with everything the composer offers), or sends it to a bot such as the Mail scanner recipe. The text is read on your computer (Tesseract on Linux, Vision on macOS, Windows' own OCR on Windows), and scans stay on your computer until you send them ([docs/scanning.md](scanning.md)).
- **Printers** you allow here print PDFs from a chat, through CUPS or the Windows spooler. You approve each job in Chat with Work, unless you let one bot print on one printer unattended, here, on this computer ([docs/printing.md](printing.md)).

On first run it opens the welcome page: it starts the agent, pairs, offers to share your Documents folder, and asks whether to send [crash reports](configuration.md#crash-reports), and it shares nothing until you say so. Closing a window keeps the tray item and the agent running, and frees the window's web view.

```sh
cargo run --release -p cww-app
```

Install it as described under [Install](#install). On Linux it needs WebKitGTK 4.1, GTK 3 and the AppIndicator library, which the packages install; scanning also needs SANE and Tesseract, which the packages recommend; the scan window adds the languages you read itself (Add a language…). On NVIDIA's driver under Wayland it sets `__NV_DISABLE_EXPLICIT_SYNC=1` for itself, without which WebKitGTK can't draw there, and everywhere it sets `WEBKIT_USE_SKIA_FOR_COMPOSITION=0`, since WebKitGTK 2.54's new compositor flickers at the end of animations (set it to `1` to keep Skia).

Through version 0.3.1, the desktop app was a native egui app with its own chat window. It is kept at the [`egui-final`](https://github.com/crmne/chatwithwork-desktop/tree/egui-final) tag, for anyone who wants to revive a fully native app.
