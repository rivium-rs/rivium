#!/usr/bin/env bash
# ci/template: generates a project from template/ with cargo-generate, points its rivium
# dependencies at this workspace with [patch.crates-io], then builds and tests it.
# Usage: scripts/template-smoke.sh [<output dir>]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$(mktemp -d)}
name=template-smoke
rm -rf "${out:?}/$name"
cargo generate --path "$root/template" --name "$name" --destination "$out" --vcs none --silent
cd "$out/$name"
patch=$(
  echo
  echo "[patch.crates-io]"
  for crate in rivium-error rivium rivium-http rivium-jni rivium-test; do
    if grep -q "^$crate = " Cargo.toml; then echo "$crate = { path = \"$root/crates/$crate\" }"; fi
  done
)
printf '%s\n' "$patch" >> Cargo.toml
cargo test
cargo run --quiet
