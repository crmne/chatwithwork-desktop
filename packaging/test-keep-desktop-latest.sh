#!/usr/bin/env bash
# Tests keep-desktop-latest.sh against a fake gh that keeps its releases and
# its latest in files: run with no arguments.
set -euo pipefail
script="$(cd "$(dirname "$0")" && pwd)/keep-desktop-latest.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/gh" <<'GH'
#!/usr/bin/env bash
# A fake gh api: GET releases lists $FAKE/releases.tsv (id, tag, prerelease,
# draft), GET releases/latest prints $FAKE/latest, PATCH make_latest=true sets
# it, and every PATCH is logged. A release in $FAKE/publish_on_patch is added
# by the first PATCH, as if release.yml published it meanwhile.
set -euo pipefail
args="$*"
case "$args" in
  *"--method PATCH"*)
    echo "$args" >> "$FAKE/patches"
    id="$(sed -E 's|.*releases/([0-9]+).*|\1|' <<< "$args")"
    if [[ "$args" == *make_latest=true* ]]; then awk -F '\t' -v id="$id" '$1 == id { print $2 }' "$FAKE/releases.tsv" > "$FAKE/latest"; fi
    if [ -s "$FAKE/publish_on_patch" ]; then cat "$FAKE/publish_on_patch" >> "$FAKE/releases.tsv"; : > "$FAKE/publish_on_patch"; fi
    ;;
  *releases/latest*) cat "$FAKE/latest" ;;
  *"releases?per_page"*) awk -F '\t' '$4 == "false" { print $1 "\t" $2 "\t" $3 }' "$FAKE/releases.tsv" ;;
  *) echo "fake gh: unexpected $args" >&2; exit 2 ;;
esac
GH
chmod +x "$work/bin/gh"
export PATH="$work/bin:$PATH" GITHUB_REPOSITORY=owner/repo FAKE="$work"

failures=0
check() { # name, expected latest, expected PATCH count
  local latest patches
  latest="$(cat "$FAKE/latest")"
  patches="$( [ -f "$FAKE/patches" ] && wc -l < "$FAKE/patches" || echo 0 )"
  if [ "$latest" = "$2" ] && [ "$patches" -eq "$3" ]; then echo "ok: $1"; else echo "FAIL: $1 (latest $latest, $patches patches)"; failures=$((failures + 1)); fi
}
setup() { # latest, then releases as "id tag prerelease draft"
  rm -f "$FAKE/patches"; : > "$FAKE/publish_on_patch"; echo "$1" > "$FAKE/latest"; shift
  printf '%s\n' "$@" | tr ' ' '\t' > "$FAKE/releases.tsv"
}
RELEASES=("1 v0.9.0 false false" "2 v0.10.0 false false" "3 browser-v0.1.6 false false" "4 v0.11.0-rc.1 true false" "5 v0.12.0 false true")

setup browser-v0.1.6 "${RELEASES[@]}"
"$script" browser-v0.1.6 > /dev/null
check "a browser release made latest gives way to the newest stable Desktop, by version, not a prerelease or draft" v0.10.0 2
if grep -q "releases/3 -f make_latest=false" "$FAKE/patches"; then echo "ok: the browser release is marked not latest"; else echo "FAIL: browser release not marked"; failures=$((failures + 1)); fi

setup v0.10.0 "${RELEASES[@]}"
"$script" browser-v0.1.6 > /dev/null
check "with Desktop already latest only the browser release is marked" v0.10.0 1

setup v0.10.0 "${RELEASES[@]}"
"$script" > /dev/null
check "the daily check with Desktop latest changes nothing" v0.10.0 0

setup browser-v0.1.6 "${RELEASES[@]}"
DRY_RUN=1 "$script" browser-v0.1.6 > /dev/null
check "a dry run changes nothing" browser-v0.1.6 0

setup browser-v0.1.6 "${RELEASES[@]}"
echo $'6\tv0.13.0\tfalse\tfalse' > "$FAKE/publish_on_patch"
"$script" browser-v0.1.6 > /dev/null
check "a Desktop release published meanwhile ends latest, never the older one" v0.13.0 3

setup browser-v0.1.6 "${RELEASES[@]}"
"$script" v0.10.0 > /dev/null
check "a Desktop tag is no browser release: nothing to do" browser-v0.1.6 0

setup browser-v0.1.6 "${RELEASES[@]}"
if "$script" browser-v9.9.9 > /dev/null 2>&1; then echo "FAIL: an unknown tag passed"; failures=$((failures + 1)); else echo "ok: an unknown browser tag fails"; fi

setup browser-v0.1.6 "3 browser-v0.1.6 false false"
if "$script" browser-v0.1.6 > /dev/null 2>&1; then echo "FAIL: no Desktop release passed"; failures=$((failures + 1)); else echo "ok: with no Desktop release it fails"; fi

exit "$failures"
