#!/usr/bin/env bash
# Reject every non-success result and guard against forgetting a job in ci-pass.needs.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
check="$root/scripts/check-ci.sh"
"$check" '{"lint":{"result":"success"},"test":{"result":"success"}}'
for status in failure cancelled skipped pending unknown ''; do
  needs=$(jq -nc --arg status "$status" '{lint:{result:"success"},test:{result:$status}}')
  if "$check" "$needs" > "$work/log" 2>&1; then
    echo "ci-pass accepted $status" >&2; exit 1
  fi
done
for needs in '{}' '[]' 'null' 'broken JSON' '{} {"lint":{"result":"success"}}' '{"lint":{}}' '{"lint":"success"}'; do
  if "$check" "$needs" > "$work/log" 2>&1; then
    echo "ci-pass accepted invalid needs: $needs" >&2; exit 1
  fi
done

# This narrow check matches this repository's two-space job keys and inline needs list.
# A layout change must update the check rather than silently weakening branch protection.
verify_needs() {
  local workflow=$1 gate=$2
  if sed -n '/^jobs:$/,$p' "$workflow" | grep -E '^  [^[:space:]#]' | grep -Ev '^  [a-zA-Z_][a-zA-Z0-9_-]*:$' > "$work/unparsed"; then
    echo 'ci-pass: unrecognized job key layout' >&2; return 1
  fi
  sed -n '/^jobs:$/,$p' "$workflow" | sed -nE 's/^  ([a-zA-Z_][a-zA-Z0-9_-]*):$/\1/p' | grep -vx "$gate" | sort > "$work/jobs"
  awk -v key="  $gate:" '$0 == key {inside=1; next} inside && /^  [^ ]/ {exit} inside {print}' "$workflow" > "$work/gate"
  sed -nE 's/^    needs: \[([^]]*)\]$/\1/p' "$work/gate" | tr ',' '\n' | tr -d ' ' | sort > "$work/needs"
  [ -s "$work/jobs" ] && cmp -s "$work/jobs" "$work/needs"
}
for gate in ci-pass daily-pass; do
  workflow="$root/.github/workflows/${gate%-pass}.yml"
  verify_needs "$workflow" "$gate"
  # shellcheck disable=SC2016 # Match the literal GitHub expression, not a shell variable.
  grep -Fq '    if: ${{ always() }}' "$work/gate"
  sed -E 's/(needs: \[)[^,]+, /\1/' "$workflow" > "$work/missing-job.yml"
  if verify_needs "$work/missing-job.yml" "$gate"; then
    echo "$gate accepted a missing required job" >&2; exit 1
  fi
done
# In particular, an experimental platform must not mask a simultaneous Android failure.
if "$check" '{"platform":{"result":"failure"},"android":{"result":"failure"},"glibc-centos":{"result":"success"}}' > "$work/log" 2>&1; then
  echo 'daily-pass accepted simultaneous platform failures' >&2; exit 1
fi
if grep -Eq '^[[:space:]]*continue-on-error:|if:[[:space:]]*false' "$root/.github/workflows/daily.yml"; then
  echo 'daily platforms must not silently pass or disable failed checks' >&2; exit 1
fi
echo 'CI and daily aggregate self-tests: passed'
