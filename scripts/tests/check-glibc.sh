#!/usr/bin/env bash
# Planted reader failures and version/ELF violations; no cross toolchain needed for these unit checks.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf 'fixture\n' > "$work/artifact"
cp "$work/artifact" "$work/libfixture.so"
cat > "$work/readelf" <<'READER'
#!/usr/bin/env bash
set -eu
[ "${CASE:-}" != reader-failure ] || exit 9
[ "${CASE:-}" != empty ] || exit 0
[ "${CASE:-}" != warning ] || echo 'readelf: malformed section' >&2
cat <<ELF
  Class:                             ${ELF_CLASS:-ELF64}
  Type:                              ${ELF_TYPE:-DYN} (Position-Independent Executable file)
  Machine:                           ${MACHINE:-Advanced Micro Devices X86-64}
      [Requesting program interpreter: ${INTERPRETER:-/lib64/ld-linux-x86-64.so.2}]
  0x0000000000000001 (NEEDED)          Shared library: [${LIBC:-libc.so.6}]
ELF
if [ "${CASE:-}" != no-versions ]; then
  for version in ${VERSIONS:-2.2.5 2.17}; do
    printf '  1: 0 0 FUNC GLOBAL DEFAULT UND function@GLIBC_%s (2)\n' "$version"
    printf '  000:   2 (GLIBC_%s)\n' "$version"
    if [ "${CASE:-}" != no-version-names ]; then
      printf '  0x0010:   Name: GLIBC_%s  Flags: none  Version: 2\n' "$version"
    fi
  done
fi
READER
chmod +x "$work/readelf"
export READELF="$work/readelf"
pass() { "$root/scripts/check-glibc.sh" "$@" > "$work/log" 2>&1 || { cat "$work/log" >&2; exit 1; }; }
reject() { # <diagnostic> <artifacts...>
  local reason=$1
  shift
  if "$root/scripts/check-glibc.sh" "$@" > "$work/log" 2>&1; then
    echo "expected rejection: $reason" >&2; exit 1
  fi
  grep -Fq "$reason" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
pass "$work/artifact" "$work/libfixture.so"
VERSIONS='2.9 2.10 2.17' pass "$work/artifact"
grep -Fq 'at most GLIBC_2.17' "$work/log"
VERSIONS=2.18 reject 'exceeds GLIBC_2.17' "$work/artifact"
VERSIONS=2.100 reject 'exceeds GLIBC_2.17' "$work/artifact"
VERSIONS=2.17.1 reject 'exceeds GLIBC_2.17' "$work/artifact"
VERSIONS=2.18446744073709551618 reject 'unrecognized version' "$work/artifact"
VERSIONS=PRIVATE reject 'unrecognized version' "$work/artifact"
VERSIONS=ABI_DT_RELR reject 'unrecognized version' "$work/artifact"
# Keep unknown suffixes intact even when a valid version is also present.
for version in '2.2-5' '2.2+5' '2.2/5' '2.2(5' '2.2)5' '2.2@5'; do
  VERSIONS="2.17 $version" reject "unrecognized version GLIBC_$version:" "$work/artifact"
done
ELF_CLASS=ELF32 reject 'not ELF64' "$work/artifact"
ELF_TYPE=REL reject 'not an executable or shared object' "$work/artifact"
MACHINE=AArch64 reject 'wrong machine' "$work/artifact"
INTERPRETER=/lib/ld-musl-x86_64.so.1 reject 'wrong or missing interpreter' "$work/artifact"
LIBC=libother.so reject 'missing dynamic libc dependency' "$work/artifact"
CASE=no-versions reject 'no GLIBC symbol versions' "$work/artifact"
CASE=no-version-names reject 'no GLIBC symbol versions' "$work/artifact"
CASE=reader-failure reject 'readelf failed' "$work/artifact"
CASE=empty reject 'not ELF64' "$work/artifact"
CASE=warning reject 'readelf diagnostics' "$work/artifact"
READELF="$work/missing-readelf" reject 'readelf is missing' "$work/artifact"
reject 'missing or empty artifact' "$work/missing"
: > "$work/empty"
reject 'missing or empty artifact' "$work/empty"
reject 'usage:'

# Exercise inventories with synthetic Cargo messages, including successful-but-empty output.
mkdir -p "$work/repo/scripts" "$work/bin"
cp "$root/scripts/glibc.sh" "$root/scripts/check-glibc.sh" "$work/repo/scripts/"
cat > "$work/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1" = metadata ]; then
  echo '{"packages":[{"name":"tests-build","id":"tests-build"}]}'
  exit 0
