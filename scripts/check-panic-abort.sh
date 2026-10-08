#!/usr/bin/env bash
# rivium-jni refuses panic = "abort", with which a panic in a service would end the Java
# application: checking fixtures/jni-forbid with that profile must fail with rivium-jni's
# compile_error!. Fails when it builds, or fails for another reason.
# Usage: scripts/check-panic-abort.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
log=$(mktemp)
trap 'rm -f "$log"' EXIT
if CARGO_PROFILE_DEV_PANIC=abort cargo check --locked --manifest-path "$root/Cargo.toml" \
  -p jni-forbid --target-dir "$root/target/panic-abort" 2> "$log"; then
  echo "check-panic-abort: rivium-jni builds with panic = \"abort\"" >&2
  exit 1
fi
if ! grep -q 'rivium-jni needs panic = "unwind"' "$log"; then
  cat "$log" >&2
  echo "check-panic-abort: the check failed for another reason" >&2
  exit 1
fi
echo "check-panic-abort: rivium-jni refuses panic = \"abort\""
