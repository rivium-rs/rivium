#!/usr/bin/env bash
# Planted ELF/NDK failures; real linking is a separate validation, not claimed by these fixtures.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/repo/scripts" "$work/ndk"
cp "$root/scripts/build-android.sh" "$root/scripts/check-android.sh" "$work/repo/scripts/"
printf 'fixture\n' > "$work/library.so"
cat > "$work/bin/readelf" <<'READER'
#!/usr/bin/env bash
set -eu
[ "${PLANT:-}" != reader-failure ] || exit 9
[ "${PLANT:-}" != empty-output ] || exit 0
[ "${PLANT:-}" != reader-warning ] || echo 'readelf: bad section' >&2
cat <<HEADER
  Class: ${CLASS:-ELF64}
  Data: 2's complement, ${ENDIAN:-little} endian
  Type: ${TYPE:-DYN}
  Machine: ${MACHINE:-AArch64}
  (NEEDED) Shared library: [${LIBC:-libc.so}]
HEADER
printf '  Android 0x00000084 NT_VERSION '
if [ "${PLANT:-}" = wrapped-note ]; then printf '\n'; fi
if [ "${PLANT:-}" = foreign-note ]; then printf '\n  Other 0x00000084 NT_VERSION '; fi
printf 'description data: %s 00 00 00 72 32 38 62\n' "${API:-1e}"
[ "${PLANT:-}" = no-export ] || echo "  1: 0 16 FUNC GLOBAL DEFAULT ${SECTION:-13} JNI_OnLoad"
if [ "${PLANT:-}" != no-load ]; then
  echo "  LOAD 0x0 0x0 0x0 0x400 0x400 R ${ALIGN:-0x4000}"
  echo "  LOAD ${OFFSET:-0x4000} 0x8000 0x8000 0x100 0x100 R E ${ALIGN:-0x4000}"
fi
READER
cat > "$work/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
if [ "$*" = 'ndk --version' ]; then echo "cargo-ndk ${TOOL_VERSION:-4.1.2}"; exit 0; fi
[[ " $* " == *' ndk -t arm64-v8a --platform 30 build --workspace --all-features --lib --bins --tests '* ]] || exit 90
[[ " $* " == *' --locked '* ]] || exit 91
[ "$ANDROID_NDK_ROOT" = "$ANDROID_NDK_HOME" ] || exit 92
[ "$RUSTFLAGS" = '-C link-arg=-Wl,-z,max-page-size=16384' ] || exit 93
[ "${PLANT:-}" != build-failure ] || { echo 'planted NDK build failure'; exit 9; }
out="$PWD/target/android/aarch64-linux-android/debug"
mkdir -p "$out"
rm -f "$out/libtest_jni.so"
if [ "${PLANT:-}" != no-library ]; then printf 'fixture\n' > "$out/libtest_jni.so"; fi
CARGO
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH" READELF="$work/bin/readelf" ANDROID_NDK_HOME="$work/ndk"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
printf 'Pkg.Revision = 28.1.13356709\n' > "$work/ndk/source.properties"
check="$work/repo/scripts/check-android.sh"
"$check" "$work/library.so" > "$work/log" 2>&1
PLANT=wrapped-note "$check" "$work/library.so" > "$work/log" 2>&1
reject() {
  local reason=$1
  if "$check" "$work/library.so" > "$work/log" 2>&1; then echo "accepted $reason" >&2; exit 1; fi
  grep -Fq "$reason" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
ALIGN=0x1000 reject 'LOAD alignment is below 16 KB'
ALIGN=0x6000 reject 'not a power of two'
ALIGN=unknown reject 'malformed LOAD'
OFFSET=0x4100 reject 'not congruent'
MACHINE=X86-64 reject 'wrong machine'
CLASS=ELF32 reject 'not ELF64'
TYPE=EXEC reject 'not a shared object'
ENDIAN=big reject 'not little-endian'
LIBC=libc.so.6 reject 'not linked to bionic'
API=1f reject 'wrong Android API 30 note'
PLANT=foreign-note reject 'wrong Android API 30 note'
SECTION=UND reject 'missing defined JNI_OnLoad'
PLANT=no-export reject 'missing defined JNI_OnLoad'
PLANT=no-load reject 'no LOAD segments'
PLANT=reader-failure reject 'readelf failed'
PLANT=reader-warning reject 'readelf diagnostics'
PLANT=empty-output reject 'not ELF64'
READELF="$work/missing" reject 'readelf is missing'
: > "$work/library.so"
reject 'missing or empty JNI library'

build="$work/repo/scripts/build-android.sh"
ANDROID_NDK_ROOT=/stale "$build" > "$work/log" 2>&1 || { cat "$work/log" >&2; exit 1; }
reject_build() {
  local reason=$1
  if "$build" > "$work/log" 2>&1; then echo "accepted $reason" >&2; exit 1; fi
  grep -Fq "$reason" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
PLANT=build-failure reject_build 'planted NDK build failure'
PLANT=no-library reject_build 'missing or empty JNI library'
TOOL_VERSION=0.0 reject_build 'cargo-ndk version must be 4.1.2'
RUSTFLAGS='--cfg wrong' reject_build 'RUSTFLAGS must be unset'
ANDROID_NDK_HOME=relative reject_build 'ANDROID_NDK_HOME must be absolute'
ANDROID_NDK_HOME='' reject_build 'set ANDROID_NDK_HOME'
printf 'Pkg.Revision = 28.2.0\n' > "$work/ndk/source.properties"
reject_build 'NDK version must be 28.1.13356709'
echo 'Android toolchain and ELF self-tests: passed (fixtures, not device execution)'
