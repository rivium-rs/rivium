#!/usr/bin/env bash
# Local reproduction of ci/slices in a Debian image with Rust (RIVIUM_RUST_IMAGE): installs a JDK
# 17, net-snmp's tools and jq, then checks the slices' packages, runs their tests, the business
# tests through the embedded host in a JVM, rivium-jni's JVM contract on snmp-lite-jni, and the
# interoperability check with net-snmp.
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
# The single quotes are the container's script.
# shellcheck disable=SC2016
docker run --rm -v "$root":/w -w /w -v "$registry":/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/w/target/local/slices "$RIVIUM_RUST_IMAGE" bash -c '
  set -euo pipefail
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends snmp openjdk-17-jdk-headless jq > /dev/null
  scripts/check-slice-deps.sh
  scripts/tests/check-slice-deps.sh
  cargo test --locked -p snmp-lite -p snmp-lite-bin -p snmp-lite-jni -p edge-lite -p edge-lite-bin
  cargo build --locked -p snmp-lite-jni
  SNMP_LITE_JNI=$CARGO_TARGET_DIR/debug cargo test --locked -p snmp-lite-bin --test hosts -- --ignored
  examples/slices/snmp-lite/snmp-lite-jni/jvm-test/run.sh "$CARGO_TARGET_DIR/debug"
  scripts/snmp-interop.sh "$CARGO_TARGET_DIR/debug/snmp-lite"'
