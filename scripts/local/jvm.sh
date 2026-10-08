#!/usr/bin/env bash
# Local reproduction of the desktop-JVM step of ci/test-linux: builds the JNI smoke cdylib for
# x86_64 Linux and runs the JVM harness (including the planted signature mismatch) in a JDK 17 image.
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
zig "cargo zigbuild --locked --release -p smoke-jni --target x86_64-unknown-linux-gnu.2.17"
docker run --rm --platform linux/amd64 -v "$root":/w -w /w --entrypoint bash "$RIVIUM_JDK_IMAGE" \
  examples/smoke-jni/jvm/run.sh target/local/zig/x86_64-unknown-linux-gnu/release
