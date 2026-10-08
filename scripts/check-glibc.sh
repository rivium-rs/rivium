#!/usr/bin/env bash
# Fails when an ELF file requires a GLIBC_x.y symbol version above the baseline (design: artifact
# baselines; glibc 2.17 for x86_64/aarch64/armv7, 2.36 for loongarch64).
# Usage: scripts/check-glibc.sh <max version, e.g. 2.17> <ELF file>...
# Uses $READELF if set, otherwise readelf (binutils) or llvm-readelf.
set -euo pipefail
[ $# -ge 2 ] || { echo "usage: check-glibc.sh <max version> <ELF file>..." >&2; exit 2; }
max=$1
shift
readelf=${READELF:-$(command -v readelf || command -v llvm-readelf || true)}
[ -n "$readelf" ] || { echo "check-glibc: readelf not found" >&2; exit 2; }

status=0
for file in "$@"; do
  "$readelf" -h "$file" > /dev/null 2>&1 || { echo "check-glibc: not an ELF file: $file" >&2; exit 2; }
  highest=$("$readelf" --version-info --wide "$file" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' \
    | awk -F. '{ v = $1 * 1000000 + $2 * 1000 + $3; if (v > best) { best = v; text = $0 } } END { print text }')
  if [ -z "$highest" ]; then
    echo "ok   $file: no GLIBC symbol versions (static or not glibc)"
  elif awk -v a="$highest" -v b="$max" 'BEGIN {
      split(a, x, "."); split(b, y, ".")
      exit !((x[1] * 1000000 + x[2] * 1000 + x[3]) <= (y[1] * 1000000 + y[2] * 1000 + y[3])) }'; then
    echo "ok   $file: highest GLIBC_$highest <= $max"
  else
    echo "FAIL $file: requires GLIBC_$highest > $max"
    status=1
  fi
done
exit "$status"
