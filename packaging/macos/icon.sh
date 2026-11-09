#!/bin/bash
# Put the app icon into a bundle's Contents/Resources.
#
#   packaging/macos/icon.sh <Contents/Resources>
#
# actool (Xcode 26 or later) compiles the Icon Composer document into
# Assets.car, which macOS 26 and later read through CFBundleIconName, so
# the system draws the tile and its dark, clear and tinted looks.
# AppIcon.icns, named by CFBundleIconFile, is what older macOS shows. Both
# come from app/assets/macos/icon.py.
set -euo pipefail

resources="$1"
here="$(cd "$(dirname "$0")" && pwd)"
source="$here/../../app/assets/macos"
mkdir -p "$resources"

partial="$(mktemp -d)"
xcrun actool "$source/AppIcon.icon" --compile "$resources" \
    --platform macosx --minimum-deployment-target 13.5 \
    --app-icon AppIcon --output-partial-info-plist "$partial/icon.plist" \
    --errors --warnings --output-format human-readable-text
rm -rf "$partial"
# actool writes an .icns of its own with nothing above 256 pixels; the
# committed one has every size up to 1024.
install -m 644 "$source/AppIcon.icns" "$resources/AppIcon.icns"

if ! assetutil --info "$resources/Assets.car" | grep -q '"Name" : "AppIcon"'; then
    echo "Assets.car has no AppIcon" >&2
    exit 1
fi
