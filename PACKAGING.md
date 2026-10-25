# Packaging

Every way to install Chat with Work Desktop comes from one release: a `v*` tag on a commit already on `main`.

Every package installs the desktop app, terminal interface and daemon together. Portable Linux musl archives are the explicit terminal-only download for servers and systems without glibc.

| Platform | What users install | Built by |
|---|---|---|
| macOS | App disk image (`.dmg`), `.pkg` installer, Homebrew cask or formula, or one-line installer. The cask depends on the formula to expose `cww` on PATH without conflicting with existing formula installs. | `release.yml`, job `macos` |
| Windows | Per-user MSI, directly or through WinGet or the PowerShell installer, with `cww.exe`, `cww-app.exe`, `cww-agent.exe` and the scanner helper. | `release.yml`, job `build` |
| Linux | `.deb`, `.rpm`, all three AUR variants, Homebrew, Nix, the one-line installer, or the complete desktop archive. | `release.yml` (jobs `build` and `linux-app`) and `packaging.yml` |

The Linux archive `cww-app-vX.Y.Z-<arch>-unknown-linux-gnu.tar.gz` is the single input for native packages and binary AUR recipes. `packaging/linux/archive.sh` creates it with both executables, the systemd unit, launcher, icon, docs and licenses. `cww` itself remains static (musl); the GUI requires glibc 2.39 or newer, WebKitGTK 4.1 and GTK 3 for its windows (Tauri), the AppIndicator library for its tray item (loaded at run time), D-Bus, and ALSA (`libasound.so.2`) and PipeWire's client library (`libpipewire-0.3.so.0`) for the terminal's dictation helper, which records through PipeWire, then PulseAudio, then ALSA. Linux source builds need those development packages, clang and pkg-config (`libasound2-dev libpipewire-0.3-dev clang libclang-dev pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libdbus-1-dev` on Debian and Ubuntu, `alsa-lib libpipewire clang pkgconf webkit2gtk-4.1 gtk3 libayatana-appindicator` on Arch); the terminal-only musl binary has no audio or graphics library dependency. Release and packaging-test workflows build the GUI on Ubuntu 24.04: older releases ship a PipeWire too old for the bindings fastframe-audio builds. ELF inspection adds libc and libgcc dependencies; `native-packages.yaml` and the AUR recipes also declare WebKitGTK, GTK and the AppIndicator library loaded at run time. On Windows the window needs the WebView2 runtime, which Windows 11 and current Windows 10 include; the WinGet manifest depends on `Microsoft.EdgeWebView2Runtime` for the rest. Debian 12's glibc 2.36 and Rocky/RHEL 9's 2.34 are too old for this GUI, and Rocky/RHEL 10 ship no WebKitGTK 4.1, so the desktop package installs on neither; the terminal-only archive works there.

Each app download carries `cww` beside the GUI (inside the bundle on macOS), so the app starts the matching daemon. Every file `release.yml` publishes is covered by `checksums.txt` and its signature; GitHub shows each file's SHA-256 on the release too, the `.deb` and `.rpm` files' included. The macOS app is signed and stapled before it is copied into the `.pkg` and the formula/shell-installer archive.

## Release assets

A release carries only what people download or something reads; anything else stays in the workflow's artifacts. Before adding a file, add it here with what reads it. People-facing files come first (GitHub lists assets by name, so the release notes' Download section, from [`packaging/release-notes/TEMPLATE.md`](packaging/release-notes/TEMPLATE.md), says which one to pick):

| File | Who it's for |
|---|---|
| `chat-with-work-vX.Y.Z-macos-universal.dmg` | Mac users: drag the app to Applications. The Homebrew cask installs it too. |
| `cww-vX.Y.Z-macos-universal.pkg` | Mac users and admins who want `cww` on the PATH and the agent started by the installer. |
| `cww-vX.Y.Z-x86_64-pc-windows-msvc.msi` | Windows users (x64). WinGet, `cww-installer.ps1` and `test-windows-installer.yml` use it too. |
| `cww-vX.Y.Z-aarch64-pc-windows-msvc.msi` | Windows on Arm users. |
| `chatwithwork-desktop_X.Y.Z_amd64.deb`, `_arm64.deb` | Debian and Ubuntu users. Attached by `packaging.yml`. |
| `chatwithwork-desktop-X.Y.Z-1.x86_64.rpm`, `.aarch64.rpm` | Fedora and openSUSE users. Attached by `packaging.yml`. |
| `cww-installer.sh` | The README's one-line installer for macOS and Linux, at `releases/latest/download/`. |
| `cww-installer.ps1` | The README's one-line installer for Windows without WinGet. |
| `cww-vX.Y.Z-<arch>-unknown-linux-musl.tar.gz` | Terminal-only `cww` and `cww-notify`, for servers and distributions older than glibc 2.39. |
| `cww-app-vX.Y.Z-<arch>-unknown-linux-gnu.tar.gz` | The portable Linux app and `cww`. The input for the `.deb` and `.rpm` builds; the Homebrew formula on Linux, AUR `-bin` and `cww-installer.sh` download it. |
| `cww-vX.Y.Z-macos-universal.tar.gz` | The app and a signed `cww`, for the Homebrew formula and `cww-installer.sh`. |
| `checksums.txt` | SHA-256 of every file above but the `.deb` and `.rpm` files (GitHub shows theirs on the release). Both installers and native-packages check downloads against it. |
| `checksums.txt.sig` | Ed25519 signature of `checksums.txt` with the key in `app/assets/update-public-key.hex`, kept for the desktop app's updater, which isn't built yet. |

