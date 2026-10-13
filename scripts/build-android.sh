#!/usr/bin/env bash
# API 30 is the platform floor; this is linking/ELF validation, not an ART/device test.
set -euo pipefail
[ "$#" -eq 0 ] || { echo 'usage: build-android.sh' >&2; exit 2; }
fail() { echo "android: $*" >&2; exit 1; }
: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to NDK 28.1.13356709}"
[[ "$ANDROID_NDK_HOME" = /* ]] || fail 'ANDROID_NDK_HOME must be absolute'
grep -Eq '^Pkg.Revision[[:space:]]*=[[:space:]]*28\.1\.13356709[[:space:]]*$' "$ANDROID_NDK_HOME/source.properties" \
  || fail 'NDK version must be 28.1.13356709'
[ "$(cargo ndk --version)" = 'cargo-ndk 4.1.2' ] || fail 'cargo-ndk version must be 4.1.2'
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || fail 'RUSTFLAGS must be unset; this script owns the target flags'
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out="$root/target/android"
# Keep the linker requirement explicit rather than depending on an NDK default.
export RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384'
# Honor one selected NDK consistently even if the calling shell has a stale legacy variable.
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
cargo ndk -t arm64-v8a --platform 30 build --workspace --all-features --lib --bins --tests \
  --target-dir "$out" --locked
scripts/check-android.sh "$out/aarch64-linux-android/debug/libtest_jni.so"
