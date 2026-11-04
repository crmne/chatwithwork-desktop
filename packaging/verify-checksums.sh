#!/usr/bin/env bash
# Check a release's checksums.txt against its Ed25519 signature
# (checksums.txt.sig, 64 raw bytes) and the key the desktop app's updater
# carries (app/assets/update-public-key.hex), as release.yml does.
#
#   verify-checksums.sh DIR    # DIR holds checksums.txt and checksums.txt.sig
set -euo pipefail
dir="${1:?directory holding checksums.txt and checksums.txt.sig}"
root="$(cd "$(dirname "$0")/.." && pwd)"
key="$(tr -d '[:space:]' < "$root/app/assets/update-public-key.hex")"
[[ "$key" =~ ^[0-9a-f]{64}$ ]] || { echo "bad public key in app/assets/update-public-key.hex" >&2; exit 1; }
[ "$(wc -c < "$dir/checksums.txt.sig")" -eq 64 ] || { echo "checksums.txt.sig isn't a 64-byte Ed25519 signature" >&2; exit 1; }
der="$(mktemp)"
trap 'rm -f "$der"' EXIT
# The DER SubjectPublicKeyInfo of an Ed25519 key: a fixed prefix, then the key.
printf '302a300506032b6570032100%s' "$key" | xxd -r -p > "$der"
openssl pkeyutl -verify -pubin -keyform DER -inkey "$der" -rawin \
  -in "$dir/checksums.txt" -sigfile "$dir/checksums.txt.sig" >/dev/null ||
  { echo "checksums.txt's signature doesn't match the release key" >&2; exit 1; }
echo "checksums.txt is signed by the release key"
