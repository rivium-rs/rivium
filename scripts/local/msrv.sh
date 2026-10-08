#!/usr/bin/env bash
# Local reproduction of ci/msrv: installs Rust 1.92 into .tools/rustup (ignored by git; nothing
# outside the repository changes) and runs the workspace tests with the locked dependencies.
# shellcheck source=scripts/local/common.sh
. "$(dirname "$0")/common.sh"
export RUSTUP_HOME="$root/.tools/rustup"
rustup toolchain install 1.92 --profile minimal --no-self-update
rustup run 1.92 cargo test --workspace --all-features --locked --target-dir target/local/msrv
