# Rivium workspace tasks; `just` lists them. Developer tools are installed under .tools/ (ignored
# by git) with `just tools`, never globally.

export PATH := justfile_directory() / ".tools/bin" + ":" + env_var("PATH")

libs := "rivium-error rivium rivium-http rivium-jni rivium-test"

default:
    @just --list

# Install the developer tools used by the recipes into .tools/
tools:
    cargo install --locked --root .tools cargo-hack@0.6.45 cargo-generate@0.25.0

# Formatting, clippy, cargo-deny, feature matrix, the panic = "abort" refusal and script self-tests (what ci/lint runs)
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo deny --locked check
    cargo hack check --each-feature --workspace --locked
    scripts/check-panic-abort.sh
    shellcheck scripts/*.sh scripts/hooks/* scripts/tests/*.sh scripts/local/*.sh scripts/hosting/*.sh crates/rivium-jni/tests/jvm/run.sh
    scripts/tests/check-public.sh
    scripts/tests/check-commit-msg.sh
    scripts/tests/docs-only.sh
    scripts/tests/loc.sh
    if command -v actionlint > /dev/null; then actionlint; else echo "actionlint not installed (see .tools/); skipped"; fi

# Workspace tests on this machine
test:
    cargo test --workspace --locked

# lint and test
check: lint test

# Tests with the MSRV (Rust 1.92, installed under .tools/), as ci/msrv runs them
msrv:
    scripts/local/msrv.sh

# Size budget of the library crates (fails when a budget is exceeded)
loc:
    scripts/loc.sh --check

# Normal-dependency closure size of each library crate (crates besides the crate itself)
deps:
    #!/usr/bin/env bash
    set -euo pipefail
    for crate in {{ libs }}; do
      n=$(cargo tree --locked -p "$crate" -e normal --prefix none | sed -E 's/ \(.*\)$//' | sort -u | grep -vc "^$crate v" || true)
      printf '%-14s %4s\n' "$crate" "$n"
    done

# Public-content check of the whole history against the private denylist
public denylist=`git config --get rivium.denylist || true`:
    scripts/check-public.sh "{{ denylist }}"

# Install the git hooks (pre-commit, commit-msg, pre-push): public-content check, Conventional Commits
hooks denylist:
    git config core.hooksPath scripts/hooks
    git config rivium.denylist "$(cd "$(dirname "{{ denylist }}")" && pwd)/$(basename "{{ denylist }}")"

# Local reproductions of CI jobs with Docker (see scripts/local/ and .tools/local.env):
# `cross test` under qemu-user / the bionic runner in the pinned images
cross *targets="armv7-unknown-linux-gnueabihf loongarch64-unknown-linux-gnu aarch64-linux-android":
    scripts/local/cross-test.sh {{ targets }}

# Android: bionic tests and 16 KB page alignment of the beacon-jni library
android:
    scripts/local/android.sh

# glibc 2.17 baseline builds (cargo-zigbuild), symbol check and a run on CentOS 7
glibc:
    scripts/local/glibc.sh

# Desktop JVM: rivium-jni's contract on the beacon-jni library in a JDK 17
jvm:
    scripts/local/jvm.sh

# Generate projects from the template and check them against this workspace, as ci/template does
template:
    scripts/template-smoke.sh

# systemd hosting smoke in a local systemd container
systemd:
    scripts/local/glibc.sh
    RIVIUM_SYSTEMD_BASE=debian:bookworm-slim scripts/hosting/systemd.sh target/local/zig/$(uname -m | sed 's/arm64/aarch64/')-unknown-linux-gnu/release/smoke
