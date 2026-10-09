#!/usr/bin/env bash
# Self-test of scripts/old-world/check-elf.sh, run by cross/loongarch64 on its new-world build of
# a program: the program must fail the three ABI checks, and a copy patched to look like an
# old-world build (flags 0x3, interpreter /lib64/ld.so.1, symbol versions GLIBC_2.27) must pass.
# The real old-world record comes from the machine (scripts/old-world/run.sh).
# Usage: scripts/tests/old-world.sh <new-world loongarch64 program>
set -euo pipefail
scripts=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

status=0
"$scripts/old-world/check-elf.sh" "$1" > "$work/new" 2>&1 || status=$?
cat "$work/new"
[ "$status" -eq 1 ] || { echo "old-world self-test: a new-world build was not rejected (exit $status)"; exit 1; }
for part in "flags 0x43 (want 0x3)" "interpreter /lib64/ld-linux-loongarch-lp64d.so.1 (want /lib64/ld.so.1)" \
  "requires GLIBC_2.36 > 2.28"; do
  grep -qF -- "$part" "$work/new" || { echo "old-world self-test: the rejection does not say: $part"; exit 1; }
done

python3 - "$1" "$work/old-world" <<'EOF'
import re, sys
data = bytearray(open(sys.argv[1], 'rb').read())
assert data[:5] == b'\x7fELF\x02', 'not an ELF64 file'
data[0x30:0x34] = (0x3).to_bytes(4, 'little')  # e_flags of ABI 1.0
new, old = b'/lib64/ld-linux-loongarch-lp64d.so.1\0', b'/lib64/ld.so.1\0'
assert new in data, 'no new-world interpreter'
data = data.replace(new, old.ljust(len(new), b'\0'))
open(sys.argv[2], 'wb').write(re.sub(rb'GLIBC_2\.3[0-9]\0', b'GLIBC_2.27\0', bytes(data)))
EOF
"$scripts/old-world/check-elf.sh" "$work/old-world" || { echo "old-world self-test: an old-world build was rejected"; exit 1; }
echo "old-world self-test: passed"
