#!/usr/bin/env bash
# Unit-check command wiring and false-green rejection; these fixtures do not run QEMU or Wine.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/repo/scripts"
cp "$root/scripts/test-platform.sh" "$work/repo/scripts/"
cat > "$work/bin/uname" <<'UNAME'
#!/bin/sh
if [ "$1" = -s ]; then echo "${HOST_OS:-Linux}"; else echo x86_64; fi
UNAME
cat > "$work/bin/timeout" <<'TIMEOUT'
#!/bin/sh
[ "${PLANT:-}" != timeout ] || { echo 'planted timeout'; exit 124; }
[ "$1" = --kill-after=10s ] && [ "$2" = 900s ] || exit 90
shift 2
exec "$@"
TIMEOUT
printf '#!/bin/sh\nexit 0\n' > "$work/bin/target-runner"
cat > "$work/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
args=" $* "
cmd=$1; target=; out=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) target=$2; shift ;;
    --target-dir) out=$2; shift ;;
  esac
  shift
done
[[ "$args" == *' --locked '* ]] && [ -n "$target" ] && [ -n "$out" ] || exit 91
[ "${CARGO_BUILD_TARGET:-}" = "$target" ] || exit 89
case "$target" in
  *-linux-*) [ "${RUSTFLAGS:-}" = '--cfg qemu' ] || exit 92; mode=qemu; suffix=; lib=libtest_jni.so ;;
  *-windows-gnu)
    [ -z "${RUSTFLAGS:-}" ] || exit 93
    [ "${WINEDEBUG:-}" = -all ] || { echo 'Wine diagnostics can corrupt test result lines'; exit 99; }
    mode=wine; suffix=.exe; lib=test_jni.dll ;;
esac
if [ "${PLANT:-}" = cargo-failure ]; then echo 'planted Cargo failure'; exit 9; fi
if [ "$cmd" = build ]; then
  [[ "$args" == *' -p tests-integration --bins -p test-jni --lib '* ]] || exit 94
  mkdir -p "$out/$target/debug"
  rm -f "$out/$target/debug/test-host$suffix" "$out/$target/debug/$lib"
  if [ "${PLANT:-}" != missing-artifacts ]; then
    printf 'fixture\n' > "$out/$target/debug/test-host$suffix"
    printf 'fixture\n' > "$out/$target/debug/$lib"
  fi
  exit 0
fi
[[ "$args" == *' --lib --tests '* && "$args" == *' -- --test-threads=1 --color never '* ]] || exit 95
[[ "$args" != *' -p rivium-jni '* && "$args" != *' -p tests-build '* ]] || exit 96
if [ "${PLANT:-}" = no-tests ]; then echo 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;'; exit 0; fi
if [ "${PLANT:-}" = filtered ]; then echo 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;'; exit 0; fi
if [[ "$args" != *' -p tests-integration '* ]]; then
  [[ "$args" == *' -p rivium -p rivium-util -p rivium-host -p rivium-http -p rivium-test '* && "$args" == *' --all-features '* ]] || exit 97
  echo 'test smoke ... ok'
  ignored=0
  for name in concurrent_lifecycle_stress surviving_descendants_restart_stress; do
    if [ "${PLANT:-}" = missing-stress-ignore ] && [ "$name" = concurrent_lifecycle_stress ]; then continue; fi
    echo "test $name ... ignored, opt-in sustained stress"; ignored=$((ignored+1))
  done
  if [ "$mode" = qemu ] || [ "${PLANT:-}" = wine-signal-ignore ]; then
    for name in process_handles_first_second_signals_and_shared_deadline stop_during_restart_backoff_exits_successfully_without_another_generation; do
      if [ "${PLANT:-}" = missing-library-signal-ignore ] && [ "$name" = process_handles_first_second_signals_and_shared_deadline ]; then continue; fi
      echo "test $name ... ignored, native signals"; ignored=$((ignored+1))
    done
  fi
  if [ "${PLANT:-}" = unexpected-ignore ]; then echo 'test other ... ignored'; ignored=$((ignored+1)); fi
  if [ "${PLANT:-}" = hidden-ignore ]; then ignored=$((ignored+1)); fi
  echo "test result: ok. 1 passed; 0 failed; $ignored ignored; 0 measured; 0 filtered out;"
  exit 0
