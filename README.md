# Chat with Work Desktop

Lets [Chat with Work](https://chatwithwork.com)'s chats and bots work on your computer, as you allow: they read, and change files or run commands in folders you allow, each approved. They also scan, print and use your browser. Open source, for Mac, Windows and Linux.

**Download:** [Mac](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg) · [Windows](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-x64.msi) · [Linux](https://github.com/crmne/chatwithwork-desktop/releases/latest) · [every package](docs/install.md)

## What you can do with it

### Scan paper to a bot

Scan a letter, and a bot reads it and files it; the text is read on your computer. [Scanning](docs/scanning.md)

<a href="docs/scanning.md"><img src="https://github.com/user-attachments/assets/7699fd8a-cfd4-494e-ae3e-1a5a1ade5b49" width="760" alt="The scan window with four pages of a synthetic invoice, its menu offering Send to Mail scanner, Send to Bills from the post and Save to Computer; synthetic data"></a>

### Let a bot code with Claude Code

Lend your computer to a coding bot that runs Claude Code, Codex, OpenCode, Gemini CLI or pi with your own sign-in, in one Allow. [Coding bots](docs/coding-computers.md)

<a href="docs/coding-computers.md"><img src="https://github.com/crmne/chatwithwork-desktop/releases/download/v0.15.0/coding-setup.png" width="520" alt="Chat with Work Desktop asking &quot;Let Fixer Claude (Bot 1) code here?&quot; with three checks ticked and Allow; synthetic data"></a>

### Use your own browser

Chat with Work works in your browser, signed in as you, on the sites you allow, and by default asks before each action, with a picture of what it will press. [Your browser](docs/browser.md)

<a href="docs/browser.md"><img src="https://github.com/crmne/chatwithwork-desktop/releases/download/v0.18.0/point-card.png" width="520" alt="An approval card for a click on a seat map, the seat it will click marked on a screenshot of the page; a synthetic test page"></a>

### Work with your files

Share a folder, and chats search and read it; with changes allowed, each change waits for your approval, and what it replaces goes to the trash. [Changing files](docs/file-changes.md)

<a href="docs/file-changes.md"><img src="https://github.com/user-attachments/assets/8ac1b98e-f3c8-440b-9028-a750e2aaadc7" width="560" alt="Shared folders in the app: a read-only Documents folder and a Work projects folder with Allow changes and Run commands on; synthetic data"></a>

### Run commands in a sandbox

Run tests and builds in a folder that allows commands, in a sandbox that changes only that folder, approved in Chat with Work. [Commands](docs/commands.md)

<a href="docs/commands.md"><img src="https://github.com/user-attachments/assets/44e1659f-35fa-4944-b210-73239b0486b7" width="560" alt="A command waiting for approval: ls -la in the shared folder work, network off, with CPU and time limits, Deny and Approve; from a test chat"></a>

### Print from a chat

Print a chat's PDF on a printer you allow, through your system's own printing. [Printing](docs/printing.md)

<a href="docs/printing.md"><img src="https://github.com/crmne/chatwithwork-desktop/releases/download/v0.10.0/printers-light.png" width="760" alt="The Printers page in Settings with one allowed printer; synthetic data"></a>

### Chat with Work in a window, and in your terminal

Chat with Work in a window of its own, and `cww tui` in your terminal: chats, bots and Decisions. [The terminal](docs/terminal.md), [the app](docs/desktop-app.md)

<a href="docs/terminal.md"><img src="https://github.com/crmne/chatwithwork-desktop/releases/download/v0.18.0/terminal-decisions.png" width="760" alt="The terminal app's Decisions page beside its sidebar, with a Slack change and a question waiting; synthetic data"></a>

## Install

| Your computer | Install |
|---|---|
| Mac | [`.dmg`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg), or `brew install --cask crmne/tap/chat-with-work` |
| Windows | [`.msi`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-x64.msi) ([Arm](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Windows-arm64.msi)) |
| Debian, Ubuntu | [`.deb`](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-Linux-x86_64.deb), then `sudo apt install ./Chat-with-Work-Desktop-*.deb` |
| Fedora | `sudo dnf copr enable paolino/chatwithwork-desktop && sudo dnf install chatwithwork-desktop` |
| Arch | `yay -S chatwithwork-desktop-bin` |
| macOS or Linux, one line | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-install.sh \| sh` |

Open **Chat with Work**, sign in, and choose what to share; nothing is shared until you do. openSUSE, Nix, Homebrew, AppImage, Flatpak, pairing, the background agent and updates: [docs/install.md](docs/install.md).

## How it protects your computer

- **Only the folders you share,** and never the secrets inside them (SSH keys, `.env` files, keychains, browser profiles).
- **Every change and command approved in Chat with Work,** unless you chose "Allow everything" for that chat. What a change replaces goes to the trash.
- **Outbound only:** one connection to Chat with Work, no open port, no relay. Every request is in a local log (`cww log -f`).
- **Open source,** so you can check all of this. [Threat model](docs/threat-model.md), [SECURITY.md](SECURITY.md).

## More

[Install and set up](docs/install.md) · [The terminal](docs/terminal.md) · [What the assistant can do](docs/tools.md) · [Configuration](docs/configuration.md) · [Threat model](docs/threat-model.md) · [Protocol](PROTOCOL.md) · [Control socket](CONTROL.md) · [Packaging](PACKAGING.md) · [Development](docs/development.md)

Report a vulnerability: [SECURITY.md](SECURITY.md).

## License

Copyright (c) 2026 Carmine Paolino. [GNU General Public License, version 3 only](LICENSE). "Chat with Work" and its logo are trademarks of PlentyLabs UG (haftungsbeschränkt) & Co. KG; the license covers the code, not the name or the logo in `app/assets`.
