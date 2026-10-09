#!/usr/bin/env bash
# Checks that the tree can be released as <version>, before anything is published:
#   - the workspace (every crate) and the template (.rivium-template) have that version;
#   - the release notes render, with their five parts (Added, Changed, Fixed, Breaking,
#     Template): git-cliff with cliff.toml, from the commits since the previous tag;
#   - a new minor or major version, once there is an earlier release, has its section in
#     docs/upgrading.md, which is written by hand;
#   - crates.io does not have that version of any library crate yet, and before the first
#     release, no crate of their names at all (someone else's).
# Writes the release notes to <notes file>, the GitHub release's body. With --notes-only it only
# renders them, as once the crates are published. Needs git-cliff.
# Usage: scripts/release-check.sh [--notes-only] <version> <notes file>
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
notes_only=false
if [ "${1:-}" = --notes-only ]; then
  notes_only=true
  shift
fi
[ $# -eq 2 ] || { echo "usage: scripts/release-check.sh [--notes-only] <version> <notes file>" >&2; exit 2; }
version=$1
notes=$2
crates="rivium-error rivium rivium-http rivium-jni rivium-test"
failures=0
ok() { echo "ok   $*"; }
fail() { echo "FAIL $*"; failures=$((failures + 1)); }

# The commits since the previous tag; once the tag exists, those of the release it marks.
if git -C "$root" rev-parse -q --verify "refs/tags/v$version" > /dev/null; then
  range=(--latest)
else
  range=(--unreleased --tag "v$version")
fi
if (cd "$root" && git cliff --config cliff.toml "${range[@]}" --strip all --output "$notes" 2> /dev/null); then
  missing=""
  for heading in Added Changed Fixed Breaking Template; do
    grep -q "^### $heading$" "$notes" || missing="$missing $heading"
  done
  if [ -z "$missing" ]; then ok "the release notes render with their five parts"; else fail "the release notes lack:$missing"; fi
else
  fail "git cliff cannot render the release notes"
fi
$notes_only && exit "$((failures > 0))"

workspace=$(sed -n -E 's/^version = "([^"]+)"$/\1/p' "$root/Cargo.toml" | head -n 1)
if [ "$workspace" = "$version" ]; then ok "the workspace is at $version"; else fail "the workspace is at $workspace, not $version"; fi
if grep -q "^version = \"$version\"$" "$root/template/.rivium-template"; then ok "the template records $version"; else fail "template/.rivium-template does not record $version"; fi

earlier=$(git -C "$root" tag -l 'v*' | grep -vx "v$version" || true)
if [ -n "$earlier" ] && [ "${version##*.}" = 0 ]; then
  if grep -q "^## ${version//./\\.}\b" "$root/docs/upgrading.md"; then
    ok "docs/upgrading.md has a section for $version"
  else
    fail "docs/upgrading.md has no section for $version, a breaking release"
  fi
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
