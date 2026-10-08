#!/usr/bin/env bash
# Size budget check: Rust code lines of each library crate's src/ as counted by tokei (comments
# and blank lines excluded), without #[cfg(test)] modules (inline or out-of-line `mod x;`) and
# without tests/. Relies on rustfmt layout: an inline test module ends at the first `}` line with
# the module's indentation.
#
# Usage: scripts/loc.sh [--check] [<workspace root>]    --check exits 1 when a budget is exceeded
set -euo pipefail
export LC_ALL=C

check=false
if [ "${1:-}" = "--check" ]; then
  check=true
  shift
fi
root=$(cd "${1:-$(dirname "$0")/..}" && pwd)
for tool in tokei jq; do
  command -v "$tool" > /dev/null || { echo "loc.sh: $tool is required" >&2; exit 2; }
done

# crate:budget, then the total budget. A budget is raised only with the maintainer's approval.
budgets="rivium-error:500 rivium:5400 rivium-http:900 rivium-jni:350 rivium-test:750"
total_budget=7900

work=$(mktemp -d "${TMPDIR:-/tmp}/loc.XXXXXX")
trap 'rm -rf "$work"' EXIT

strip_test_modules() { # <file> <file for out-of-line test module paths, relative to the file's dir>
  awk -v excl="$2" '
    function flush() { if (held != "") printf "%s", held; held = ""; pending = 0 }
    skip { if ($0 ~ ("^" ind "}[[:space:]]*$")) skip = 0; next }
    pending && /^[[:space:]]*#\[/ { held = held $0 "\n"; next }
    pending && /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*;/ {
      name = $0; sub(/^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+/, "", name); sub(/[^A-Za-z0-9_].*$/, "", name)
      print name > excl; held = ""; pending = 0; next
    }
    pending && /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*\{/ {
      held = ""; pending = 0
      if ($0 !~ /\}[[:space:]]*$/) { match($0, /^[[:space:]]*/); ind = substr($0, 1, RLENGTH); skip = 1 }
      next
    }
    pending { flush() }
    /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/ { pending = 1; held = $0 "\n"; next }
    { print }
    END { flush() }
  ' "$1"
}

count_crate() { # <crate dir>; prints the code line count
  local src=$1/src copy f rel dir base name
  copy="$work/$(basename "$1")"
  [ -d "$src" ] || { echo 0; return; }
  (cd "$src" && find . -name '*.rs' -type f | sed 's|^\./||' | sort) > "$work/files"
  : > "$work/excluded"
  while IFS= read -r rel; do
    mkdir -p "$copy/$(dirname "$rel")"
    : > "$work/mods"
    strip_test_modules "$src/$rel" "$work/mods" > "$copy/$rel"
    dir=$(dirname "$rel")
    base=$(basename "$rel" .rs)
    case "$base" in lib | main | mod) ;; *) dir="$dir/$base" ;; esac
    while IFS= read -r name; do
      printf '%s\n' "$dir/$name.rs" "$dir/$name" >> "$work/excluded"
    done < "$work/mods"
  done < "$work/files"
  while IFS= read -r f; do rm -rf "${copy:?}/${f#./}"; done < "$work/excluded"
  tokei --types Rust --output json "$copy" | jq '.Rust.code // 0'
}

printf '%-14s %8s %8s\n' crate code budget
total=0
over=false
for entry in $budgets; do
  crate=${entry%%:*}
  budget=${entry##*:}
  code=$(count_crate "$root/crates/$crate")
  total=$((total + code))
  mark=""
  if [ "$code" -gt "$budget" ]; then mark="  OVER"; over=true; fi
  printf '%-14s %8s %8s%s\n' "$crate" "$code" "$budget" "$mark"
done
mark=""
if [ "$total" -gt "$total_budget" ]; then mark="  OVER"; over=true; fi
printf '%-14s %8s %8s%s\n' total "$total" "$total_budget" "$mark"
if $check && $over; then
  echo "loc.sh: size budget exceeded" >&2
  exit 1
fi
