#!/usr/bin/env bash
# Local reproduction of cross/android: non-JNI tests under the bionic runner, then the JNI cdylib
# linked for 16 KB pages and its LOAD segment alignment. (CI links the cdylib with the NDK's
# API 30 clang; the cross image links for API 28, which does not affect alignment.)
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
check_cross
target=aarch64-linux-android
cross "+$RIVIUM_CROSS_TOOLCHAIN" test --locked --workspace --target "$target" --target-dir "target/cross/$target"
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" cross "+$RIVIUM_CROSS_TOOLCHAIN" build --locked --release \
  -p smoke-jni --target "$target" --target-dir target/cross/android-16k
zig "scripts/check-align.sh 16384 target/cross/android-16k/$target/release/libsmoke_jni.so"
