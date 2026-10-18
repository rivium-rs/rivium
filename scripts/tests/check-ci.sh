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
  local workflow=$1 gate=$2 omitted=${3:-}
  if sed -n '/^jobs:$/,$p' "$workflow" | grep -E '^  [^[:space:]#]' | grep -Ev '^  [a-zA-Z_][a-zA-Z0-9_-]*:$' > "$work/unparsed"; then
    echo 'ci-pass: unrecognized job key layout' >&2; return 1
  fi
  sed -n '/^jobs:$/,$p' "$workflow" | sed -nE 's/^  ([a-zA-Z_][a-zA-Z0-9_-]*):$/\1/p' \
    | awk -v gate="$gate" -v omitted="$omitted" '$0 != gate && $0 != omitted' | sort > "$work/jobs"
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

# Release additionally requires Loom; its privileged publish job is downstream of ready.
workflow="$root/.github/workflows/release.yml"
verify_needs "$workflow" ready publish
# shellcheck disable=SC2016
grep -Fq '    if: ${{ always() }}' "$work/gate"
grep -Fxq loom "$work/needs"
awk '/^  loom:$/ {inside=1; next} inside && /^  [^ ]/ {exit} inside {print}' "$workflow" > "$work/loom-job"
grep -Fxq '    uses: ./.github/workflows/loom.yml' "$work/loom-job"
grep -Fxq '  workflow_call:' "$root/.github/workflows/loom.yml"
for status in failure cancelled skipped pending unknown ''; do
  needs=$(jq -nc --arg status "$status" '{check:{result:"success"},ci:{result:"success"},loom:{result:$status}}')
  if "$check" "$needs" > "$work/log" 2>&1; then
    echo "release/ready accepted Loom status $status" >&2; exit 1
  fi
done
sed 's/, loom\]/]/' "$workflow" > "$work/missing-loom.yml"
if verify_needs "$work/missing-loom.yml" ready publish; then
  echo 'release/ready accepted missing Loom dependency' >&2; exit 1
fi
# Reusable workflows inherit github.workflow: distinct literal prefixes prevent caller cancellation.
grep -Fq '  group: loom-' "$root/.github/workflows/loom.yml"
grep -Fq '  group: release-' "$workflow"
if grep -Eq '^[[:space:]]*(continue-on-error:|if:)' "$work/loom-job"; then
  echo 'release Loom must not be optional' >&2; exit 1
fi
# In particular, an experimental platform must not mask a simultaneous Android failure.
if "$check" '{"platform":{"result":"failure"},"android":{"result":"failure"},"glibc-centos":{"result":"success"}}' > "$work/log" 2>&1; then
  echo 'daily-pass accepted simultaneous platform failures' >&2; exit 1
fi
if grep -Eq '^[[:space:]]*continue-on-error:|if:[[:space:]]*false' "$root/.github/workflows/daily.yml"; then
  echo 'daily platforms must not silently pass or disable failed checks' >&2; exit 1
fi
echo 'CI, daily and release aggregate self-tests: passed'
