#!/usr/bin/env bash
# Fails when a manifest or a cargo configuration asks for panic = "abort": Rivium's hosts turn a
# panic into a service failure (and a JNI result code), which an aborting panic makes impossible.
# Usage: scripts/check-panic.sh (part of `just check`)
set -euo pipefail
cd "$(dirname "$0")/.."
files=$(find . -path ./target -prune -o \( -name Cargo.toml -o -path '*/.cargo/config.toml' -o -path '*/.cargo/config' \) -print)
# shellcheck disable=SC2086
if grep -nE '^[[:space:]]*panic[[:space:]]*=[[:space:]]*"abort"|panic=abort' $files; then
  echo "check-panic: keep panic = \"unwind\" in every profile (see AGENTS.md)" >&2
  exit 1
fi
echo "check-panic: every profile unwinds"
