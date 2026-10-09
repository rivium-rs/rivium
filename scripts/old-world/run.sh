#!/usr/bin/env bash
# The old-world LoongArch record (V-17, ABI 1.0), run on a native old-world machine without
# network, inside a bundle made by scripts/old-world/bundle.sh (this script is its run.sh). With
# the vendor's Rust 1.92 toolchain and only the bundled crates, it tests the workspace, builds the
# release programs, checks their ELF headers (check-elf.sh) and runs and stops each one once
# (check-stop.sh). Needs no root. The full log goes next to this script; a summary ends the run.
# Usage: bash run.sh [<toolchain directory>]   (default: the one recorded in ./toolchain)
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src=$here/src
toolchain=${1:-$(cat "$here/toolchain" 2> /dev/null || true)}
log=$here/v17-$(date +%Y%m%d-%H%M%S).log
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
# The programs built for release, checked and run once: package, program, and the arguments it
# needs to run in a test. A validation branch adds the programs it brings.
programs=(
  "edge-lite-bin edge-lite --set http.addr=127.0.0.1:0"
  "snmp-lite-bin snmp-lite --set snmp.addr=127.0.0.1:0"
  "udp-echo udp-echo"
)
summary=()
failures=0

say() { printf '%s\n' "$*" | tee -a "$log"; }
record() { # <PASS|FAIL|SKIP> <what>
  summary+=("$(printf '%-4s  %s' "$1" "$2")")
  [ "$1" != FAIL ] || failures=$((failures + 1))
  printf '==== %s  %s\n' "$1" "$2" >> "$log"
}
since() { local s=$(($(date +%s) - $1)); printf '%dm%02ds' $((s / 60)) $((s % 60)); }
present() { grep -rqs --include Cargo.toml -x "name = \"$1\"" "$src/examples" "$src/crates"; }

say "V-17 old-world record: $(grep '^commit' "$here/BUNDLE" 2> /dev/null || echo 'commit unknown')"
say "full log: $log"
{
  echo "==== machine"
  cat "$here/BUNDLE" 2> /dev/null
  uname -a
  grep -E '^(PRETTY_NAME|VERSION_ID)=' /etc/os-release 2> /dev/null
  grep -m 1 -i 'model name' /proc/cpuinfo 2> /dev/null
  echo "cpus $(nproc 2> /dev/null), page size $(getconf PAGESIZE 2> /dev/null)"
  ldd --version 2> /dev/null | head -n 1
  ls -l /lib64/ld.so.1 2> /dev/null
  LC_ALL=C readelf -h /bin/ls 2> /dev/null | grep -E 'Machine|Flags'
  df -h "$here" 2> /dev/null | tail -n 1
  env | grep -E '^(CARGO|RUST)' || true
  ls -l "$HOME/.cargo/config.toml" "$HOME/.cargo/config" 2> /dev/null || true
} >> "$log" 2>&1
machine="$(uname -r) $(uname -m), $(sed -n -E 's/^PRETTY_NAME="?([^"]*)"?$/\1/p' /etc/os-release 2> /dev/null)"
machine="$machine, glibc $(ldd --version 2> /dev/null | head -n 1 | awk '{ print $NF }'), page $(getconf PAGESIZE 2> /dev/null)"

# The toolchain: rustc and cargo 1.92 for loongarch64-unknown-linux-gnu, not on PATH.
version=""
if [ -x "$toolchain/bin/cargo" ] && [ -x "$toolchain/bin/rustc" ]; then
  export PATH=$toolchain/bin:$PATH
  rustc -vV >> "$log" 2>&1
  cargo -V >> "$log" 2>&1
  version=$(rustc -V 2> /dev/null)
  host=$(rustc -vV 2> /dev/null | sed -n 's/^host: //p')
  if [ "$host" = loongarch64-unknown-linux-gnu ] && [[ "$version" == "rustc 1.92."* ]]; then
    record PASS "toolchain: $version, host $host"
  else
    record FAIL "toolchain: $version, host $host (want 1.92 for loongarch64-unknown-linux-gnu)"
  fi
else
  record FAIL "toolchain: no rustc and cargo in ${toolchain:-<none>}/bin (pass the directory: bash run.sh <dir>)"
fi

# Only the bundled crates: src/.cargo/config.toml points at ../vendor and keeps cargo offline.
export CARGO_HOME=$here/cargo-home CARGO_TARGET_DIR=$here/target CARGO_NET_OFFLINE=true
mkdir -p "$CARGO_HOME"
cd "$src" || exit 2

if [ -n "$version" ]; then
  say "[1/3] cargo test --workspace (tens of minutes) ..."
  start=$(date +%s)
  status=0
  cargo test --workspace --all-features --locked --no-fail-fast > "$out/test" 2>&1 || status=$?
  cat "$out/test" >> "$log"
  counts=$(grep -E '^test result: ' "$out/test" | sed -E 's/.* ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1 \2 \3/' |
    awk '{ p += $1; f += $2; i += $3; n++ } END { printf "%d passed, %d failed, %d ignored in %d suites", p, f, i, n }')
  if [ "$status" -eq 0 ]; then
    record PASS "cargo test: $counts, $(since "$start")"
  else
    record FAIL "cargo test (exit $status): $counts, $(since "$start"); see the log"
  fi

  say "[2/3] release build ..."
  start=$(date +%s)
  packages=()
  for entry in "${programs[@]}"; do
    read -r package _ <<< "$entry"
    if present "$package"; then packages+=(-p "$package"); else record SKIP "$package: not in this commit"; fi
  done
  status=0
  cargo build --release --locked "${packages[@]}" >> "$log" 2>&1 || status=$?
  if [ "$status" -eq 0 ]; then record PASS "release build, $(since "$start")"; else record FAIL "release build (exit $status); see the log"; fi

  say "[3/3] ELF checks and one run of each program ..."
  for entry in "${programs[@]}"; do
    read -r package program args <<< "$entry"
    present "$package" || continue
    bin=$CARGO_TARGET_DIR/release/$program
    if [ ! -x "$bin" ]; then
      record FAIL "$program: not built"
      continue
    fi
    if line=$(cd "$CARGO_TARGET_DIR/release" && bash "$src/scripts/old-world/check-elf.sh" "$program" 2>&1); then
      record PASS "${line#ok   }"
    else
      record FAIL "${line#FAIL }"
    fi
    # shellcheck disable=SC2086 # the arguments are words
    if bash "$src/scripts/check-stop.sh" "$bin" $args >> "$log" 2>&1; then
      record PASS "$program runs, and stops on SIGTERM with 0"
    else
      record FAIL "$program does not run and stop cleanly; see the log"
    fi
  done
fi

{
  echo
  echo "==== summary"
  echo "rivium $(sed -n 's/^commit //p' "$here/BUNDLE" 2> /dev/null)"
  echo "machine: $machine"
  printf '%s\n' "${summary[@]}"
  if [ "$failures" -eq 0 ]; then echo "RESULT: PASS (${#summary[@]} items)"; else echo "RESULT: FAIL ($failures of ${#summary[@]} items failed)"; fi
  echo "log: $log"
} | tee -a "$log"
[ "$failures" -eq 0 ]