fi
[[ "$args" != *' --all-features '* ]] || exit 98
passed=0
for name in embedded_repeated_start_stop embedded_startup_failures stop_during_start_returns_that_runs_result config_commands_are_read_only process_errors_have_stable_codes; do
  if [ "${PLANT:-}" = missing-contract ] && [ "$name" = process_errors_have_stable_codes ]; then continue; fi
  echo "test $name ... ok"; passed=$((passed+1))
done
ignored=0
if [ "$mode" = qemu ]; then
  for name in process_signals_and_deadline normal_sigterm_flushes_the_final_run_record_before_process_exit sigquit_retains_default_signal_termination; do
    if [ "${PLANT:-}" = missing-ignore ] && [ "$name" = process_signals_and_deadline ]; then continue; fi
    if [ "${PLANT:-}" = missing-new-process-ignore ] && [ "$name" = sigquit_retains_default_signal_termination ]; then continue; fi
    echo "test $name ... ignored, requires native Unix signal delivery"; ignored=$((ignored+1))
  done
fi
if [ "${PLANT:-}" = hidden-process-ignore ]; then ignored=$((ignored+1)); fi
echo "test result: ok. $passed passed; 0 failed; $ignored ignored; 0 measured; 0 filtered out;"
CARGO
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER=target-runner
export CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_RUNNER=target-runner
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=target-runner
run="$work/repo/scripts/test-platform.sh"
for target in armv7-unknown-linux-gnueabihf loongarch64-unknown-linux-gnu x86_64-pc-windows-gnu; do
  "$run" "$target" > "$work/log" 2>&1 || { cat "$work/log" >&2; exit 1; }
done
reject() {
  local reason=$1
  if "$run" armv7-unknown-linux-gnueabihf > "$work/log" 2>&1; then echo "accepted $reason" >&2; exit 1; fi
  grep -Fq "$reason" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
PLANT=cargo-failure reject 'planted Cargo failure'
PLANT=timeout reject 'planted timeout'
PLANT=missing-artifacts reject 'application or JNI library was not built'
PLANT=no-tests reject 'library tests did not complete'
PLANT=filtered reject 'library tests did not complete'
PLANT=unexpected-ignore reject 'unexpected library ignored tests'
PLANT=missing-stress-ignore reject 'unexpected library ignored tests'
PLANT=missing-library-signal-ignore reject 'unexpected library ignored tests'
PLANT=hidden-ignore reject 'library ignored count does not match'
PLANT=missing-contract reject 'required process contract did not pass'
PLANT=missing-ignore reject 'unexpected process ignored tests'
PLANT=missing-new-process-ignore reject 'unexpected process ignored tests'
PLANT=hidden-process-ignore reject 'process ignored count does not match'
if PLANT=wine-signal-ignore "$run" x86_64-pc-windows-gnu > "$work/log" 2>&1; then echo 'accepted Wine signal ignores' >&2; exit 1; fi
grep -Fq 'unexpected library ignored tests' "$work/log"
HOST_OS=Darwin reject 'requires a Linux x86_64 build host'
RUSTFLAGS='--cfg other' reject 'RUSTFLAGS must be unset'
CARGO_ENCODED_RUSTFLAGS='--cfg=other' reject 'RUSTFLAGS must be unset'
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER='' reject 'must select the target runner'
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER=missing-runner reject 'runner not found'
if "$run" unknown > "$work/log" 2>&1; then echo 'accepted unsupported target' >&2; exit 1; fi
grep -Fq 'unsupported target' "$work/log"
echo 'Platform driver self-tests: passed (fixtures, not emulator execution)'
