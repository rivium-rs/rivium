#!/usr/bin/env bash
# Checks that the tree can be released as <version>, before anything is published:
#   - the workspace (every crate) and the template (.rivium-template) have that version;
#   - CHANGELOG.md has its section, `## [<version>] - <date>`, with the five headings Added,
#     Changed, Fixed, Breaking and Template; a dry run accepts `unreleased` for the date;
#   - a new minor or major version, once there is an earlier release, has its section in
#     docs/upgrading.md;
#   - crates.io does not have that version of any library crate yet, and before the first
#     release, no crate of their names at all (someone else's).
# Writes the section, without its title, to <notes file>: the GitHub release's body. With
# --notes-only it only writes that file, as once the crates are published.
# Usage: scripts/release-check.sh [--dry-run | --notes-only] <version> <notes file>
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dry=false
notes_only=false
case "${1:-}" in
  --dry-run) dry=true && shift ;;
  --notes-only) notes_only=true && shift ;;
esac
[ $# -eq 2 ] || { echo "usage: scripts/release-check.sh [--dry-run | --notes-only] <version> <notes file>" >&2; exit 2; }
version=$1
notes=$2
crates="rivium-error rivium rivium-http rivium-jni rivium-test"
failures=0
ok() { echo "ok   $*"; }
fail() { echo "FAIL $*"; failures=$((failures + 1)); }

workspace=$(sed -n -E 's/^version = "([^"]+)"$/\1/p' "$root/Cargo.toml" | head -n 1)
if [ "$workspace" = "$version" ]; then ok "the workspace is at $version"; else fail "the workspace is at $workspace, not $version"; fi
if grep -q "^version = \"$version\"$" "$root/template/.rivium-template"; then ok "the template records $version"; else fail "template/.rivium-template does not record $version"; fi

date='[0-9]{4}-[0-9]{2}-[0-9]{2}'
$dry && date="($date|unreleased)"
escaped=${version//./\\.}
section=$(awk -v head="^## \\\\[$escaped\\\\] - $date\$" '
  $0 ~ head { inside = 1; next }
  inside && /^## / { exit }
  inside { print }
' "$root/CHANGELOG.md")
if [ -n "$section" ]; then
  ok "CHANGELOG.md has a section for $version"
  for heading in Added Changed Fixed Breaking Template; do
    grep -q "^### $heading$" <<< "$section" || fail "its section lacks ### $heading"
  done
  printf '%s\n' "$section" | sed -e '/./,$!d' > "$notes"
else
  fail "CHANGELOG.md has no section \"## [$version] - <date>\""
fi
$notes_only && exit "$((failures > 0))"

earlier=$(git -C "$root" tag -l 'v*' | grep -vx "v$version" || true)
if [ -n "$earlier" ] && [ "${version##*.}" = 0 ]; then
  if grep -q "^## $escaped\b" "$root/docs/upgrading.md"; then ok "docs/upgrading.md has a section for $version"; else fail "docs/upgrading.md has no section for $version, a breaking release"; fi
fi

crates_io() { # <path>: the status of a crates.io API request
  curl -s -o /dev/null -w '%{http_code}' -A "rivium-release-check (https://github.com/rivium-rs/rivium)" \
    "https://crates.io/api/v1/crates/$1"
}
for crate in $crates; do
  if [ -z "$earlier" ] && [ "$(crates_io "$crate")" != 404 ]; then
    fail "crates.io has a crate $crate before Rivium's first release"
    continue
  fi
  status=$(crates_io "$crate/$version")
  case "$status" in
    404) ok "crates.io does not have $crate $version" ;;
    200) fail "crates.io has $crate $version already" ;;
    *) fail "crates.io answered $status for $crate $version" ;;
  esac
done

[ "$failures" -eq 0 ] || { echo "release-check: $failures failure(s)"; exit 1; }
echo "release-check: $version can be released"
