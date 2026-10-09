#!/usr/bin/env bash
# Checks the packages of the validation slices (examples/slices/<slice>/, design S-2): the code
# specific to a host stays out of the service library. In each slice:
#   - one service library (a lib target that is not a cdylib), which every other package uses;
#   - a program (bin target) depends directly on nothing but the service library and rivium;
#   - a JNI library (cdylib) depends directly on nothing but the service library and rivium-jni;
#   - the src/ of each program and JNI library has at most 10 lines of code (comments and blank
#     lines aside).
# Direct dependencies are the normal and build ones of the manifest; dev-dependencies, which only
# tests compile, do not count.
# Usage: scripts/check-slice-deps.sh [<workspace root>]    Needs cargo and jq.
set -euo pipefail
root=$(cd "${1:-$(dirname "$0")/..}" && pwd)
max_lines=10
for tool in cargo jq; do
  command -v "$tool" > /dev/null || { echo "check-slice-deps: $tool is required" >&2; exit 2; }
done
metadata=$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml")
failures=0
fail() { echo "FAIL $*"; failures=$((failures + 1)); }

# Lines of code below a directory: Rust lines that are neither blank nor comments.
code_lines() {
  find "$1" -name '*.rs' -type f -exec cat {} + |
    awk '/^[[:space:]]*$/ || /^[[:space:]]*\/\// { next } { n++ } END { print n + 0 }'
}

slices=$(find "$root/examples/slices" -mindepth 1 -maxdepth 1 -type d 2> /dev/null | sort)
[ -n "$slices" ] || { echo "check-slice-deps: no slices below $root/examples/slices" >&2; exit 2; }
for slice in $slices; do
  # Each package of the slice: name, kind (lib, bin or cdylib), directory, direct dependencies.
  packages=$(jq -r --arg dir "$slice/" '
    .packages[] | select(.manifest_path | startswith($dir))
    | [ .name,
        (if any(.targets[]; .kind | index("cdylib")) then "cdylib"
         elif any(.targets[]; .kind | index("bin")) then "bin" else "lib" end),
        (.manifest_path | rtrimstr("/Cargo.toml")),
        ([.dependencies[] | select(.kind == null or .kind == "build") | .name] | unique | join(" ")) ]
    | @tsv' <<< "$metadata")
  libs=$(awk -F '\t' '$2 == "lib" { print $1 }' <<< "$packages")
  if [ "$(wc -w <<< "$libs")" -ne 1 ]; then
    fail "$(basename "$slice"): $(wc -w <<< "$libs" | tr -d ' ') service libraries ($libs), not one"
    continue
  fi
  while IFS=$'\t' read -r name kind dir deps; do
    case "$kind" in
      bin) allowed="$libs rivium" ;;
      cdylib) allowed="$libs rivium-jni" ;;
      *) continue ;;
    esac
    before=$failures
    extra=""
    for dep in $deps; do
      case " $allowed " in *" $dep "*) ;; *) extra="$extra $dep" ;; esac
    done
    [ -z "$extra" ] || fail "$name ($kind) depends on:$extra; only on $allowed"
    case " $deps " in *" $libs "*) ;; *) fail "$name ($kind) does not depend on the service library $libs" ;; esac
    lines=$(code_lines "$dir/src")
    [ "$lines" -le "$max_lines" ] || fail "$name ($kind) has $lines lines of code, more than $max_lines"
    [ "$failures" -gt "$before" ] || echo "ok   $name ($kind): depends on $deps; $lines lines of code"
  done <<< "$packages"
done
[ "$failures" -eq 0 ] || { echo "check-slice-deps: $failures failure(s)"; exit 1; }
echo "check-slice-deps: passed"
