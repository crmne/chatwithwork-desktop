#!/bin/bash
# Build "Chat with Work.app" on macOS from release binaries.
#
#   packaging/macos/bundle.sh <cww-app> <cww> <output.app> <version> <browser-engine>
#
# The app and the cww command sit side by side in Contents/MacOS, so the app
# finds cww there to start the agent, and `cww daemon install` registers
# the bundle's copy, which updates with the app.
#
# Set CODESIGN_IDENTITY to sign with a Developer ID (hardened runtime, for
# notarization). Otherwise the bundle gets the ad-hoc signature arm64 needs.
set -euo pipefail

app_binary="$1"
cww_binary="$2"
app="$3"
version="$4"
engine="${5:?staged browser engine}"
numeric_version="${version%%-*}"
here="$(cd "$(dirname "$0")" && pwd)"
python3 "$here/../../browser/engine/scripts/package.py" validate "$engine"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 755 "$app_binary" "$app/Contents/MacOS/cww-app"
install -m 755 "$cww_binary" "$app/Contents/MacOS/cww"
mkdir -p "$app/Contents/Resources"
cp -R "$engine" "$app/Contents/Resources/browser-engine"
sed "s/__VERSION__/$numeric_version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
# Start at login runs this agent, which opens the app with --background (a
# login item cannot pass arguments). Named after CFBundleIdentifier.
mkdir -p "$app/Contents/Library/LaunchAgents"
install -m 644 "$here/autostart.plist" "$app/Contents/Library/LaunchAgents/com.chatwithwork.desktop.autostart.plist"

"$here/icon.sh" "$app/Contents/Resources"

# Replace upstream Node's entitlements: no debugger attachment, DYLD override
# or unsigned-library exception. V8 needs only JIT. Rehash before sealing.
runtime="$(python3 "$here/../../browser/engine/scripts/package.py" path "$app/Contents/Resources/browser-engine")/node"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --options runtime --entitlements "$here/browser-engine.entitlements.plist" --sign "$CODESIGN_IDENTITY" "$runtime"
else
    codesign --force --options runtime --entitlements "$here/browser-engine.entitlements.plist" --sign - "$runtime"
fi
python3 "$here/../../browser/engine/scripts/package.py" refresh "$app/Contents/Resources/browser-engine"

# Sign the inner command first, then the bundle.
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --options runtime --sign "$CODESIGN_IDENTITY" "$app/Contents/MacOS/cww"
    codesign --force --timestamp --options runtime --entitlements "$here/entitlements.plist" --sign "$CODESIGN_IDENTITY" "$app"
else
    codesign --force --sign - "$app/Contents/MacOS/cww"
    codesign --force --entitlements "$here/entitlements.plist" --sign - "$app"
fi
codesign --verify --strict --deep "$app"
plutil -lint "$app/Contents/Info.plist" >/dev/null

echo "$app"
