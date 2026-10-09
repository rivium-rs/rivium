#!/usr/bin/env bash
# Builds the program for each platform and packs what the installer downloads:
#   dist/<component>/<platform>/<version>/<component>.tar.gz (zip on Windows) and its .sha256,
# holding the program and configs/default.toml for that platform. The configuration is printed
# by the program built for this machine (`default-config`, with the platform's settings below),
# so that it shows every default the shipped program has.
#
# Builders: Linux and Windows with cargo-zigbuild (and zig); the Linux builds link against glibc
# 2.17, the baseline of the targets' oldest systems. linux_loong64 (new-world ABI, glibc 2.36)
# with cross and Docker. darwin_arm64 with cargo on a macOS machine.
# Every tool the platforms need is checked first: a missing one is reported with how to get it.
# Usage: scripts/package.sh [--build-only] <platform>...
set -euo pipefail
cd "$(dirname "$0")/.."
name={{project-name}}
component={% if component == "" %}{{project-name}}{% else %}{{component}}{% endif %}
build_only=false
if [ "${1:-}" = --build-only ]; then
  build_only=true
  shift
fi
[ $# -ge 1 ] || { echo "usage: scripts/package.sh [--build-only] <platform>..." >&2; exit 2; }

# The Rust target and builder of a platform.
target_of() {
  case "$1" in
    linux_amd64) echo "zigbuild x86_64-unknown-linux-gnu.2.17" ;;
    linux_arm64) echo "zigbuild aarch64-unknown-linux-gnu.2.17" ;;
    linux_arm32hf) echo "zigbuild armv7-unknown-linux-gnueabihf.2.17" ;;
    linux_loong64) echo "cross loongarch64-unknown-linux-gnu" ;;
    windows_amd64) echo "zigbuild x86_64-pc-windows-gnu" ;;
    darwin_arm64) echo "cargo aarch64-apple-darwin" ;;
    *) echo "package.sh: unknown platform $1" >&2; return 1 ;;
  esac
}

# The settings that differ by platform, as arguments of `default-config`.
settings_of() {
  case "$1" in
    # The deployment's arm32hf unit neither restarts the service nor starts it at boot, so the
    # service restarts itself in the process after a failure.
    linux_arm32hf) echo "--set lifecycle.restart=in-process" ;;
  esac
}

sha256() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }

# The tools of every platform asked for, before the first build: each missing one with how to get it.
missing=()
need() { # <what is missing> <how to get it>
  case " ${missing[*]-} " in *" $1:"*) ;; *) missing+=("$1: $2") ;; esac
}
installed_targets=$(rustup target list --installed 2> /dev/null || true)
for platform in "$@"; do
  spec=$(target_of "$platform")
  read -r builder target <<< "$spec"
  target=${target%.2.17}
  case "$builder" in
    zigbuild)
      command -v cargo-zigbuild > /dev/null || need cargo-zigbuild "cargo install cargo-zigbuild --locked"
      # cargo-zigbuild finds zig on PATH or as the ziglang Python package.
      command -v zig > /dev/null || python3 -c 'import ziglang' 2> /dev/null \
        || need zig "brew install zig, pip install ziglang, or https://ziglang.org/download/"
      ;;
    cross)
      command -v cross > /dev/null || need cross "cargo install cross --locked"
      command -v docker > /dev/null || need docker "Docker or another engine that cross supports"
      ;;
    cargo) [ "$(uname -s)" = Darwin ] || need "$platform" "build it on macOS" ;;
  esac
  if [ "$builder" != cross ] && [ -n "$installed_targets" ] && ! grep -qx "$target" <<< "$installed_targets"; then
    need "$target" "rustup target add $target"
  fi
  case "$platform" in windows_*) $build_only || command -v zip > /dev/null || need zip "your package manager" ;; esac
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "package.sh: missing tools for $*:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi

version=$(cargo pkgid -p "$name-bin" | sed 's/.*[@#]//')
for platform in "$@"; do
  read -r builder target <<< "$(target_of "$platform")"
  case "$builder" in
    zigbuild) cargo zigbuild --locked --release -p "$name-bin" --target "$target" ;;
    *) "$builder" build --locked --release -p "$name-bin" --target "$target" ;;
  esac
  $build_only && continue
  exe=$name
  case "$platform" in windows_*) exe=$name.exe ;; esac
  stage=$(mktemp -d)
  mkdir -p "$stage/configs"
  cp "${CARGO_TARGET_DIR:-target}/${target%.2.17}/release/$exe" "$stage/"
  # shellcheck disable=SC2046
  cargo run --locked -q -p "$name-bin" -- default-config $(settings_of "$platform") > "$stage/configs/default.toml"
  out=dist/$component/$platform/$version
  mkdir -p "$out"
  case "$platform" in
    windows_*) archive=$component.zip && (cd "$stage" && zip -qr - .) > "$out/$archive" ;;
    *) archive=$component.tar.gz && tar -czf "$out/$archive" -C "$stage" . ;;
  esac
  (cd "$out" && sha256 "$archive" > "$archive.sha256")
  rm -rf "$stage"
  echo "package.sh: $out/$archive"
done
