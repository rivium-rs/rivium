#!/usr/bin/env bash
# Fails unless every program is built for the old-world LoongArch ABI 1.0, as the vendor's Rust
# 1.92 toolchain builds it on a native machine: machine LoongArch, ELF flags 0x3, interpreter
# /lib64/ld.so.1, and no GLIBC_x.y symbol version above 2.28. A new-world build (flags 0x43,
# interpreter /lib64/ld-linux-loongarch-lp64d.so.1, glibc 2.36) fails the last three.
# Usage: scripts/old-world/check-elf.sh <program>...
# Uses $READELF if set, otherwise readelf (binutils) or llvm-readelf. Runs it in the C locale:
# under zh_CN binutils translates "Machine:" and "Flags:" (but not the interpreter line).
set -euo pipefail
export LC_ALL=C
[ $# -ge 1 ] || { echo "usage: check-elf.sh <program>..." >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
readelf=${READELF:-$(command -v readelf || command -v llvm-readelf || true)}
[ -n "$readelf" ] || { echo "check-elf: readelf not found" >&2; exit 2; }
export READELF=$readelf

status=0
for file in "$@"; do
  header=$("$readelf" -h "$file" 2> /dev/null) || { echo "check-elf: not an ELF file: $file" >&2; exit 2; }
  machine=$(sed -n -E 's/^ *Machine: *//p' <<< "$header")
  flags=$(sed -n -E 's/^ *Flags: *(0x[0-9a-fA-F]+).*/\1/p' <<< "$header")
  interpreter=$("$readelf" -lW "$file" | sed -n -E 's/.*\[Requesting program interpreter: (.*)\]$/\1/p')
  glibc=$("$here/../check-glibc.sh" 2.28 "$file" 2>&1) || true
  wrong=()
  case "$machine" in *LoongArch* | *0x102*) ;; *) wrong+=("machine $machine (want LoongArch)") ;; esac
  [ "$flags" = 0x3 ] || wrong+=("flags ${flags:-none} (want 0x3)")
  [ "$interpreter" = /lib64/ld.so.1 ] || wrong+=("interpreter ${interpreter:-none} (want /lib64/ld.so.1)")
  case "$glibc" in "ok   "*) ;; *) wrong+=("${glibc#FAIL "$file": }") ;; esac
  if [ ${#wrong[@]} -eq 0 ]; then
    echo "ok   $file: LoongArch, flags 0x3, interpreter /lib64/ld.so.1, ${glibc#ok   "$file": }"
  else
    joined=$(printf '%s; ' "${wrong[@]}")
    echo "FAIL $file: ${joined%; }"
    status=1
  fi
done
exit "$status"
