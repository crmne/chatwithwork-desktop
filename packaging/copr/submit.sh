#!/usr/bin/env bash
# Build Chat with Work Desktop's source RPM for Fedora COPR from a published
# release, and submit it to paolino/chatwithwork-desktop.
#
#   submit.sh 0.11.0            # render the spec and build the SRPM in dist/copr
#   submit.sh 0.11.0 --submit   # also submit it (needs ~/.config/copr)
#
# The spec repackages the release's complete Linux archives, checked against
# the release's signed checksums.txt here and again by the spec's %prep, so
# COPR's packages carry the same files as the release's .rpm. Builds run in a
# Fedora container; only Docker (or Podman) is needed on this machine.
set -euo pipefail
version="${1:?version, e.g. 0.11.0}"
version="${version#v}"
submit="${2:-}"
pkgrel="${PKGREL:-1}"
repo="crmne/chatwithwork-desktop"
project="${COPR_PROJECT:-paolino/chatwithwork-desktop}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$root/dist/copr"
engine="$(command -v podman || command -v docker)"
platform="linux/$(uname -m | sed -e s/x86_64/amd64/ -e s/aarch64/arm64/)"
rm -rf "$out" && mkdir -p "$out/SOURCES" "$out/SPECS"

gh release download "v$version" -R "$repo" -D "$out/SOURCES" -p checksums.txt \
  -p "cww-app-v$version-x86_64-unknown-linux-gnu.tar.gz" \
  -p "cww-app-v$version-aarch64-unknown-linux-gnu.tar.gz"
(cd "$out/SOURCES" && grep -E " cww-app-v$version-(x86_64|aarch64)-unknown-linux-gnu\.tar\.gz$" checksums.txt | sha256sum -c -)
sha256() {
  awk -v name="cww-app-v$version-$1-unknown-linux-gnu.tar.gz" '$2 == name { print $1 }' "$out/SOURCES/checksums.txt"
}

# The spec at the release's tag, so the package matches what was released.
spec_in="$out/chatwithwork-desktop.spec.in"
gh api -H "Accept: application/vnd.github.raw" \
  "repos/$repo/contents/packaging/copr/chatwithwork-desktop.spec.in?ref=v$version" > "$spec_in" 2>/dev/null ||
  cp "$root/packaging/copr/chatwithwork-desktop.spec.in" "$spec_in"
spec="$out/SPECS/chatwithwork-desktop.spec"
VERSION="$version" PKGREL="$pkgrel" AMD64="$(sha256 x86_64)" ARM64="$(sha256 aarch64)" \
  DATE="$(LC_ALL=C date -u +'%a %b %d %Y')" POSTINSTALL="$root/packaging/linux/postinstall.sh" \
  ruby -e '
    post = File.read(ENV["POSTINSTALL"]).lines.reject { _1.start_with?("#!") }.join.chomp
    text = STDIN.read
      .gsub("@VERSION@", ENV["VERSION"]).gsub("@PKGREL@", ENV["PKGREL"])
      .gsub("@LINUX_APP_AMD64_SHA256@", ENV["AMD64"]).gsub("@LINUX_APP_ARM64_SHA256@", ENV["ARM64"])
      .gsub("@CHANGELOG_DATE@", ENV["DATE"]).gsub("@POSTINSTALL@") { post }
    abort "unfilled placeholder: #{$&}" if text =~ /@[A-Z0-9_]+@/
    print text
  ' < "$spec_in" > "$spec"

"$engine" run --rm --platform "$platform" -e OWNER="$(id -u):$(id -g)" -v "$out:/root/rpmbuild:Z" registry.fedoraproject.org/fedora:latest bash -euc '
  dnf -q -y install rpm-build rpmlint systemd-rpm-macros >/dev/null 2>&1
  rpmlint /root/rpmbuild/SPECS/chatwithwork-desktop.spec || true
  rpmbuild -bs /root/rpmbuild/SPECS/chatwithwork-desktop.spec
  chown -R "$OWNER" /root/rpmbuild
'
srpm="$(ls "$out"/SRPMS/chatwithwork-desktop-"$version"-"$pkgrel".*.src.rpm)"
echo "Built $srpm"

if [ "$submit" = "--submit" ]; then
  [ -f "$HOME/.config/copr" ] || { echo "No ~/.config/copr: get an API token at https://copr.fedorainfracloud.org/api/"; exit 1; }
  "$engine" run --rm --platform "$platform" -v "$out:/work:Z" -v "$HOME/.config/copr:/root/.config/copr:ro,Z" \
    registry.fedoraproject.org/fedora:latest bash -euc "
      dnf -q -y install copr-cli >/dev/null
      copr-cli build --nowait $project /work/SRPMS/$(basename "$srpm")
    "
fi
