#!/usr/bin/env bash
# Update Chat with Work Desktop on openSUSE's Open Build Service,
# home:plenty/chatwithwork-desktop, to a published release.
#
#   submit.sh 0.16.2            # stage the new spec and archives in dist/obs and show the change
#   submit.sh 0.16.2 --submit   # also commit them, which makes OBS rebuild and publish
#
# The spec and archives come from packaging/copr/submit.sh, which checks
# checksums.txt against its signature and the release key and the archives
# against it, so OBS gets the same files as COPR. The previous version's
# archives are removed from the package.
#
# The OBS login is the account `plenty`. It's read from OBS_USERNAME and
# OBS_PASSWORD when set, otherwise from 1Password through op-cache
# (op://Plenty/openSUSE). It goes into a temporary osc config readable only
# by you, removed on exit, and is never printed.
#
# OBS_ROOT points the script at another checkout (the tests use it with
# stand-ins for the COPR script, osc and op-cache).
set -euo pipefail
version="${1:?version, e.g. 0.16.2}"
version="${version#v}"
submit=false
case "${2:-}" in
  "") ;;
  --submit) submit=true ;;
  *) echo "unknown argument: $2 (expected --submit)" >&2; exit 2 ;;
esac
project="${OBS_PROJECT:-home:plenty}"
package="chatwithwork-desktop"
apiurl="${OBS_APIURL:-https://api.opensuse.org}"
root="${OBS_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
copr="$root/dist/copr"
out="$root/dist/obs"

"$root/packaging/copr/submit.sh" "$version"

username="${OBS_USERNAME:-$(op-cache read "op://Plenty/openSUSE/username")}"
password="${OBS_PASSWORD:-$(op-cache read "op://Plenty/openSUSE/password")}"
[ -n "$username" ] && [ -n "$password" ] || { echo "no OBS login (OBS_USERNAME/OBS_PASSWORD or op://Plenty/openSUSE)" >&2; exit 1; }

umask 077
session="$(mktemp -d)"
trap 'rm -rf "$session"' EXIT
export OSC_CONFIG="$session/oscrc"
# A cookie jar of its own, so a session another OBS account left in osc's
# shared one is never used instead of this login.
cat > "$OSC_CONFIG" <<EOF
[general]
apiurl = $apiurl
cookiejar = $session/cookiejar

[$apiurl]
user = $username
pass = $password
credentials_mgr_class = osc.credentials.PlaintextConfigFileCredentialsManager
EOF
umask 022

rm -rf "$out" && mkdir -p "$out"
(cd "$out" && osc checkout --output-dir "$package" "$project" "$package" >/dev/null) ||
  { echo "couldn't check out $project/$package from $apiurl" >&2; exit 1; }
work="$out/$package"
find "$work" -maxdepth 1 -name 'cww-app-v*-unknown-linux-gnu.tar.gz' -delete
cp "$copr/SPECS/$package.spec" "$work/"
cp "$copr/SOURCES/cww-app-v$version-x86_64-unknown-linux-gnu.tar.gz" \
   "$copr/SOURCES/cww-app-v$version-aarch64-unknown-linux-gnu.tar.gz" "$work/"
(cd "$work" && osc addremove >/dev/null && osc status)

if $submit; then
  (cd "$work" && osc commit -m "Update to $version")
  echo "Committed $version to $project/$package; follow the builds with: osc results $project $package"
else
  echo "Staged in $work; run again with --submit to commit"
fi
