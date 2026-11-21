#!/usr/bin/env bash
# Keep the newest Desktop release the repository's latest. The stable
# download links (releases/latest/download/...) and the updater read GitHub's
# latest release, so a browser extension release (browser-v*) must never take
# its place. This marks the newest published, non-prerelease vX.Y.Z release
# latest where another is, and, given a browser-v* tag, marks that release
# not latest first. It changes no tag, asset, or note. keep-desktop-latest.yml
# runs it when a browser-v* release is published or edited, and once a day.
#
#   keep-desktop-latest.sh [BROWSER_TAG]           # needs GH_TOKEN and GITHUB_REPOSITORY
#   DRY_RUN=1 keep-desktop-latest.sh [BROWSER_TAG] # says what it would change, changes nothing
set -euo pipefail
tag="${1:-}"
repo="${GITHUB_REPOSITORY:?owner/name of the repository}"
[ -z "$tag" ] || [[ "$tag" == browser-v* ]] || { echo "$tag isn't a browser extension release; nothing to do"; exit 0; }

run() {
  if [ -n "${DRY_RUN:-}" ]; then echo "would run: $*"; else "$@" > /dev/null; fi
}

# Published releases, as id, tag, prerelease.
releases() {
  gh api --paginate "repos/$repo/releases?per_page=100" --jq '.[] | select(.draft | not) | [.id, .tag_name, .prerelease] | @tsv'
}

# The newest stable Desktop release, as tag and id: vX.Y.Z, no suffix, not a prerelease.
newest_desktop() {
  awk -F '\t' '$2 ~ /^v[0-9]+\.[0-9]+\.[0-9]+$/ && $3 == "false" { print $2 "\t" $1 }' | sort -V -k1,1 | tail -n 1
}

listed="$(releases)"
if [ -n "$tag" ]; then
  browser_id="$(awk -F '\t' -v tag="$tag" '$2 == tag { print $1 }' <<< "$listed")"
  [ -n "$browser_id" ] || { echo "no published release for $tag" >&2; exit 1; }
  run gh api --method PATCH "repos/$repo/releases/$browser_id" -f make_latest=false
fi

# A Desktop release published meanwhile is newer than the one chosen: choose
# again until the newest is the latest, so this never puts an older one back.
for _ in 1 2 3; do
  desktop="$(newest_desktop <<< "$listed")"
  [ -n "$desktop" ] || { echo "no published Desktop vX.Y.Z release to keep latest" >&2; exit 1; }
  desktop_tag="${desktop%%$'\t'*}"
  latest="$(gh api "repos/$repo/releases/latest" --jq .tag_name)"
  if [ "$latest" = "$desktop_tag" ]; then
    echo "$desktop_tag is the latest release"
    exit 0
  fi
  echo "$latest is the latest release; marking $desktop_tag latest"
  run gh api --method PATCH "repos/$repo/releases/${desktop##*$'\t'}" -f make_latest=true
  [ -z "${DRY_RUN:-}" ] || exit 0
  listed="$(releases)"
done
echo "GitHub's latest release is still not the newest Desktop release" >&2
exit 1
