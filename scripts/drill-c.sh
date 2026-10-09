#!/usr/bin/env bash
# Scenario C (V-12): a service built on Rivium gains a background service, a program and a
# configuration section without any change to Rivium. Applies fixtures/scenario-c.patch, which
# adds them to examples/multi-bin, to a copy of this repository's files; asserts that every path
# it changes is the example's code (its business modules and composition roots below src/) or
# its tests, and nothing else: no Rivium crate, rule, manifest or lockfile; then runs the
# example's tests with --locked, including the new program's lifecycle contract.
# With --plant, the copy also changes a file of rivium, as if the scenario had needed it: the
# path check must then fail.
# Usage: scripts/drill-c.sh [--plant]
# shellcheck disable=SC2001
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
plant=false
[ "${1:-}" = --plant ] && plant=true
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
copy=$work/repo

# The repository's files as they are (committed or not), in a new git repository.
mkdir -p "$copy"
(cd "$root" && git ls-files -z --cached --others --exclude-standard) | (cd "$root" && xargs -0 tar -cf -) | tar -xf - -C "$copy"
git -C "$copy" init -q
git -C "$copy" add -A
git -C "$copy" -c user.name=drill -c user.email=drill@example.com commit -qm before

git -C "$copy" apply --index "$root/fixtures/scenario-c.patch"
if $plant; then
  echo "// planted" >> "$copy/crates/rivium/src/lib.rs"
  git -C "$copy" add crates/rivium/src/lib.rs
fi
changed=$(git -C "$copy" diff --cached --name-only)
echo "drill-c: the scenario changes"
sed 's/^/  /' <<< "$changed"
outside=$(grep -vE '^examples/multi-bin/(src|tests)/' <<< "$changed" || true)
if [ -n "$outside" ]; then
  echo "drill-c: the scenario changes more than the example's code and tests:" >&2
  sed 's/^/  /' <<< "$outside" >&2
  exit 1
fi
echo "drill-c: only the example's code and tests change"

# The example's tests, the new program's lifecycle contract among them, with the lockfile as it is.
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target/drill-c} \
  cargo test --locked --manifest-path "$copy/Cargo.toml" -p multi-bin
echo "drill-c: passed"
