#!/usr/bin/env bash
# Gives a release's files the names people read (PACKAGING.md, "Release
# assets"). The builds keep their build names (cww-v…, cww-app-v…,
# chat-with-work-v…, and native-packages' chatwithwork-desktop…); this
# renames each one found in <directory> to Chat-with-Work-Desktop-<version>-
# <system>.<extension>.
#
#   packaging/release-names.sh <directory> <version> [--stable] [--legacy]
#
# --stable also copies the downloads people pick to a name without the
# version, so releases/latest/download/<name> always gives the latest one.
# --legacy keeps a copy under the old name where an installed app (0.19.0
# and older) looks for it: its updater, its update notice's links and
# commands, and AppImageUpdate. Releases carry both until the step that
# drops the old names (PACKAGING.md, "Renaming the downloads").
set -euo pipefail

usage() { echo "usage: $0 <directory> <version> [--stable] [--legacy]" >&2; exit 2; }
[ $# -ge 2 ] || usage
directory=$1
version=${2#v}
shift 2
stable=false
legacy=false
for option in "$@"; do
  case "$option" in
    --stable) stable=true ;;
    --legacy) legacy=true ;;
    *) usage ;;
  esac
done
# X.Y.Z, or a prerelease's X.Y.Z-beta.1.
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]] || { echo "not a version: $version" >&2; exit 2; }
[ -d "$directory" ] || { echo "no directory $directory" >&2; exit 2; }

name=Chat-with-Work-Desktop
v=$version
# build name | new name | copy without the version | old name kept
table="
chat-with-work-v$v-macos-universal.dmg|$name-$v-macOS-universal.dmg|$name-macOS-universal.dmg|yes
cww-v$v-macos-universal.pkg|$name-$v-macOS-universal.pkg||yes
cww-v$v-x86_64-pc-windows-msvc.msi|$name-$v-Windows-x64.msi|$name-Windows-x64.msi|yes
cww-v$v-aarch64-pc-windows-msvc.msi|$name-$v-Windows-arm64.msi|$name-Windows-arm64.msi|yes
chatwithwork-desktop-$v-x86_64.AppImage|$name-$v-Linux-x86_64.AppImage|$name-Linux-x86_64.AppImage|yes
chatwithwork-desktop-$v-aarch64.AppImage|$name-$v-Linux-aarch64.AppImage|$name-Linux-aarch64.AppImage|yes
chatwithwork-desktop_${v}_amd64.deb|$name-$v-Linux-x86_64.deb|$name-Linux-x86_64.deb|yes
chatwithwork-desktop_${v}_arm64.deb|$name-$v-Linux-aarch64.deb|$name-Linux-aarch64.deb|yes
chatwithwork-desktop-$v-1.x86_64.rpm|$name-$v-Linux-x86_64.rpm|$name-Linux-x86_64.rpm|yes
chatwithwork-desktop-$v-1.aarch64.rpm|$name-$v-Linux-aarch64.rpm|$name-Linux-aarch64.rpm|yes
chatwithwork-desktop-$v-x86_64.flatpak|$name-$v-Linux-x86_64.flatpak||yes
chatwithwork-desktop-$v-aarch64.flatpak|$name-$v-Linux-aarch64.flatpak||yes
cww-installer.sh|$name-install.sh||yes
cww-installer.ps1|$name-install.ps1||yes
cww-v$v-macos-universal.tar.gz|$name-$v-macOS-universal.tar.gz||no
cww-app-v$v-x86_64-unknown-linux-gnu.tar.gz|$name-$v-Linux-x86_64.tar.gz||yes
cww-app-v$v-aarch64-unknown-linux-gnu.tar.gz|$name-$v-Linux-aarch64.tar.gz||yes
cww-v$v-x86_64-unknown-linux-musl.tar.gz|$name-$v-Linux-x86_64-terminal-only.tar.gz||no
cww-v$v-aarch64-unknown-linux-musl.tar.gz|$name-$v-Linux-aarch64-terminal-only.tar.gz||no
"

while IFS='|' read -r old new copy keep; do
  [ -n "$old" ] || continue
  [ -f "$directory/$old" ] || continue
  cp -p "$directory/$old" "$directory/$new"
  if $stable && [ -n "$copy" ]; then
    cp -p "$directory/$old" "$directory/$copy"
  fi
  if ! $legacy || [ "$keep" != yes ]; then
    rm "$directory/$old"
  fi
  echo "$old -> $new${copy:+ (and $copy)}"
done <<<"$table"
