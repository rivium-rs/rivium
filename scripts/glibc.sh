#!/usr/bin/env bash
# Build/inspect actual workspace artifacts, then optionally run them in the old userland.
set -euo pipefail
if [ "$#" -ne 1 ] || [[ "$1" != build && "$1" != run ]]; then
  echo 'usage: glibc.sh <build|run>' >&2
  exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out="$root/target/glibc"
triple=x86_64-unknown-linux-gnu
# The amd64 manifest of cross-rs 0.2.5-centos; the bare :centos tag does not exist.
image=ghcr.io/cross-rs/x86_64-unknown-linux-gnu@sha256:8642c6db188219b7d1f9768c08145e9263aabd57017e190165ac157708c422e2

if [ "$1" = build ]; then
  mkdir -p "$out"
  # Removing only our previous inventories prevents a failed build leaving runnable stale evidence.
  rm -f "$out/artifacts.txt" "$out/runtime-tests.txt"
  cargo zigbuild --package tests-integration --bins --all-features --locked \
    --target "$triple.2.17" --target-dir "$out" --message-format=json-render-diagnostics > "$out/bins.jsonl"
  # zigbuild wraps cargo build: --tests links test executables without running them (no --no-run).
  cargo zigbuild --workspace --all-features --lib --tests --locked \
    --target "$triple.2.17" --target-dir "$out" --message-format=json-render-diagnostics > "$out/tests.jsonl"
  for messages in "$out/bins.jsonl" "$out/tests.jsonl"; do
    jq -se '.[-1].reason == "build-finished" and .[-1].success == true' "$messages" >/dev/null \
      || { echo 'glibc: Cargo did not report a successful build' >&2; exit 1; }
  done
  jq -sr '[.[] | select(.reason == "compiler-artifact") |
    if .executable != null then .executable
    elif (.target.crate_types | index("cdylib")) then .filenames[] | select(endswith(".so"))
    else empty end] | unique[]' "$out/bins.jsonl" "$out/tests.jsonl" > "$out/artifacts.txt"
  # tests-build needs Cargo/tokei, and Invocation needs a JDK: their native CI jobs remain required.
  cargo metadata --no-deps --format-version 1 --locked > "$out/metadata.json"
  build_tests=$(jq -er '.packages[] | select(.name == "tests-build") | .id' "$out/metadata.json")
  jq -sr --arg build_tests "$build_tests" '[.[] | select(.reason == "compiler-artifact" and
    .profile.test and .executable != null and .package_id != $build_tests and
    .target.name != "invocation" and (.target.kind | index("bin") | not)) | .executable] | unique[]' \
    "$out/tests.jsonl" > "$out/runtime-tests.txt"
fi

if [ ! -s "$out/artifacts.txt" ] || [ ! -s "$out/runtime-tests.txt" ]; then
  echo 'glibc: missing artifact inventory; run glibc.sh build first' >&2
  exit 1
fi
# Check both inventories before invoking anything, including when reusing a build for the runtime pass.
for inventory in artifacts runtime-tests; do
  while IFS= read -r artifact; do
    [[ "$artifact" == "$out/$triple/"* ]] \
      || { echo "glibc: artifact is outside the target directory: $artifact" >&2; exit 1; }
    if [ "$inventory" = runtime-tests ]; then
      grep -Fxq "$artifact" "$out/artifacts.txt" \
        || { echo 'glibc: runtime test was not in the checked build' >&2; exit 1; }
    else
      "$root/scripts/check-glibc.sh" "$artifact"
    fi
  done < "$out/$inventory.txt"
done
# Nonempty inventories alone must not let a trivial, unrelated ELF stand in for Rivium's subjects.
for subject in test-host libtest_jni.so; do
  grep -Fxq "$out/$triple/debug/$subject" "$out/artifacts.txt" \
    || { echo "glibc: missing required subject: $subject" >&2; exit 1; }
done
for name in hosts lifecycle config log log_export export_compat fs task contract server vocabulary files; do
  grep -Eq "/deps/$name-[a-f0-9]+$" "$out/runtime-tests.txt" \
    || { echo "glibc: missing required runtime test: $name" >&2; exit 1; }
done
echo "glibc: checked $(wc -l < "$out/artifacts.txt" | tr -d ' ') artifacts"
[ "$1" = run ] || exit 0

# This lane proves native execution, not Docker's transparent cross-architecture emulation.
[ "$(uname -s):$(uname -m)" = Linux:x86_64 ] \
  || { echo 'glibc: runtime verification requires a native Linux x86_64 host' >&2; exit 1; }
docker run --rm -i --platform linux/amd64 --network none --ulimit core=0 \
  --mount "type=bind,source=$root,target=$root,readonly" --workdir "$root" \
  --entrypoint /bin/bash "$image" -s -- "$out/runtime-tests.txt" <<'RUNTIME'
set -euo pipefail
if [ "$(uname -m)" != x86_64 ] || [ "$(getconf GNU_LIBC_VERSION)" != 'glibc 2.17' ]; then
  echo 'glibc: the runtime is not x86_64 glibc 2.17' >&2
  exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
passed=0
while IFS= read -r executable; do
  echo "glibc 2.17 runtime: $executable"
  status=0
  timeout --kill-after=5s 120s "$executable" --test-threads=1 --color never < /dev/null > "$work/log" 2>&1 || status=$?
  cat "$work/log"
  [ "$status" -eq 0 ] || exit "$status"
  summary=$(grep -E '^test result: ok\. [0-9]+ passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;' "$work/log")
  count=$(sed -nE 's/^test result: ok\. ([0-9]+) passed;.*/\1/p' <<< "$summary")
  [[ "$count" =~ ^[0-9]+$ ]] || { echo 'glibc: missing unskipped test summary' >&2; exit 1; }
  passed=$((passed + count))
done < "$1"
[ "$passed" -gt 0 ] || { echo 'glibc: no tests ran' >&2; exit 1; }
echo "glibc 2.17 runtime: $passed tests passed without skips or retries"
RUNTIME
