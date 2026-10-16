#!/usr/bin/env bash
# Validate source/version/notes only. This helper never uploads packages or creates tags.
set -euo pipefail
fail() { echo "release: $*" >&2; exit 1; }
[ "$#" -ge 2 ] || { echo 'usage: release-check.sh preview <output-dir> | tag <vX.Y.Z> <output-dir> | environment <JSON>' >&2; exit 2; }
mode=$1; shift
tag=
case "$mode:$#" in
  preview:1) ;;
  tag:2) tag=$1; shift ;;
  environment:1)
    jq -e '.protection_rules | type == "array" and any(.[]; .type == "required_reviewers" and
      (.reviewers | type == "array" and any(.[]; (.type == "User" or .type == "Team") and
        (.reviewer.id | type == "number" and . > 0))))' "$1" >/dev/null \
      || fail 'protected environment has no required reviewer'
    echo 'release: required environment reviewer is configured'
    exit 0 ;;
  *) exit 2 ;;
esac
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
# Real releases must be immutable Git inputs, not a locally patched checkout.
if [ "$mode" = tag ]; then
  [ -z "$(git status --porcelain --untracked-files=all)" ] || fail 'tag checkout is dirty'
fi
mkdir -p "$1"
out=$(cd "$1" && pwd)
cargo metadata --no-deps --format-version 1 --locked > "$out/metadata.json"
expected='["rivium","rivium-host","rivium-http","rivium-jni","rivium-test","rivium-util"]'
jq -e --argjson expected "$expected" '[.packages[] | select(.publish != [])] |
  (map(.name) | sort) == $expected and (map(.version) | unique | length) == 1' "$out/metadata.json" >/dev/null \
  || fail 'expected exactly the six public crates at one version'
version=$(jq -r '.packages[] | select(.name == "rivium") | .version' "$out/metadata.json")
valid_version() { [[ "$1" =~ ^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$ ]]; }
valid_version "$version" || fail 'only stable X.Y.Z versions are supported'
jq -e --arg v "^$version" --argjson names "$expected" 'all(.packages[] | select(.publish != []) |
  .dependencies[] | select(.kind != "dev") | select(.name as $n | $names | index($n)); .req == $v)' \
  "$out/metadata.json" >/dev/null || fail 'internal dependency versions are not aligned'
jq -e --argjson names "$expected" 'all(.packages[] | select(.publish != []) |
  .dependencies[] | select(.kind != "dev"); .path == null or (.name as $n | $names | index($n)) != null)' \
  "$out/metadata.json" >/dev/null || fail 'normal/build dependency points outside the public workspace crates'
head=$(git rev-parse HEAD)
if [ "$mode" = tag ]; then
  [ "$tag" = "v$version" ] || fail 'tag and workspace version differ'
  [ "$(git rev-parse --verify "refs/tags/$tag^{commit}")" = "$head" ] || fail 'tag does not identify HEAD'
  git merge-base --is-ancestor "$head" refs/remotes/origin/main || fail 'release commit is not on origin/main'
fi
baseline=
while IFS= read -r candidate; do
  [[ "$candidate" = v* ]] || continue
  valid_version "${candidate#v}" || continue
  [ "$candidate" != "$tag" ] || continue
  baseline=$candidate; break
done < <(git for-each-ref --merged=HEAD --sort=-v:refname --format='%(refname:short)' refs/tags)
[ -n "$baseline" ] || fail 'a reachable stable release baseline is required'
notes_version=$version
if [ "$mode" = preview ] && [ "$baseline" = "v$version" ]; then
  IFS=. read -r major minor patch <<< "$version"
  notes_version="$major.$minor.$((patch + 1))"
  valid_version "$notes_version" || fail 'preview version overflow'
  echo "release: packages stay at $version; previewing notes for $notes_version only"
fi
version_key() { local a b c; IFS=. read -r a b c <<< "$1"; printf '%09d%09d%09d' "$a" "$b" "$c"; }
[[ "$(version_key "$notes_version")" > "$(version_key "${baseline#v}")" ]] || fail 'version does not advance the baseline'
if [ "$mode" = tag ]; then
  grep -Fxq "## $version" docs/platforms.md || fail 'candidate platform verification record is missing'
  old=${baseline#v}
  if [ "${version%.*}" != "${old%.*}" ]; then
    grep -Fxq "## $version" docs/upgrading.md || fail 'breaking release needs an upgrading section'
  fi
  scripts/release-registry.sh absent "$version"
fi
[ "$(git cliff --version)" = 'git-cliff 2.14.2' ] || fail 'git-cliff 2.14.2 is required'
git cliff "$baseline..HEAD" --config cliff.toml --tag "v$notes_version" --strip all --output "$out/notes.md"
for heading in Added Changed Fixed Breaking 'Platforms and upgrades'; do
  [ "$(grep -Fxc "### $heading" "$out/notes.md")" -eq 1 ] || fail "release notes lack one unambiguous $heading section"
done
[ -s "$out/notes.md" ] || fail 'release notes are empty'
rust=$(rustc --version | cut -d ' ' -f 2)
valid_version "$rust" || fail 'a stable Rust compiler is required for reproducible packaging'
jq -n --arg mode "$mode" --arg version "$version" --arg notes_version "$notes_version" \
  --arg baseline "$baseline" --arg sha "$head" --arg rust "$rust" \
  '{mode:$mode,version:$version,notes_version:$notes_version,baseline:$baseline,sha:$sha,rust:$rust}' > "$out/release.json"
echo "release: $mode source and notes checked at $head"