native-packages also attaches its rendered recipes (`chatwithwork-desktop-X.Y.Z-packaging.tar.xz`, `packaging-checksums.txt`); `packaging.yml`'s `tidy-release` job removes them, since the AUR and Homebrew are published from the run's own copy and `winget-submit.sh` renders WinGet's manifests itself. Releases up to 0.5.0 also carried a `.zip` of the macOS app and portable Windows zips; nothing read them, so they're no longer built. Scoop's recipe went with the zips: no bucket was ever published.

## Files

| Path | Role |
|---|---|
| [`native-packages.yaml`](native-packages.yaml) | The packaging configuration: `.deb`/`.rpm` contents, release assets, recipe templates, and the AUR and Homebrew destinations. |
| `.github/workflows/release.yml` | Builds, signs and packages every platform on a tag, then publishes the GitHub release. Run it by hand to try a change without publishing. |
| `.github/workflows/packaging.yml` | After a release: `.deb`, `.rpm`, AUR, Homebrew and WinGet recipes. On pushes and PRs that touch packaging: the same packages from the commit, installed in clean containers. |
| `packaging/systemd/cww.service` | The systemd user unit shipped by the Linux packages. |
| `packaging/linux/postinstall.sh` | Tells `.deb`/`.rpm` users how to start. Starts nothing: the daemon runs per user. |
| `packaging/arch/*/PKGBUILD.in`, `cww.install` | AUR recipes; `build-local.sh` builds the source recipe from the working tree. |
| `packaging/homebrew/cww.rb.in` | The formula, with a `brew services` definition. |
| `packaging/homebrew/chat-with-work.rb.in` | The desktop app's cask, from the notarized `.dmg`. |
| `packaging/macos/` | `bundle.sh` (the desktop app's bundle), `pkg.sh` (the installer package), its `postinstall`, `distribution.xml` and pages, and `import-installer-identity.sh` for CI. |
| `packaging/linux/cww-app.desktop` | Applications-menu entry, installed with the `cww-app.svg` icon by every Linux package. |
| `packaging/linux/archive.sh` | Assembles the complete Linux package payload, shared by release and packaging-test builds. |
| `packaging/test-packaging.py` | Installer, archive and AUR package-layout regression tests, isolated from the user's account. |
| `.github/workflows/desktop-preview.yml` | By hand: the desktop app for macOS and Windows as a `desktop-preview-<run>` prerelease, to try a change between releases. |
| `packaging/windows/` | `cww.wxs` (WiX 5 MSI), `build-msi.ps1`, and `sign.ps1` for certificate signing. |
| `packaging/winget/` | WinGet manifest templates. |
| `packaging/install.sh`, `install.ps1` | The one-line installers, attached to each release as `cww-installer.sh` and `cww-installer.ps1`. |
| `packaging/test-install.sh` | Installs, runs and removes Debian, RPM and Arch packages in clean containers. |
| `packaging/release-notes/vX.Y.Z.md` | Required for a stable tag. Start from `TEMPLATE.md`, whose Download section names the one file to pick per platform. |
| `flake.nix` | The Nix package (Linux and macOS) and a dev shell. |

## Crash reports

The background agent's opt-in crash reports (README, "Crash reports") go to the `cww-desktop` project on our Bugsink server. Its address is compiled in from `src/crash_reports.rs`. Set `CWW_CRASH_REPORTS_DSN` at build time to send them somewhere else, or to an empty value to build a version that can't send them and never asks. Release builds need nothing set.

## Releasing

1. Bump `version` in `Cargo.toml` and `app/Cargo.toml` (they move together) and run `mbx build` so `Cargo.lock` follows.
2. Write `packaging/release-notes/vX.Y.Z.md`, starting from `TEMPLATE.md`.
3. Push `main`, wait for CI, then tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.

`release.yml` checks the tag matches `Cargo.toml` and is on `main`, builds every target and the desktop app, signs what it has credentials for (the app bundle and its disk image are notarized and stapled too), attests the archives (`gh attestation verify <file> --repo crmne/chatwithwork-desktop`), and publishes the release with `checksums.txt` and the installers (see [Release assets](#release-assets)). Tags with a `-` (`v1.2.0-beta.1`) become draft prereleases and skip the package managers.

For a stable tag, `packaging.yml` then runs the shared [native-packages](https://github.com/crmne/native-packages/tree/v0.10.0) workflow: it downloads the Linux archives, verifies them against `checksums.txt`, builds and attaches the `.deb` and `.rpm` files, renders every recipe with the published checksums, and pushes the AUR and Homebrew ones when enabled. It attaches the rendered recipes too, as `chatwithwork-desktop-X.Y.Z-packaging.tar.xz`; `tidy-release` takes them off the release again, and they stay in the run's `native-packages` artifact.

### Trying changes before a tag

- `gh workflow run release.yml` builds and signs everything, uploading the files as workflow artifacts without publishing.
- Any push or PR that touches packaging runs `packaging.yml` against the commit: static daemon and glibc GUI builds, `.deb`/`.rpm`, all recipes (with stand-ins for the macOS and Windows assets), and install tests on Ubuntu 24.04, Debian 13, and Fedora 41 and latest, on amd64 and arm64.
- Locally, with `gem install native-packages --version 0.10.0`, nFPM 2.47.0 and `bsdtar`:

  ```sh
  python3 packaging/test-packaging.py
  native-packages validate
  # dist/ needs release-shaped inputs; see the "inputs" job in packaging.yml
  native-packages build --version 0.3.0 --target linux-amd64 --output dist/np
  bash packaging/test-install.sh ubuntu:24.04 dist/np
  packaging/arch/build-local.sh                 # makepkg from the working tree
  nix build .#default
  ```

## Signing

The MSI, WinGet manifests and Linux package metadata display **Plenty** as the
publisher or vendor. Signing identities retain the company's full name as
issued on the certificates.

A complete set of secrets turns each kind of signing on. With none, the release is built unsigned; an incomplete set fails the build.

### macOS

The same secrets as ZapFast and TonePush. `native-packages notarize-macos` signs the universal binary with the hardened runtime and a timestamp, and notarizes it; the Homebrew formula uses that archive. The same Developer ID Application identity signs the desktop app (`bundle.sh` signs the `cww` inside it first, then the bundle), and the app and its `.dmg` are each notarized and stapled, so both open offline without warnings. A bare binary can't carry a stapled ticket, so Gatekeeper checks it online the first time.

- `APPLE_CERTIFICATE_P12`: base64 PKCS#12 Developer ID **Application** certificate and private key.
- `APPLE_CERTIFICATE_PASSWORD`: its export password.
- `APPLE_SIGNING_IDENTITY`: `Developer ID Application: PlentyLabs UG (haftungsbeschrankt) & Co. KG (JPL7999US3)`.
- `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: Apple ID email, Team ID and an app-specific password, for notarization.

The `.pkg` needs a Developer ID **Installer** certificate as well, which a Developer ID Application certificate doesn't cover. With it, `pkg.sh` signs the package, notarizes it and staples the ticket, so it installs offline too. Without it the `.pkg` is built unsigned, which Gatekeeper refuses to open with a double-click; the Homebrew formula and the one-line installer don't need it.

#### Getting the Developer ID Installer certificate

Only the Apple Developer account holder can create Developer ID certificates.

1. On a Mac, open Keychain Access → Certificate Assistant → *Request a Certificate From a Certificate Authority*. Enter the account holder's email, choose *Saved to disk*, and save the `.certSigningRequest`.
2. At [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/add), choose **Developer ID Installer**, keep the *G2 Sub-CA* profile, upload the request and download the `.cer`.
3. Double-click the `.cer` so Keychain Access pairs it with the private key from step 1. It appears under *My Certificates* as `Developer ID Installer: PlentyLabs UG (haftungsbeschrankt) & Co. KG (JPL7999US3)`.
4. Right-click that certificate → *Export*, save as `.p12` with a strong password. Keep the `.p12` and its password in the 1Password item "Apple Notarization", next to the Application certificate.
5. Set the secrets:

   ```sh
   base64 -i developer-id-installer.p12 | gh secret set APPLE_INSTALLER_CERTIFICATE_P12
   gh secret set APPLE_INSTALLER_CERTIFICATE_PASSWORD   # paste the export password
   gh secret set APPLE_INSTALLER_SIGNING_IDENTITY --body "Developer ID Installer: PlentyLabs UG (haftungsbeschrankt) & Co. KG (JPL7999US3)"
   ```

6. Run `gh workflow run release.yml` and check that the `macos` job signs, notarizes and staples the `.pkg`. `pkgutil --check-signature` and `spctl -a -vv -t install` on the downloaded artifact should both report the Developer ID Installer identity.

The notarization secrets (`APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`) are shared with the binary and need no change.

### Windows

Unsigned Windows binaries that register a logon task are what Microsoft Defender's behaviour models look for, and SmartScreen warns about any unsigned MSI or program a browser downloaded. `cww.exe`, `cww-agent.exe`, `cww-app.exe` and the MSI are all signed with SHA-256 and an RFC 3161 timestamp once one of these is set up. Sign releases before publishing them to WinGet.

#### What SmartScreen needs

- **A signature.** Unsigned downloads always get the full-screen warning.
- **Reputation for the publisher.** A signature alone doesn't silence SmartScreen: since 2024 no certificate type, EV included, is trusted instantly. Reputation builds as people download and run releases signed by the same publisher identity, so keep signing every release with the same identity and don't rotate it needlessly.
- The warning only applies to files with the Mark of the Web, meaning downloaded by a browser. `winget install` and the PowerShell one-liner don't trigger it; Defender still scans them.

#### Option A: Azure Artifact Signing (formerly Trusted Signing)

About $10 a month, keys in Microsoft's HSMs, and `release.yml` already supports it. Eligibility for Public Trust:

- Organizations in the EU (and the US, Canada, UK and others) with **at least three years of verifiable tax history**. PlentyLabs qualifies only if it has that history.
- Individuals only in the US and Canada.

Steps:

1. In the Azure portal, create an *Artifact Signing account* in West Europe (endpoint `https://weu.codesigning.azure.net/`).
2. Under *Identity validation*, start a **Public Trust** validation for the organization. Microsoft checks the registration and tax records, and one person completes an ID check with AU10TIX. It usually takes a few days.
3. Create a *certificate profile* of type **Public Trust** using the validated identity.
4. Create an app registration (Entra ID) with a client secret, and give it the **Artifact Signing Certificate Profile Signer** role on the account.
5. Set the secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.
6. Run `gh workflow run release.yml` and check the Windows jobs report signed files (`Get-AuthenticodeSignature` on the MSI shows `Valid`).

#### Option B: a certificate from a CA with cloud signing

If the organization doesn't qualify for Option A, buy an **OV** code signing certificate for the organization (or for yourself as an individual) from a CA that offers cloud signing usable in CI, such as SSL.com (eSigner) or Certum (SimplySign). Since June 2023, publicly trusted code signing keys must live in a hardware or cloud HSM, so a new certificate can't be exported as a `.pfx`. The `WINDOWS_CERTIFICATE_PFX` path in `sign.ps1` only works for older exportable certificates. Using a cloud-signing CA needs a small change to `packaging/windows/sign.ps1` to call the CA's signing tool instead. EV costs more and no longer buys instant SmartScreen trust, so OV is enough.

#### Option C: the Microsoft Store

Individual developer accounts are free. Microsoft signs Store packages itself, so they never hit SmartScreen, and `winget install` can install from the Store. It needs an MSIX package with a full-trust startup task for the agent, which `cww.wxs` doesn't produce yet.

## Package managers

| Destination | How it's published | Needs |
|---|---|---|
| GitHub release (`.deb`, `.rpm`) | Automatic | Nothing |
| Homebrew, `crmne/homebrew-tap` `Formula/cww.rb` and `Casks/chat-with-work.rb` | Automatic when `PUBLISH_HOMEBREW=true` | `HOMEBREW_TAP_SSH_KEY` (a deploy key with write access to the tap only) or `HOMEBREW_TAP_GITHUB_TOKEN` |
| AUR `chatwithwork-desktop`, `-bin`, `-git` | Automatic when `PUBLISH_AUR=true` | `AUR_SSH_KEY`, `AUR_KNOWN_HOSTS`, and the three AUR packages registered to that key (the publish step clones them, so a new package needs a first push by hand) |
| WinGet, `ChatWithWork.Desktop` | By hand, below | A fork of `microsoft/winget-pkgs` |
| Nix | `nix profile install github:crmne/chatwithwork-desktop` works from the repository; nixpkgs is a separate submission | - |

`PUBLISH_AUR` and `PUBLISH_HOMEBREW` are repository **variables**; the rest are secrets.

**The desktop app** is included in every package recipe alongside the terminal agent. Updating the existing package upgrades users to the complete installation; there is no separate Linux GUI package to discover.

**WinGet.** `packaging/windows/winget-submit.sh X.Y.Z` renders the manifests from the tag's `packaging/winget/*.yaml.in` with the MSIs' SHA-256s from the release's `checksums.txt`, checks that every manifest names `ChatWithWork.Desktop` and the version and that each installer URL matches its hash, and prepares a branch in `crmne/winget-pkgs` with `manifests/c/ChatWithWork/Desktop/X.Y.Z/`. Validate it on Windows (`winget validate --manifest <dir>` and `winget install --manifest <dir>`), then run it again with `--open` to push and open the pull request. The manifests use the MSI's fixed UpgradeCode, so upgrades replace the previous version, 0.3 installs included.

## Existing releases

Starting with v0.3.0, packages include the desktop app. The published v0.2.0 Linux packages and MSI still contain only the terminal agent; its desktop archives are separate downloads. Do not republish old desktop archives in place: the new Linux package recipes need the service and documentation included by `packaging/linux/archive.sh`.

## Startup and removal

- **Windows MSI:** per-user install to `%LOCALAPPDATA%\Programs\Chat with Work Desktop`, on PATH, with separate **Chat with Work** and **Chat with Work (Terminal)** Start menu entries. It runs `cww daemon install`, registering and starting the logon task with `cww-agent.exe`, or a per-user Startup shortcut if Task Scheduler is unavailable. Neither path requires elevation. CI exercises installation, immediate startup, the fallback login shortcut, repeated startup and removal under a fresh standard account while a protected task with the same name exists. Uninstall runs `cww daemon uninstall` and keeps keys and settings. Preserve the UpgradeCode `{FBC69B33-161E-40DC-9D83-5B54DB1A9821}` so upgrades replace earlier versions. WinGet and PowerShell use this MSI.
- **Windows runtime:** the Windows binaries link the C runtime statically (`.cargo/config.toml`), since a fresh Windows install has no Visual C++ Redistributable. CI and the release check every Windows executable with `packaging/windows/check-runtime.ps1`, because GitHub's runners have the Redistributable installed and the installer tests there wouldn't notice.
- **macOS `.pkg`:** installs `/Applications/Chat with Work.app`, `/usr/local/bin/cww` and the `cww-app` command. It tries to register and start the LaunchAgent for the console user. The completion page explains `cww daemon install` when no user session is available or startup fails. To remove it, run `cww daemon uninstall` before deleting the app and commands.
- **Linux distro packages:** install both commands, the launcher and icon, and `/usr/lib/systemd/user/cww.service`. The root package transaction does not select a user account or start an agent as root. The install and upgrade messages tell each user to choose **Start Background Agent** in the app, or run `cww daemon install` without sudo. This enables and starts the packaged service. Check with `systemctl --user status cww.service`; restart after upgrading with `systemctl --user restart cww.service`. Before removing the package, run `cww daemon uninstall` as each user who enabled it.
- **Homebrew:** both the formula and cask carry the GUI and terminal tools. The cask registers the app in Applications; the formula keeps its macOS bundle under the formula prefix. Run `cww daemon install`, or choose **Start Background Agent** in the app. If using `brew services start cww` instead, stop that service before switching to the app-managed service to avoid running two daemons.
- **Nix:** builds the whole workspace, includes Linux desktop integration and the service, and wraps the GUI with its runtime graphics libraries. On Apple silicon macOS it carries an app bundle under `$out/Applications`. The pinned Nixpkgs no longer supports Intel macOS; use the universal native installer or Homebrew there. `nix profile install` does not enable services or register macOS Applications aliases; run `cww daemon install` and use your NixOS/Home Manager or nix-darwin configuration for declarative desktop integration.
- **One-line macOS/Linux installer:** verifies checksums, installs both commands plus desktop integration, and attempts `cww daemon install` for the invoking user. If startup is unavailable it leaves the installation usable and prints the recovery command. It never registers a daemon for root. Uninstall with `cww daemon uninstall`, then remove the installed commands, launcher/icon or app bundle.

The daemon stays idle until pairing, and nothing is shared until the user chooses a folder. `cww status` checks it on every platform. Without a supported user service manager, `cww daemon run` runs in the foreground. Starting the app and enabling its tray icon at login are separate from enabling the background daemon.
