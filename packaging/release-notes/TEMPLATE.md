<!-- Copy to vX.Y.Z.md, replace every X.Y.Z, and delete this comment. Match the two previous releases' notes. -->
One or two plain sentences on what this release changes for people.

## Download

Pick one file for your computer. The other files on this page are for package managers and the one-line installers.

| Your computer | Download |
|---|---|
| Mac (Apple silicon or Intel) | [Chat with Work for Mac](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/chat-with-work-vX.Y.Z-macos-universal.dmg) (`.dmg`), or `brew install --cask crmne/tap/chat-with-work` |
| Windows | [Windows installer](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/cww-vX.Y.Z-x86_64-pc-windows-msvc.msi), [for Arm](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/cww-vX.Y.Z-aarch64-pc-windows-msvc.msi), or `winget install ChatWithWork.Desktop` |
| Debian, Ubuntu | [`.deb` package](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/chatwithwork-desktop_X.Y.Z_amd64.deb), [for Arm](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/chatwithwork-desktop_X.Y.Z_arm64.deb) |
| Fedora, openSUSE | [`.rpm` package](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/chatwithwork-desktop-X.Y.Z-1.x86_64.rpm), [for Arm](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/chatwithwork-desktop-X.Y.Z-1.aarch64.rpm) |
| Arch | `yay -S chatwithwork-desktop-bin` |
| A server, terminal only | `cww-vX.Y.Z-<arch>-unknown-linux-musl.tar.gz` below |

Already installed? Upgrade through the package manager you used (`brew upgrade`, `winget upgrade`, `apt`, `dnf`, `pacman`). GitHub shows each file's SHA-256 beside it, and the signed `checksums.txt` lists all but the Linux packages; [PACKAGING.md](https://github.com/crmne/chatwithwork-desktop/blob/main/PACKAGING.md#release-assets) says what each one is for.

![What the main change looks like, with synthetic demo content only](https://github.com/crmne/chatwithwork-desktop/releases/download/vX.Y.Z/screenshot.png)

## New

- **What people can now do.** How it works, briefly. By @who ([#N](https://github.com/crmne/chatwithwork-desktop/pull/N)).

## Fixed

- **What works now that didn't.** By @who; thanks @reporter ([#N](https://github.com/crmne/chatwithwork-desktop/issues/N)).

## Known limitations

- **What still doesn't work, said plainly.**

## Thanks

- @who for ...

**Full changelog**: https://github.com/crmne/chatwithwork-desktop/compare/vPREVIOUS...vX.Y.Z
