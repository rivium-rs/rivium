#!/usr/bin/env bash
# Self-test for scripts/check-commit-msg.sh: conforming messages pass, planted violations fail.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-commit-msg-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
failures=0
expect() { # <exit status> <message>
  local got=0
  printf '%s\n' "$2" > "$tmp/msg"
  "$root/scripts/check-commit-msg.sh" "$tmp/msg" > /dev/null 2>&1 || got=$?
  if [ "$got" = "$1" ]; then echo "ok   $1 <- $(head -n 1 "$tmp/msg")"; else
    echo "FAIL $(head -n 1 "$tmp/msg") (exit $got, want $1)"; failures=$((failures + 1)); fi
}

expect 0 "ci(hosting): plant a failing stop exit code in every hosting job"
expect 0 "feat: add the error type"
expect 0 "fix(rivium-http)!: map InvalidBody to 406"
expect 0 "docs: x"
expect 0 "$(printf '# Please enter the commit message\n\nchore: tidy\n\nBody text. Ends with a period.')"
expect 0 "Merge branch 'main' into c-1"
expect 0 "fixup! feat: add the error type"
expect 1 "Add the workspace skeleton, smoke programs and CI baseline"
expect 1 "Feat: add the error type"
expect 1 "feat:add the error type"
expect 1 "feat(Scope): add the error type"
expect 1 "feature: add the error type"
expect 1 "feat: add the error type."
expect 1 "feat: $(printf 'x%.0s' $(seq 1 70))"
expect 1 ""

if [ "$failures" -ne 0 ]; then echo "check-commit-msg self-test: $failures failure(s)"; exit 1; fi
echo "check-commit-msg self-test: all passed"
