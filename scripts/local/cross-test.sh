#!/usr/bin/env bash
# Local reproduction of the `cross test` step of cross/armv7, cross/loongarch64 and cross/android
# (qemu-user or the bionic runner, in the pinned images of Cross.toml); x86_64-pc-windows-gnu runs
# the tests under wine. Each target gets its own target dir: build scripts compiled in one image
# need that image's glibc.
# Usage: scripts/local/cross-test.sh <target>...
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
[ $# -ge 1 ] || { echo "usage: cross-test.sh <target>..." >&2; exit 2; }
check_cross
for target in "$@"; do
  cross "+$RIVIUM_CROSS_TOOLCHAIN" test --locked --workspace --target "$target" --target-dir "target/cross/$target"
done
