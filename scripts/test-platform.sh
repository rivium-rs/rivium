#!/usr/bin/env bash
# Run real cross-target tests; JVM and build-tool contracts remain in native CI.
set -euo pipefail
fail() { echo "platform: $*" >&2; exit 1; }
[ "$#" -eq 1 ] || { echo 'usage: test-platform.sh <target>' >&2; exit 2; }
target=$1
case "$target" in
  armv7-unknown-linux-gnueabihf | loongarch64-unknown-linux-gnu) mode=qemu; suffix=; library=libtest_jni.so ;;
  x86_64-pc-windows-gnu) mode=wine; suffix=.exe; library=test_jni.dll ;;
  *) fail "unsupported target: $target" ;;
esac
[ "$(uname -s):$(uname -m)" = Linux:x86_64 ] || fail 'requires a Linux x86_64 build host'
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || fail 'RUSTFLAGS must be unset; this script owns the target flags'
runner_name="CARGO_TARGET_$(tr '[:lower:]-' '[:upper:]_' <<< "$target")_RUNNER"
[ -n "${!runner_name:-}" ] || fail "$runner_name must select the target runner"
# Match the whitespace-separated runner convention used by rivium-test::process::command.
read -r -a runner <<< "${!runner_name}"
command -v "${runner[0]}" >/dev/null || fail "runner not found: ${runner[0]}"
command -v timeout >/dev/null || fail 'GNU timeout is required'
if [ "$mode" = qemu ]; then export RUSTFLAGS='--cfg qemu'; fi
export RUST_TEST_THREADS=1 CARGO_TERM_COLOR=never
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out="$root/target/platform"
logs="$out/logs/$target"
mkdir -p "$logs"
rm -f "$logs/build.log" "$logs/library.log" "$logs/process.log"
common=(--target "$target" --target-dir "$out" --locked)

# Build before testing, with the same target/profile/flags as the process tests.
timeout --kill-after=10s 900s cargo build -p test-app --bins -p test-jni --lib "${common[@]}" \
  < /dev/null 2>&1 | tee "$logs/build.log"
[ -s "$out/$target/debug/host$suffix" ] && [ -s "$out/$target/debug/$library" ] \
  || fail 'application or JNI library was not built'

# Avoid selecting JNI Invocation or tests-build: a target emulator is not a JDK/Cargo host.
timeout --kill-after=10s 900s cargo test -p rivium -p rivium-error -p rivium-http -p rivium-test \
  --lib --tests --all-features "${common[@]}" -- --test-threads=1 --color never \
  < /dev/null 2>&1 | tee "$logs/library.log"
timeout --kill-after=10s 900s cargo test -p tests-integration --lib --tests "${common[@]}" \
  -- --test-threads=1 --color never < /dev/null 2>&1 | tee "$logs/process.log"

for group in library process; do
  # A successful Cargo exit with no tests, failed/filtered results, or only zero-test harnesses is not proof.
  awk '/^test result:/ {
    if ($0 !~ /^test result: ok\. [0-9]+ passed; 0 failed; [0-9]+ ignored; 0 measured; 0 filtered out;/) exit 1
    passed += $4; summaries++
  } END {if (!summaries || !passed) exit 1}' "$logs/$group.log" || fail "$group tests did not complete"
  awk '/^test .* \.\.\. ignored/ {print $2}' "$logs/$group.log" | LC_ALL=C sort > "$logs/$group.ignored"
  : > "$logs/$group.expected"
  if [ "$mode:$group" = qemu:process ]; then
    printf '%s\n' a_second_signal_aborts_with_its_own_number host_deadline_exits_124 \
      sigint_stops_with_0 sigquit_terminates_by_signal_not_exit_code sigterm_stops_with_0_and_flushes_the_log \
      | LC_ALL=C sort > "$logs/$group.expected"
  fi
  cmp -s "$logs/$group.expected" "$logs/$group.ignored" || fail "unexpected $group ignored tests"
  ignored=$(wc -l < "$logs/$group.ignored" | tr -d ' ')
  reported=$(awk '/^test result:/ {n += $8} END {print n+0}' "$logs/$group.log")
  [ "$ignored" -eq "$reported" ] || fail "$group ignored count does not match named tests"
done

# These contracts launch target child processes, not just the top-level test runner.
for name in tests::roots_are_isolated_and_removed host_fault_build_exits_69 host_fault_ready_exits_69 \
  host_fault_run_exits_70 a_log_directory_that_cannot_be_created_exits_73 \
  version_and_help_exit_0_without_starting_or_writing_files unknown_arguments_exit_64_without_starting \
  invalid_configuration_exits_78_without_starting configuration_commands_are_read_only; do
  grep -Fxq "test $name ... ok" "$logs/process.log" || fail "required process contract did not pass: $name"
done
echo "platform: $target library and child-process contracts passed; $mode exclusions checked"
