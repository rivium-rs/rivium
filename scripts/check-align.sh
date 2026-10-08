#!/usr/bin/env bash
# Fails when a LOAD segment of an ELF file is aligned to less than the given number of bytes
# (Android libraries are linked for 16 KB pages: -Wl,-z,max-page-size=16384).
# Usage: scripts/check-align.sh <min alignment, e.g. 16384> <ELF file>...
# Uses $READELF if set, otherwise readelf (binutils) or llvm-readelf.
set -euo pipefail
[ $# -ge 2 ] || { echo "usage: check-align.sh <min alignment> <ELF file>..." >&2; exit 2; }
min=$1
shift
readelf=${READELF:-$(command -v readelf || command -v llvm-readelf || true)}
[ -n "$readelf" ] || { echo "check-align: readelf not found" >&2; exit 2; }

status=0
for file in "$@"; do
  aligns=$("$readelf" -lW "$file" | awk '$1 == "LOAD" { print $NF }')
  [ -n "$aligns" ] || { echo "check-align: no LOAD segments in $file" >&2; exit 2; }
  smallest=$(for a in $aligns; do printf '%d\n' "$a"; done | sort -n | head -n 1)
  if [ "$smallest" -ge "$min" ]; then
    echo "ok   $file: LOAD segments aligned to >= $smallest bytes"
  else
    echo "FAIL $file: a LOAD segment is aligned to $smallest bytes (< $min)"
    status=1
  fi
done
exit "$status"
