#!/usr/bin/env bash
# Self-test for scripts/docs-only.sh: documentation-only changes are recognised, and planted changes
# to code paths (a packaged crate README, CI files, a code file moved into docs/, ...) are not.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/docs-only-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
failures=0

edit() { # <path>...: appends a line to each file, creating it and its directory if needed
  local path
  for path in "$@"; do
    mkdir -p "$(dirname "$path")"
    echo change >> "$path"
  done
}
on() { # <branch> <command>...: runs the command in the repository on <branch> and commits the result
  git -C "$repo" switch -q "$1"
  shift
  (cd "$repo" && "$@")
  git -C "$repo" add -A
  git -C "$repo" commit -q --no-verify --allow-empty -m change
}
check() { # <exit status> <name> [extra argument]: runs docs-only.sh [extra argument] main topic
  local got=0
  (cd "$repo" && "$root/scripts/docs-only.sh" "${@:3}" main topic) > "$tmp/out" 2>&1 || got=$?
  if [ "$got" = "$1" ]; then echo "ok   $1 <- $2"; else
    echo "FAIL $2 (exit $got, want $1)"
    sed 's/^/     | /' "$tmp/out"
    failures=$((failures + 1))
  fi
}
expect() { # <exit status> <name> <command>...: checks the command's change on a new branch from main
  local want=$1 name=$2
  shift 2
  git -C "$repo" switch -q -C topic main
  on topic "$@"
  check "$want" "$name"
}

code_paths=(crates/rivium/README.md crates/rivium/src/lib.rs examples/smoke/data/README.md
  .github/workflows/ci.yml .github/pull_request_template.md scripts/check.sh
  fixtures/minimal/Cargo.toml Cargo.toml Cargo.lock deny.toml Cross.toml justfile LICENSE-MIT .gitignore)
git init -q -b main "$repo"
git -C "$repo" config user.name tester
git -C "$repo" config user.email tester@example.invalid
git -C "$repo" config commit.gpgsign false
(cd "$repo" && edit README.md CONTRIBUTING.md docs/platforms.md "${code_paths[@]}")
git -C "$repo" add -A
git -C "$repo" commit -q --no-verify -m base

expect 0 "a file under docs/" edit docs/platforms.md
expect 0 "top-level Markdown files and a new file under docs/" edit README.md CONTRIBUTING.md docs/guide/new.md
expect 0 "a file under docs/ deleted" git rm -q docs/platforms.md
expect 0 "a file name with spaces under docs/" edit "docs/a b.md"
for path in "${code_paths[@]}"; do
  expect 1 "planted: $path" edit "$path"
done
expect 1 "planted: a new top-level directory" edit fixtures/minimal/notes.md
expect 1 "planted: a directory name that starts with docs" edit docs-old/notes.md
expect 1 "planted: documentation and one code file" edit docs/platforms.md Cargo.toml
expect 1 "planted: a code file moved into docs/" git mv crates/rivium/src/lib.rs docs/lib.rs
expect 1 "planted: no changed files" true

# The diff starts at the merge base: changes that main gained after the branch forked are not the
# branch's, whether or not the branch has merged main since.
git -C "$repo" switch -q -C topic main
on topic edit docs/platforms.md
on main edit crates/rivium/src/lib.rs
check 0 "code changed on main after the branch forked"
git -C "$repo" switch -q topic
git -C "$repo" merge -q --no-verify --no-edit main
check 0 "main merged into the branch"

check 2 "usage error" extra
git -C "$repo" switch -q main
git -C "$repo" branch -q -D topic
check 2 "unknown revision"

if [ "$failures" -ne 0 ]; then echo "docs-only self-test: $failures failure(s)"; exit 1; fi
echo "docs-only self-test: all passed"
