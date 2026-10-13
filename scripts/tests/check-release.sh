#!/usr/bin/env bash
# Throwaway Git repositories and fake HTTP/tools; never tag this repository or contact a publisher.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/repo/scripts" "$work/repo/docs" "$work/bin" "$work/packages"
cp "$root/scripts/"release-*.sh "$root/scripts/check-public.sh" "$work/repo/scripts/"
cp "$root/cliff.toml" "$work/repo/"
printf 'fixture\n' > "$work/repo/Cargo.toml"
printf '## 0.1.1\nReviewed candidate record.\n' > "$work/repo/docs/platforms.md"
printf '## 0.1.0\nInitial release.\n' > "$work/repo/docs/upgrading.md"
printf 'private-release-marker\n' > "$work/denylist"
cat > "$work/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
[ "$*" = 'metadata --no-deps --format-version 1 --locked' ] || exit 91
jq -nc --arg v "${VERSION:-0.1.1}" --arg plant "${PLANT:-}" '{packages:
  (["rivium","rivium-error","rivium-http","rivium-jni","rivium-test"] | map({name:.,version:$v,publish:null,dependencies:[]}))} |
  if $plant == "private-publish" then .packages += [{name:"test-app",version:$v,publish:null,dependencies:[]}]
  elif $plant == "mixed-versions" then .packages[1].version = "0.2.0"
  elif $plant == "wrong-dependency" then .packages[0].dependencies = [{name:"rivium-error",kind:null,req:"^9.0.0"}]
  elif $plant == "private-dependency" then .packages[0].dependencies = [{name:"test-app",kind:null,path:"../test-app",req:"^0.0.0"}]
  else . end'
CARGO
cat > "$work/bin/rustc" <<'RUSTC'
#!/bin/sh
echo 'rustc 1.92.0 (fixture)'
RUSTC
cat > "$work/bin/git-cliff" <<'CLIFF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$*" = --version ]; then echo 'git-cliff 2.14.2'; exit 0; fi
[[ "$*" != *--latest* && "$1" == *..HEAD ]] || exit 92
while [ "$#" -gt 0 ]; do
  if [ "$1" = --output ]; then out=$2; fi
  shift
done
: > "$out"
for heading in Added Changed Fixed Breaking 'Platforms and upgrades'; do
  if [ "${PLANT:-}" = bad-notes ] && [ "$heading" = Breaking ]; then continue; fi
  printf '### %s\nNone.\n' "$heading" >> "$out"
done
CLIFF
cat > "$work/bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
[ "${PLANT:-}" != transport ] || { echo "planted transport failure" >&2; exit 7; }
while [ "$#" -gt 0 ]; do
  if [ "$1" = --output ]; then file=$2; shift; else url=$1; fi
  shift
