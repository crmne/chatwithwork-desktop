#!/usr/bin/env bash
# Render the Chocolatey package for a published release into dist/chocolatey.
#
#   render.sh 0.11.0
#
# Fills the MSIs' SHA-256s from the release's checksums.txt, once its
# signature checks out against the release key. Then, on Windows, in a copy
# of dist/chocolatey:
#   choco pack chatwithwork-desktop.nuspec
#   choco install chatwithwork-desktop -y --source "'.;https://community.chocolatey.org/api/v2/'"  # test
# (the community feed resolves its webview2-runtime dependency)
#   choco push chatwithwork-desktop.X.Y.Z.nupkg --source https://push.chocolatey.org/
set -euo pipefail
version="${1:?version, e.g. 0.11.0}"
version="${version#v}"
repo="crmne/chatwithwork-desktop"
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$root/dist/chocolatey"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gh release download "v$version" -R "$repo" -p checksums.txt -p checksums.txt.sig -D "$work"
"$root/packaging/verify-checksums.sh" "$work"
sums="$(cat "$work/checksums.txt")"
msi_sha256() {
  local sha
  sha="$(awk -v name="cww-v$version-$1.msi" '$2 == name { print $1 }' <<<"$sums")"
  [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || { echo "checksums.txt has no $1 MSI" >&2; exit 1; }
  printf '%s' "$sha"
}
x64="$(msi_sha256 x86_64-pc-windows-msvc)"
arm64="$(msi_sha256 aarch64-pc-windows-msvc)"
rm -rf "$out" && mkdir -p "$out/tools"
render() {
  sed -e "s/@VERSION@/$version/g" -e "s/@WINDOWS_X64_MSI_SHA256@/$x64/g" \
      -e "s/@WINDOWS_ARM64_MSI_SHA256@/$arm64/g" "$1" > "$2"
  if grep -n '@[A-Z0-9_]*@' "$2"; then echo "$2 has a placeholder this script doesn't fill"; exit 1; fi
}
render "$root/packaging/chocolatey/chatwithwork-desktop.nuspec.in" "$out/chatwithwork-desktop.nuspec"
render "$root/packaging/chocolatey/tools/chocolateyinstall.ps1.in" "$out/tools/chocolateyinstall.ps1"
cp "$root/packaging/chocolatey/tools/chocolateyuninstall.ps1" "$out/tools/"
echo "Rendered $out"
