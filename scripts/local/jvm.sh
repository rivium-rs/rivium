#!/usr/bin/env bash
# Local reproduction of the desktop-JVM step of ci/test-linux: builds the beacon-jni library for
# x86_64 Linux, as is and with `--cfg rivium_jni_fault`, and runs rivium-jni's JVM contract
# (including the panics and the mismatched declaration) in a JDK 17 image.
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
target=x86_64-unknown-linux-gnu
zig "cargo zigbuild --locked -p beacon-jni --target $target.2.17 \
  && RUSTFLAGS='--cfg rivium_jni_fault' cargo zigbuild --locked -p beacon-jni --target $target.2.17 \
    --target-dir /w/target/local/zig-fault"
docker run --rm --platform linux/amd64 -v "$root":/w -w /w --entrypoint bash "$RIVIUM_JDK_IMAGE" \
  crates/rivium-jni/tests/jvm/run.sh "target/local/zig/$target/debug" beacon_jni com/example/beacon \
  "target/local/zig-fault/$target/debug"
