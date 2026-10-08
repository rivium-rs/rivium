# shellcheck shell=bash
# Shared settings for scripts/local/*.sh, which reproduce CI jobs locally with Docker (for example
# OrbStack on macOS). Image names default to public images; to use images you already have, set
# the variables below in .tools/local.env (ignored by git), e.g. RIVIUM_JDK_IMAGE=my/jdk:17.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
# shellcheck disable=SC1091
[ ! -f .tools/local.env ] || . .tools/local.env
# Rust + zig + cargo-zigbuild (same cargo-zigbuild version as CI).
: "${RIVIUM_ZIG_IMAGE:=ghcr.io/rust-cross/cargo-zigbuild:0.23.4}"
# Any linux/amd64 image with a JDK 17 (javac and java) and bash.
: "${RIVIUM_JDK_IMAGE:=eclipse-temurin:17-jdk}"
# Toolchain whose x86_64-unknown-linux-gnu build cross mounts into its images.
: "${RIVIUM_CROSS_TOOLCHAIN:=stable}"
# Crate cache for containers, so they never write to ~/.cargo.
registry="$root/.tools/docker-cargo/registry"
mkdir -p "$registry"
cross_rev=65fe72b0cdb1e7e0cc0652517498d4389cc8f5cf # keep in sync with .github/workflows/cross.yml

zig() { # <command>: runs a shell command in the zig image, with the workspace at /w
  docker run --rm -v "$root":/w -w /w -v "$registry":/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/w/target/local/zig "$RIVIUM_ZIG_IMAGE" sh -c "$1"
}

check_cross() {
  command -v cross > /dev/null || { echo "cross is required: cargo install cross --git https://github.com/cross-rs/cross --rev $cross_rev" >&2; exit 2; }
  cross --version 2>/dev/null | grep -q "${cross_rev:0:7}" \
    || echo "warning: CI pins cross at ${cross_rev:0:7}; this cross is $(cross --version 2>/dev/null | head -n 1)" >&2
}
