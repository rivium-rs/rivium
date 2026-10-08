#!/usr/bin/env bash
# Conventional Commits check for a commit message or a PR title: the first line that is neither
# empty nor a `#` comment must be `<type>(<scope>)!: <subject>` (scope and `!` optional), at most
# 72 characters, without a trailing period. Messages that git generates (merge, revert, fixup!,
# squash!, amend!) pass unchanged.
# Usage: scripts/check-commit-msg.sh <message file>
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: check-commit-msg.sh <message file>" >&2; exit 2; }
subject=$(sed -e '/^#/d' -e '/^[[:space:]]*$/d' "$1" | head -n 1)
case "$subject" in
  "Merge "* | "Revert \""* | "fixup! "* | "squash! "* | "amend! "*) exit 0 ;;
esac
types='build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test'
if ! grep -Eq "^($types)(\([a-z0-9][a-z0-9._/-]*\))?!?: [^[:space:]](.*[^.])?$" <<< "$subject"; then
  echo "not a Conventional Commit: '$subject'" >&2
  echo "expected '<type>(<scope>)!: <subject>' (scope and ! optional), type one of: ${types//|/, }" >&2
  exit 1
fi
if [ "${#subject}" -gt 72 ]; then
  echo "commit subject is ${#subject} characters long; the limit is 72" >&2
  exit 1
fi
