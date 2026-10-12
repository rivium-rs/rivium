#!/usr/bin/env bash
# Build an ordinary subject, then run the private process suite in a separate build directory.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
command -v python3 >/dev/null || { echo 'test-hosts: Python >=3.11 is required' >&2; exit 2; }
exec python3 scripts/test-hosts.py "$@"
