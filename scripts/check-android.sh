#!/usr/bin/env bash
# Check the actual JNI shared object, not an rlib or an empty target directory.
set -euo pipefail
[ "$#" -eq 1 ] || { echo 'usage: check-android.sh <JNI ELF>' >&2; exit 2; }
fail() { echo "android: $*" >&2; exit 1; }
reader=${READELF:-readelf}
command -v "$reader" >/dev/null || fail 'readelf is missing'
[ -s "$1" ] || fail 'missing or empty JNI library'
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
LC_ALL=C "$reader" --wide --file-header --program-headers --dyn-syms --dynamic --notes "$1" > "$work/elf" 2> "$work/errors" \
  || fail 'readelf failed'
[ ! -s "$work/errors" ] || { cat "$work/errors" >&2; fail 'readelf diagnostics'; }
grep -Eq 'Class:[[:space:]]+ELF64' "$work/elf" || fail 'not ELF64'
grep -Eq 'Machine:[[:space:]]+AArch64' "$work/elf" || fail 'wrong machine'
grep -Eq 'Type:[[:space:]]+DYN' "$work/elf" || fail 'not a shared object'
grep -Eq 'Data:.*little endian' "$work/elf" || fail 'not little-endian Android'
grep -Eq '\(NEEDED\).*\[libc\.so\]' "$work/elf" || fail 'not linked to bionic libc'
# GNU readelf prints the Android note payload as bytes; API 30 is 0x1e, little endian.
awk '
  /^[[:space:]]*[^[:space:]]+[[:space:]]+0x[[:xdigit:]]+[[:space:]]/ {android = ($1 == "Android"); if (android) owners++}
  android && /description data:/ {
    sub(/^.*description data:[[:space:]]*/, "")
    if ($1 == "1e" && $2 == "00" && $3 == "00" && $4 == "00") api30++
    android = 0
  }
  END {exit owners != 1 || api30 != 1}' "$work/elf" || fail 'missing or wrong Android API 30 note'
awk '$4 == "FUNC" && $5 == "GLOBAL" && $6 == "DEFAULT" && $7 != "UND" && $8 == "JNI_OnLoad" {n++}
  END {exit n != 1}' "$work/elf" || fail 'missing defined JNI_OnLoad export'
awk '$1 == "LOAD" {print $2, $3, $NF}' "$work/elf" > "$work/loads"
[ -s "$work/loads" ] || fail 'no LOAD segments'
while read -r offset address alignment; do
  for value in "$offset" "$address" "$alignment"; do
    [[ "$value" =~ ^0x[0-9a-fA-F]{1,16}$ ]] || fail 'malformed LOAD segment'
  done
  ((alignment >= 16384 && (alignment & (alignment - 1)) == 0)) || fail 'LOAD alignment is below 16 KB or not a power of two'
  (( ((address - offset) & (alignment - 1)) == 0 )) || fail 'LOAD offset/address are not congruent'
done < "$work/loads"
echo 'android: API 30 bionic JNI/AArch64; all LOAD segments satisfy 16 KB alignment'
