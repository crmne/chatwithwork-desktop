<!-- Copy to vX.Y.Z.md, replace every X.Y.Z and vPREVIOUS (the last stable tag), and delete this comment. Match the two previous releases' notes. Pictures and videos aren't release assets: upload each with `gh-attachment FILE --repo crmne/chatwithwork-desktop` before tagging and use the address it prints, or drop the image line. -->
One or two plain sentences on what this release changes for people.

## Download

Pick one file for your computer. The other files on this page are for package managers, the one-line installers and the app's updater.

| Your computer | Download |
|---|---|
| Mac (Apple silicon or Intel), macOS 13.5 or later | [Chat-with-Work-Desktop-X.Y.Z-macOS-universal.dmg](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-macOS-universal.dmg), or `brew install --cask crmne/tap/chat-with-work` |
| Windows | [Chat-with-Work-Desktop-X.Y.Z-Windows-x64.msi](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Windows-x64.msi), on Arm [Chat-with-Work-Desktop-X.Y.Z-Windows-arm64.msi](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Windows-arm64.msi) |
| Debian, Ubuntu | [Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.deb](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.deb), on Arm [Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.deb](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.deb) |
| Fedora | `sudo dnf copr enable paolino/chatwithwork-desktop && sudo dnf install chatwithwork-desktop`, or [Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.rpm](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.rpm), on Arm [Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.rpm](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.rpm) |
| Arch | `yay -S chatwithwork-desktop-bin` |
| Any Linux | [Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.AppImage](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.AppImage), on Arm [Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.AppImage](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/Chat-with-Work-Desktop-X.Y.Z-Linux-aarch64.AppImage), or the Flatpak bundle `Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64.flatpak` below; the agent can't start at login from either yet |
| A server, terminal only | `Chat-with-Work-Desktop-X.Y.Z-Linux-x86_64-terminal-only.tar.gz` (or `-aarch64-`) below |

For a link that always gives the newest version, drop `X.Y.Z-` and use `releases/latest/download/`, for example [https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg](https://github.com/crmne/chatwithwork-desktop/releases/latest/download/Chat-with-Work-Desktop-macOS-universal.dmg).

Already installed? The Mac and Windows apps and the AppImage update themselves; otherwise upgrade through the package manager you used (`brew upgrade`, `apt`, `dnf`, `pacman`). The signed `checksums.txt` lists every file, and GitHub shows each one's SHA-256 beside it; [PACKAGING.md](https://github.com/crmne/chatwithwork-desktop/blob/main/PACKAGING.md#release-assets) says what each one is for.

![What the main change looks like, with synthetic demo content only](https://github.com/user-attachments/assets/...)

## New

- **What people can now do.** How it works, briefly. By @who ([#N](https://github.com/crmne/chatwithwork-desktop/pull/N)).

## Fixed

- **What works now that didn't.** By @who; thanks @reporter ([#N](https://github.com/crmne/chatwithwork-desktop/issues/N)).

## Known limitations

- **What still doesn't work, said plainly.**

## Thanks

- @who for ...

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/vPREVIOUS...vX.Y.Z
