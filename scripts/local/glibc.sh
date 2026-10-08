#!/usr/bin/env bash
# Local reproduction of cross/glibc and the zigbuild step of cross/armv7: builds the smoke binary
# and cdylib against the glibc 2.17 baseline with cargo-zigbuild, checks their symbol versions,
# then runs `smoke check` from the build for the Docker host's architecture on CentOS 7 (glibc 2.17).
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
targets="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf"
zig "set -e
  for t in $targets; do rustup target add \$t > /dev/null 2>&1; done
  cargo zigbuild --locked --release -p smoke -p smoke-jni $(for t in $targets; do printf -- '--target %s.2.17 ' "$t"; done)
  for t in $targets; do
    scripts/check-glibc.sh 2.17 target/local/zig/\$t/release/smoke target/local/zig/\$t/release/libsmoke_jni.so
  done"
arch=$(docker info --format '{{.Architecture}}')
case "$arch" in arm64) arch=aarch64 ;; amd64) arch=x86_64 ;; esac
docker run --rm -v "$root/target/local/zig/$arch-unknown-linux-gnu/release":/b centos:7 /b/smoke check
