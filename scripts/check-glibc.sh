#!/usr/bin/env bash
# Check real x86_64 ELF artifacts against the glibc 2.17 runtime baseline.
set -euo pipefail
[ "$#" -gt 0 ] || { echo 'usage: check-glibc.sh <ELF>...' >&2; exit 2; }
reader=${READELF:-readelf}
command -v "$reader" >/dev/null || { echo 'glibc: readelf is missing' >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail() { echo "glibc: $*" >&2; exit 1; }

for artifact in "$@"; do
  [ -s "$artifact" ] || fail "missing or empty artifact: $artifact"
  LC_ALL=C "$reader" --wide --file-header --program-headers --dynamic --dyn-syms --version-info "$artifact" \
    > "$work/elf" 2> "$work/errors" || fail "readelf failed: $artifact"
  [ ! -s "$work/errors" ] || { cat "$work/errors" >&2; fail "readelf diagnostics: $artifact"; }
  grep -Eq 'Class:[[:space:]]+ELF64' "$work/elf" || fail "not ELF64: $artifact"
  grep -Eq 'Machine:[[:space:]]+Advanced Micro Devices X86-64' "$work/elf" || fail "wrong machine: $artifact"
  grep -Eq 'Type:[[:space:]]+(DYN|EXEC)' "$work/elf" || fail "not an executable or shared object: $artifact"
  grep -Eq '\(NEEDED\).*\[libc\.so\.6\]' "$work/elf" || fail "missing dynamic libc dependency: $artifact"
  if [[ "$artifact" != *.so ]]; then
    grep -Fq '[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]' "$work/elf" \
      || fail "wrong or missing interpreter: $artifact"
  fi
  # Read complete version names, not a whitelist that truncates unknown suffixes.
  awk 'match($0, /Name: GLIBC_/) {
    version = substr($0, RSTART + 6)
    sub(/  Flags: [^[:space:]]+  Version: [0-9]+$/, "", version)
    print version
  }' "$work/elf" | sort -u > "$work/versions"
  [ -s "$work/versions" ] || fail "no GLIBC symbol versions: $artifact"
  highest=0.0
  high_minor=0
  high_patch=0
  while IFS= read -r symbol; do
    version=${symbol#GLIBC_}
    [[ "$version" =~ ^[0-9]{1,3}\.[0-9]{1,3}(\.[0-9]{1,3})?$ ]] || fail "unrecognized version $symbol: $artifact"
    IFS=. read -r major minor patch <<< "$version"
    major=$((10#$major)); minor=$((10#$minor)); patch=$((10#${patch:-0}))
    # The supported GNU libc ABI starts at 2.x; compare components, never strings.
    ((major == 2)) || fail "unsupported version $symbol: $artifact"
    ((minor < 17 || (minor == 17 && patch == 0))) || fail "$symbol exceeds GLIBC_2.17: $artifact"
    if ((minor > high_minor || (minor == high_minor && patch >= high_patch))); then
      highest=$version; high_minor=$minor; high_patch=$patch
    fi
  done < "$work/versions"
  echo "glibc: $artifact requires at most GLIBC_$highest (limit GLIBC_2.17)"
done
