#!/usr/bin/env bash
# An aggregate check must fail closed, including when a required job was skipped.
set -euo pipefail
[ "$#" -eq 1 ] || { echo 'usage: check-ci.sh <needs JSON>' >&2; exit 2; }
if ! jq -es 'length == 1 and (.[0] | type == "object" and length > 0 and all(.[]; type == "object" and .result == "success"))' \
  <<< "$1" >/dev/null; then
  echo 'ci-pass: every required job must finish with success' >&2
  exit 1
fi
echo 'ci-pass: all required jobs succeeded'