fi
[ "${BUILD_CASE:-}" != failed ] || exit 7
[ "${BUILD_CASE:-}" != incomplete ] || exit 0
if [ "${BUILD_CASE:-}" = empty ]; then
  echo '{"reason":"build-finished","success":true}'
  exit 0
fi
out="$PWD/target/glibc/x86_64-unknown-linux-gnu/debug"
mkdir -p "$out/deps"
emit() {
  local name=$1 file=$2 kind=$3 test=$4 package=$5
  printf 'fixture\n' > "$file"
  jq -nc --arg name "$name" --arg file "$file" --arg kind "$kind" --argjson test "$test" --arg package "$package" \
    '{reason:"compiler-artifact",package_id:$package,profile:{test:$test},
      target:{name:$name,kind:[$kind],crate_types:[$kind]},filenames:[$file],
      executable:(if $kind == "cdylib" then null else $file end)}'
}
if [[ " $* " == *' --package '* ]]; then
  emit host "$out/host" bin false test-app
else
  if [ "${BUILD_CASE:-}" != missing-jni ]; then
    emit test_jni "$out/libtest_jni.so" cdylib false test-jni
  fi
  for name in process signal_term signal_int signal_quit signal_second invocation consumers; do
    if [ "${BUILD_CASE:-}" = missing-signal ] && [ "$name" = signal_second ]; then continue; fi
    package=tests-integration
    [ "$name" != consumers ] || package=tests-build
    emit "$name" "$out/deps/$name-abcd" test true "$package"
  done
fi
echo '{"reason":"build-finished","success":true}'
CARGO
chmod +x "$work/bin/cargo"
export PATH="$work/bin:$PATH"
build="$work/repo/scripts/glibc.sh"
"$build" build > "$work/log" 2>&1 || { cat "$work/log" >&2; exit 1; }
[ "$(wc -l < "$work/repo/target/glibc/runtime-tests.txt" | tr -d ' ')" -eq 5 ]
for case in failed incomplete empty missing-jni missing-signal; do
  if BUILD_CASE=$case "$build" build > "$work/log" 2>&1; then
    echo "glibc accepted build case: $case" >&2; exit 1
  fi
  if BUILD_CASE=$case "$build" run > "$work/log" 2>&1; then
    echo "glibc allowed a runtime pass after build case: $case" >&2; exit 1
  fi
done

# Run the actual container payload with fake tools; this is not a CentOS runtime proof.
sed -n "/<<'RUNTIME'/,/^RUNTIME$/p" "$root/scripts/glibc.sh" | sed '1d;$d' > "$work/runtime.sh"
[ -s "$work/runtime.sh" ]
printf '#!/bin/sh\necho x86_64\n' > "$work/bin/uname"
cat > "$work/bin/getconf" <<'GETCONF'
#!/bin/sh
echo "${FAKE_LIBC:-glibc 2.17}"
GETCONF
printf '#!/bin/sh\nshift 2\nexec "$@"\n' > "$work/bin/timeout"
cat > "$work/test" <<'TEST'
#!/usr/bin/env bash
[ "${RUNTIME_CASE:-}" != failed ] || exit 9
[ "${RUNTIME_CASE:-}" != no-summary ] || exit 0
if [ "${RUNTIME_CASE:-}" = drain-input ]; then cat > /dev/null; fi
case "${RUNTIME_CASE:-}" in
  zero) echo 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;' ;;
  skipped) echo 'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;' ;;
  *) echo 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;' ;;
esac
TEST
chmod +x "$work/bin/uname" "$work/bin/getconf" "$work/bin/timeout" "$work/test"
printf '%s\n' "$work/test" > "$work/tests.txt"
bash "$work/runtime.sh" "$work/tests.txt" > "$work/log" 2>&1
printf '%s\n%s\n' "$work/test" "$work/test" > "$work/two-tests.txt"
RUNTIME_CASE=drain-input bash "$work/runtime.sh" "$work/two-tests.txt" > "$work/log" 2>&1
grep -Fq '2 tests passed' "$work/log"
for case in failed no-summary zero skipped; do
  if RUNTIME_CASE=$case bash "$work/runtime.sh" "$work/tests.txt" > "$work/log" 2>&1; then
    echo "glibc accepted runtime case: $case" >&2; exit 1
  fi
done
if FAKE_LIBC='glibc 2.28' bash "$work/runtime.sh" "$work/tests.txt" > "$work/log" 2>&1; then
  echo 'glibc accepted the wrong runtime' >&2; exit 1
fi
echo 'GLIBC parser, artifact inventory and runtime payload self-tests: passed'
