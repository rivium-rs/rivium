#!/usr/bin/env bash
# Decides whether a pull request changes documentation only, so that CI can skip the code jobs.
# Lists the files changed on <head> since its merge base with <base> (the pull request's diff),
# without rename detection, so that a file moved out of a code path still lists that path.
# Documentation is everything under docs/ and the Markdown files at the top level. Everything else
# is code, including crates/*/README.md (packaged into the crates) and any path added later. A diff
# without changed files is not treated as documentation only.
# Usage: scripts/docs-only.sh <base> <head>
# Exit status: 0 documentation only, 1 anything else, 2 usage or git error.
set -euo pipefail
[ $# -eq 2 ] || { echo "usage: docs-only.sh <base> <head>" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
list=$(mktemp "${TMPDIR:-/tmp}/docs-only.XXXXXX")
trap 'rm -f "$list"' EXIT
git diff --no-renames --name-only -z "$1...$2" > "$list" || { echo "docs-only: git diff failed" >&2; exit 2; }

files=0
code=0
while IFS= read -r -d '' path; do
  files=$((files + 1))
  case "$path" in
    docs/*) kind=docs ;;
    */*) kind=code ;;
    *.md) kind=docs ;;
    *) kind=code ;;
  esac
  [ "$kind" = docs ] || code=$((code + 1))
  printf '%s  %s\n' "$kind" "$path"
done < "$list"

if [ "$files" -eq 0 ]; then
  echo "docs-only: no (no changed files)"
  exit 1
elif [ "$code" -gt 0 ]; then
  echo "docs-only: no (changed files: $files, not documentation: $code)"
  exit 1
fi
echo "docs-only: yes (changed files: $files, all documentation)"
