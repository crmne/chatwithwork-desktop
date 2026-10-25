#!/usr/bin/env bash
# Submit ChatWithWork.Desktop to microsoft/winget-pkgs once a release exists.
#
#   winget-submit.sh 0.4.0            # prepare the branch and print what it would open
#   winget-submit.sh 0.4.0 --open     # also push it and open the pull request
#
# Renders the WinGet manifests from the release tag's packaging/winget/*.yaml.in
# with the MSIs' SHA-256s from the release's checksums.txt, as packaging.yml
# renders them, so the manifests match the published MSIs. Works in Carmine's
# fork, crmne/winget-pkgs.
set -euo pipefail
version="${1:?version, e.g. 0.4.0}"
version="${version#v}"
open="${2:-}"
id="ChatWithWork.Desktop"
repo="crmne/chatwithwork-desktop"
work="$(mktemp -d)"

gh release view "v$version" -R "$repo" >/dev/null || { echo "v$version isn't released yet"; exit 1; }
gh release download "v$version" -R "$repo" -p checksums.txt -D "$work"
msi_sha256() {
  local sha
  sha="$(awk -v name="cww-v$version-$1.msi" '$2 == name { print $1 }' "$work/checksums.txt")"
  [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || { echo "checksums.txt has no $1 MSI" >&2; exit 1; }
  printf '%s' "$sha"
}
x64="$(msi_sha256 x86_64-pc-windows-msvc)"
arm64="$(msi_sha256 aarch64-pc-windows-msvc)"
recipes="$work/recipes"
mkdir "$recipes"
for manifest in "$id.yaml" "$id.installer.yaml" "$id.locale.en-US.yaml"; do
  gh api -H "Accept: application/vnd.github.raw" \
    "repos/$repo/contents/packaging/winget/$manifest.in?ref=v$version" |
    sed -e "s/@VERSION@/$version/g" \
        -e "s/@WINDOWS_X64_MSI_SHA256@/$x64/g" \
        -e "s/@WINDOWS_ARM64_MSI_SHA256@/$arm64/g" > "$recipes/$manifest"
  if grep -n '@[A-Z0-9_]*@' "$recipes/$manifest"; then
    echo "$manifest has a placeholder this script doesn't fill"; exit 1
  fi
done

# Each manifest must name this id and version, and every installer URL must load
# and match its hash.
for f in "$recipes"/*.yaml; do
  grep -q "^PackageIdentifier: $id$" "$f" && grep -q "^PackageVersion: $version$" "$f" || { echo "bad id/version in $f"; exit 1; }
done
awk '/InstallerUrl:/{u=$2} /InstallerSha256:/{print u, $2}' "$recipes/$id.installer.yaml" | while read -r url sha; do
  got="$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1)"
  [ "${got,,}" = "${sha,,}" ] || { echo "hash mismatch for $url"; exit 1; }
  echo "ok $url"
done

gh repo sync crmne/winget-pkgs --source microsoft/winget-pkgs --branch master
git clone --depth 1 --filter=blob:none --sparse https://github.com/crmne/winget-pkgs "$work/winget-pkgs"
cd "$work/winget-pkgs"
git sparse-checkout set "manifests/c/ChatWithWork"
branch="chatwithwork-desktop-$version"
git checkout -b "$branch"
dest="manifests/c/ChatWithWork/Desktop/$version"
mkdir -p "$dest" && cp "$recipes"/*.yaml "$dest/"
git add "$dest"
git -c user.name="Carmine Paolino" -c user.email=carmine@paolino.me commit -q -m "New package: $id version $version"
git show --stat HEAD

if [ "$open" = "--open" ]; then
  git push -u origin "$branch"
  gh pr create -R microsoft/winget-pkgs --head "crmne:$branch" --base master \
    --title "New package: $id version $version" \
    --body "Chat with Work Desktop $version: https://github.com/$repo/releases/tag/v$version"
else
  echo "Prepared $work/winget-pkgs on $branch. Validate on the Windows VM with:"
  echo "  winget validate --manifest <dir>; winget install --manifest <dir> --silent"
  echo "then rerun with --open."
fi