done
version=${url##*/}; prefix=${url%/*}; crate=${prefix##*/}
status=${HTTP_STATUS:-404}
if [ "$status" = 200 ]; then
  checksum=$(shasum -a 256 "$PACKAGES/$crate-$version.crate" | cut -d ' ' -f 1)
  [ "${PLANT:-}" != wrong-checksum ] || checksum=$(printf '%064d' 0)
  jq -n --arg n "$crate" --arg v "$version" --arg sum "$checksum" '{version:{crate:$n,num:$v,yanked:false,checksum:$sum}}' > "$file"
else echo '{}' > "$file"; fi
printf '%s' "$status"
CURL
printf '#!/bin/sh\nexit 0\n' > "$work/bin/sleep"
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH" PACKAGES="$work/packages"
repo="$work/repo"
git -C "$repo" init -q
git -C "$repo" config user.name 'Release Test'
git -C "$repo" config user.email test@example.invalid
git -C "$repo" config core.hooksPath /dev/null
git -C "$repo" config commit.gpgsign false
git -C "$repo" config tag.gpgsign false
git -C "$repo" add .
git -C "$repo" commit -qm 'feat: baseline'
git -C "$repo" tag v0.1.0
printf 'candidate\n' > "$repo/change"
git -C "$repo" add change
git -C "$repo" commit -qm 'fix: candidate'
git -C "$repo" update-ref refs/remotes/origin/main HEAD
git -C "$repo" tag v0.1.1
check="$repo/scripts/release-check.sh"
"$check" tag v0.1.1 "$work/release" > "$work/log" 2>&1 || { cat "$work/log" >&2; exit 1; }
base=$(git -C "$repo" rev-parse v0.1.0)
git -C "$repo" tag 0.1.0 "$base"
git -C "$repo" tag -d v0.1.0 > /dev/null
if "$check" tag v0.1.1 "$work/bare-tag" > "$work/log" 2>&1; then
  echo 'accepted a non-version-tag baseline' >&2; exit 1
fi
grep -Fq 'reachable stable release baseline' "$work/log"
git -C "$repo" tag v0.1.0 "$base"
VERSION=0.1.1 "$check" preview "$work/preview" > "$work/log" 2>&1
jq -e '.version == "0.1.1" and .notes_version == "0.1.2"' "$work/preview/release.json" >/dev/null
reject() { # <reason> <command...>
  local reason=$1; shift
  if "$@" > "$work/log" 2>&1; then echo "accepted: $reason" >&2; exit 1; fi
  grep -Fq "$reason" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
printf '%s\n' '{"protection_rules":[{"type":"required_reviewers","reviewers":[{"type":"User","reviewer":{"id":1}}]}]}' > "$work/environment.json"
"$check" environment "$work/environment.json" > "$work/log" 2>&1
for protection in '{}' '{"protection_rules":[]}' '{"protection_rules":[{"type":"wait_timer"}]}' '{"protection_rules":[{"type":"required_reviewers","reviewers":[]}]}' '{"protection_rules":[{"type":"required_reviewers","reviewers":[null]}]}'; do
  printf '%s\n' "$protection" > "$work/environment.json"
  reject 'no required reviewer' "$check" environment "$work/environment.json"
done
PLANT=private-publish reject 'exactly the five public crates' "$check" preview "$work/bad"
PLANT=mixed-versions reject 'one version' "$check" preview "$work/bad"
PLANT=wrong-dependency reject 'dependency versions' "$check" preview "$work/bad"
PLANT=private-dependency reject 'outside the public workspace' "$check" preview "$work/bad"
PLANT=bad-notes reject 'unambiguous Breaking' "$check" preview "$work/bad"
reject 'tag and workspace version differ' "$check" tag v0.2.0 "$work/bad"
printf dirty >> "$repo/change"
reject 'checkout is dirty' "$check" tag v0.1.1 "$work/bad"
git -C "$repo" restore change
git -C "$repo" update-ref refs/remotes/origin/main v0.1.0
reject 'not on origin/main' "$check" tag v0.1.1 "$work/bad"
git -C "$repo" update-ref refs/remotes/origin/main HEAD
# A tag from another history must not become the notes baseline.
git -C "$repo" branch other v0.1.0
git -C "$repo" switch -q other
printf 'unrelated\n' > "$repo/unrelated"
git -C "$repo" add unrelated
git -C "$repo" commit -qm 'feat: unrelated'
git -C "$repo" tag v9.9.9
git -C "$repo" switch -q --detach v0.1.1
"$check" preview "$work/unrelated-preview" > "$work/log" 2>&1
jq -e '.baseline == "v0.1.1"' "$work/unrelated-preview/release.json" >/dev/null
git -C "$repo" tag v0.1.2
VERSION=0.1.2 reject 'platform verification record is missing' "$check" tag v0.1.2 "$work/missing-platform"
git -C "$repo" tag -d v0.1.2 > /dev/null
printf '\n## 0.2.0\nFixture only.\n' >> "$repo/docs/platforms.md"
git -C "$repo" add docs/platforms.md
git -C "$repo" commit -qm 'docs: candidate record fixture'
git -C "$repo" update-ref refs/remotes/origin/main HEAD
git -C "$repo" tag v0.2.0
VERSION=0.2.0 reject 'upgrading section' "$check" tag v0.2.0 "$work/missing-upgrade"
reject 'tag does not identify HEAD' "$check" tag v0.1.1 "$work/wrong-head"
HTTP_STATUS=500 reject 'HTTP 500' "$repo/scripts/release-registry.sh" absent 0.1.1
PLANT=transport reject 'planted transport failure' "$repo/scripts/release-registry.sh" absent 0.1.1

# Real tiny tarballs test package membership, extraction boundaries, privacy and checksums.
for name in rivium-error rivium rivium-http rivium-jni rivium-test; do
  dir="$work/src/$name-0.1.1"
  mkdir -p "$dir/src"
  printf '[package]\nname="%s"\nversion="0.1.1"\n' "$name" > "$dir/Cargo.toml"
  printf 'pub fn public_api() {}\n' > "$dir/src/lib.rs"
  tar -czf "$work/packages/$name-0.1.1.crate" -C "$work/src" "$name-0.1.1"
done
pack="$repo/scripts/release-packages.sh"
"$pack" scan "$work/packages" "$work/release" "$work/denylist" "$work/artifacts" > "$work/log" 2>&1
"$pack" verify "$work/packages" "$work/release" "$work/artifacts" > "$work/log" 2>&1
mkdir -p "$work/nested/tmp-crate"
cp "$work/packages/"*.crate "$work/nested/tmp-crate/"
"$pack" verify "$work/nested" "$work/release" "$work/artifacts" > "$work/log" 2>&1
cp "$work/packages/rivium-0.1.1.crate" "$work/nested/"
"$pack" verify "$work/nested" "$work/release" "$work/artifacts" > "$work/log" 2>&1
printf changed >> "$work/nested/rivium-0.1.1.crate"
reject 'ambiguous package output' "$pack" verify "$work/nested" "$work/release" "$work/artifacts"
HTTP_STATUS=200 "$repo/scripts/release-registry.sh" published 0.1.1 "$work/packages" > "$work/log" 2>&1
HTTP_STATUS=200 reject 'not absent' "$repo/scripts/release-registry.sh" absent 0.1.1
HTTP_STATUS=200 PLANT=wrong-checksum reject 'checksum differs' "$repo/scripts/release-registry.sh" published 0.1.1 "$work/packages"
HTTP_STATUS=404 reject 'unconfirmed' "$repo/scripts/release-registry.sh" published 0.1.1 "$work/packages"
cp "$work/artifacts/SHA256SUMS" "$work/hashes"
sed '$d' "$work/hashes" > "$work/artifacts/SHA256SUMS"
reject 'incomplete or extra checksum' "$pack" verify "$work/packages" "$work/release" "$work/artifacts"
cp "$work/hashes" "$work/artifacts/SHA256SUMS"
printf 'changed' >> "$work/packages/rivium-0.1.1.crate"
reject 'FAILED' "$pack" verify "$work/packages" "$work/release" "$work/artifacts"
cp "$work/artifacts/rivium-0.1.1.crate" "$work/packages/"
cp "$work/packages/rivium-0.1.1.crate" "$work/packages/extra.crate"
reject 'extra or stale' "$pack" scan "$work/packages" "$work/release" "$work/denylist" "$work/extra"
rm "$work/packages/extra.crate"
printf 'private-release-marker\n' >> "$work/src/rivium-0.1.1/src/lib.rs"
tar -czf "$work/packages/rivium-0.1.1.crate" -C "$work/src" rivium-0.1.1
reject 'check-public:' "$pack" scan "$work/packages" "$work/release" "$work/denylist" "$work/private"
cp "$work/artifacts/rivium-0.1.1.crate" "$work/packages/"
ln -s /outside "$work/src/rivium-0.1.1/src/link"
tar -czf "$work/packages/rivium-0.1.1.crate" -C "$work/src" rivium-0.1.1
reject 'link or special file' "$pack" scan "$work/packages" "$work/release" "$work/denylist" "$work/link"

tar -czf "$work/packages/rivium-0.1.1.crate" -C "$work/src" rivium-0.1.1/Cargo.toml rivium-0.1.1/src/lib.rs rivium-0.1.1/src/lib.rs
reject 'duplicate archive path' "$pack" scan "$work/packages" "$work/release" "$work/denylist" "$work/duplicate"

# The privileged path must remain tag-only; all validation is required and file-based.
workflow="$root/.github/workflows/release.yml"
grep -Fq 'needs: [check, semver, public, ci, daily]' "$workflow"
awk '/^  publish:$/ {inside=1; next} inside && /^  [^ ]/ {exit} inside {print}' "$workflow" > "$work/publish-job"
grep -Fq 'needs: [check, ready]' "$work/publish-job"
[ "$(grep -c 'id-token: write' "$workflow")" -eq 1 ]
grep -Fq 'id-token: write' "$work/publish-job"
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu; do
  grep -Fq "target: $target," "$root/.github/workflows/ci.yml"
done
grep -Fq 'environment: crates-io' "$workflow"
grep -Fq 'artifact-ids:' "$workflow"
grep -Fq 'cargo publish --dry-run --workspace' "$workflow"
# shellcheck disable=SC2016 # Match the literal workflow variable.
grep -Fq -- '--notes-file "$RUNNER_TEMP/verified-release/notes.md"' "$workflow"
# shellcheck disable=SC2016
grep -Fq "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')" "$workflow"
if grep -Eq 'gh (workflow run|run list)|FIRST_RELEASE_TOKEN|continue-on-error:' "$workflow"; then
  echo 'release workflow weakened its candidate or failure boundary' >&2; exit 1
fi
echo 'Release source, registry, package and workflow self-tests: passed (no uploads)'
