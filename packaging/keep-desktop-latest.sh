#!/usr/bin/env bash
# Keep the newest Desktop release the repository's latest. The stable
# download links (releases/latest/download/...) and the updater read GitHub's
# latest release, so a browser extension release (browser-v*) must never take
# its place: this marks it not latest and marks the newest published,
# non-prerelease v* release latest again. keep-desktop-latest.yml runs it when
# a browser-v* release is published or edited; it changes no tag, asset, or
# note.
#
#   keep-desktop-latest.sh BROWSER_TAG           # needs GH_TOKEN and GITHUB_REPOSITORY
#   DRY_RUN=1 keep-desktop-latest.sh BROWSER_TAG # says what it would change, changes nothing
set -euo pipefail
tag="${1:?a browser-v* release tag}"
repo="${GITHUB_REPOSITORY:?owner/name of the repository}"
[[ "$tag" == browser-v* ]] || { echo "$tag isn't a browser extension release; nothing to do" >&2; exit 0; }

releases="$(gh api --paginate "repos/$repo/releases?per_page=100" \
  --jq '.[] | select(.draft | not) | [.id, .tag_name, .prerelease] | @tsv')"
# The newest stable Desktop release: v<major>.<minor>.<patch>, no suffix, not a prerelease.
desktop="$(awk -F '\t' '$2 ~ /^v[0-9]+\.[0-9]+\.[0-9]+$/ && $3 == "false" { print $2 "\t" $1 }' <<< "$releases" | sort -V -k1,1 | tail -n 1)"
browser_id="$(awk -F '\t' -v tag="$tag" '$2 == tag { print $1 }' <<< "$releases")"
[ -n "$browser_id" ] || { echo "no published release for $tag" >&2; exit 1; }
[ -n "$desktop" ] || { echo "no published Desktop v* release to keep latest" >&2; exit 1; }
desktop_tag="${desktop%%$'\t'*}"
desktop_id="${desktop##*$'\t'}"

run() {
  if [ -n "${DRY_RUN:-}" ]; then echo "would run: $*"; else "$@" > /dev/null; fi
}

# Not latest first, so the browser release never asks to be; then the
# Desktop release takes the place, which GitHub gives only one release.
run gh api --method PATCH "repos/$repo/releases/$browser_id" -f make_latest=false
run gh api --method PATCH "repos/$repo/releases/$desktop_id" -f make_latest=true
latest="$(gh api "repos/$repo/releases/latest" --jq .tag_name)"
echo "$tag is not latest; $desktop_tag is (GitHub's latest is now $latest)"
if [ -z "${DRY_RUN:-}" ] && [ "$latest" != "$desktop_tag" ]; then
  echo "GitHub's latest release is $latest, not $desktop_tag" >&2
  exit 1
fi
