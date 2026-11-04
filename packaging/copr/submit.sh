#!/usr/bin/env bash
# Build Chat with Work Desktop's source RPM for Fedora COPR from a published
# release, and submit it to paolino/chatwithwork-desktop.
#
#   submit.sh 0.11.0            # render the spec and build the SRPM in dist/copr
#   submit.sh 0.11.0 --submit   # also submit it (needs ~/.config/copr)
#
# checksums.txt is checked against its signature and the release key, the
# archives against checksums.txt, and the spec's %prep checks them again, so
# COPR's packages carry the same files as the release's .rpm. Builds run in a
# Fedora container; only Docker (or Podman) is needed on this machine.
set -euo pipefail
version="${1:?version, e.g. 0.11.0}"
version="${version#v}"
submit=false
case "${2:-}" in
  "") ;;
  --submit) submit=true ;;
  *) echo "unknown argument: $2 (expected --submit)" >&2; exit 2 ;;
esac
pkgrel="${PKGREL:-1}"
repo="crmne/chatwithwork-desktop"
project="${COPR_PROJECT:-paolino/chatwithwork-desktop}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$root/dist/copr"
engine="$(command -v podman || command -v docker)"
platform="linux/$(uname -m | sed -e s/x86_64/amd64/ -e s/aarch64/arm64/)"
rm -rf "$out" && mkdir -p "$out/SOURCES" "$out/SPECS"

gh release download "v$version" -R "$repo" -D "$out/SOURCES" -p checksums.txt -p checksums.txt.sig \
  -p "cww-app-v$version-x86_64-unknown-linux-gnu.tar.gz" \
  -p "cww-app-v$version-aarch64-unknown-linux-gnu.tar.gz"
"$root/packaging/verify-checksums.sh" "$out/SOURCES"
(cd "$out/SOURCES" && grep -E " cww-app-v$version-(x86_64|aarch64)-unknown-linux-gnu\.tar\.gz$" checksums.txt | sha256sum -c -)
rm "$out/SOURCES/checksums.txt.sig"
sha256() {
  awk -v name="cww-app-v$version-$1-unknown-linux-gnu.tar.gz" '$2 == name { print $1 }' "$out/SOURCES/checksums.txt"
}
# The spec installs the browser engine, which archives before 0.8 don't carry.
for arch in x86_64 aarch64; do
  listing="$(tar -tzf "$out/SOURCES/cww-app-v$version-$arch-unknown-linux-gnu.tar.gz")"
  grep -q "^cww-app-v$version-$arch-unknown-linux-gnu/browser-engine/" <<<"$listing" ||
    { echo "the $arch archive has no browser-engine/; this spec can't package v$version" >&2; exit 1; }
done

# The spec and post-install message at the release's tag, so the package
# matches what was released. Releases from before the spec existed (0.11.0)
# use this checkout's, and say so.
spec_in="$out/chatwithwork-desktop.spec.in"
if ! gh api -H "Accept: application/vnd.github.raw" \
    "repos/$repo/contents/packaging/copr/chatwithwork-desktop.spec.in?ref=v$version" > "$spec_in" 2>/dev/null; then
  echo "v$version has no packaging/copr spec; using this checkout's" >&2
  cp "$root/packaging/copr/chatwithwork-desktop.spec.in" "$spec_in"
fi
postinstall="$out/postinstall.sh"
gh api -H "Accept: application/vnd.github.raw" \
  "repos/$repo/contents/packaging/linux/postinstall.sh?ref=v$version" > "$postinstall" ||
  { echo "couldn't read packaging/linux/postinstall.sh at v$version" >&2; exit 1; }
spec="$out/SPECS/chatwithwork-desktop.spec"
VERSION="$version" PKGREL="$pkgrel" AMD64="$(sha256 x86_64)" ARM64="$(sha256 aarch64)" \
  DATE="$(LC_ALL=C date -u +'%a %b %d %Y')" POSTINSTALL="$postinstall" \
  ruby -e '
    post = File.read(ENV["POSTINSTALL"]).lines.reject { _1.start_with?("#!") }.join.chomp
    text = STDIN.read
      .gsub("@VERSION@", ENV["VERSION"]).gsub("@PKGREL@", ENV["PKGREL"])
      .gsub("@LINUX_APP_AMD64_SHA256@", ENV["AMD64"]).gsub("@LINUX_APP_ARM64_SHA256@", ENV["ARM64"])
      .gsub("@CHANGELOG_DATE@", ENV["DATE"]).gsub("@POSTINSTALL@") { post }
    abort "unfilled placeholder: #{$&}" if text =~ /@[A-Z0-9_]+@/
    print text
  ' < "$spec_in" > "$spec"

"$engine" run --rm --platform "$platform" -v "$out:/root/rpmbuild:Z" registry.fedoraproject.org/fedora:latest bash -euc '
  dnf -q -y install rpm-build rpmlint systemd-rpm-macros >/dev/null 2>&1
  rpmlint /root/rpmbuild/SPECS/chatwithwork-desktop.spec || true
  rpmbuild -bs /root/rpmbuild/SPECS/chatwithwork-desktop.spec
'
# A rootful engine leaves the output owned by the host's root; hand it back.
# Rootless Docker or Podman already wrote it as the invoking user, and a
# chown inside would give it to a subordinate UID instead.
if [ "$(stat -c %u "$out/SRPMS")" = 0 ] && [ "$(id -u)" != 0 ]; then
  "$engine" run --rm --platform "$platform" -v "$out:/out:Z" registry.fedoraproject.org/fedora:latest \
    chown -R "$(id -u):$(id -g)" /out
fi
srpm="$(ls "$out"/SRPMS/chatwithwork-desktop-"$version"-"$pkgrel".*.src.rpm)"
echo "Built $srpm"

if $submit; then
  [ -f "$HOME/.config/copr" ] || { echo "No ~/.config/copr: get an API token at https://copr.fedorainfracloud.org/api/"; exit 1; }
  "$engine" run --rm --platform "$platform" -v "$out:/work:Z" -v "$HOME/.config/copr:/root/.config/copr:ro,Z" \
    registry.fedoraproject.org/fedora:latest bash -euc "
      dnf -q -y install copr-cli >/dev/null
      copr-cli build --nowait $project /work/SRPMS/$(basename "$srpm")
    "
fi
