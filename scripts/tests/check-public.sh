#!/usr/bin/env bash
# Self-test for scripts/check-public.sh and the git hooks: plants violations of a synthetic
# denylist in throwaway repositories and asserts that each one is caught without the entry being
# printed. Never uses the real denylist.
set -euo pipefail
export LC_ALL=C
root=$(cd "$(dirname "$0")/../.." && pwd)
check="$root/scripts/check-public.sh"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-public-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

term1=planted-term-alpha
term2=植入词条
list="$tmp/denylist.txt"
printf '# synthetic denylist\r\n\r\n  %s  \r\n# another comment\n%s\n' "$term1" "$term2" > "$list"

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1"; failures=$((failures + 1)); }

# expect <exit-status> <name> <command...>: checks the status and that no entry is ever printed.
expect() {
  local want=$1 name=$2 got=0
  shift 2
  "$@" > "$tmp/out" 2>&1 || got=$?
  if [ "$got" != "$want" ]; then
    fail "$name (exit $got, want $want)"
    sed 's/^/     | /' "$tmp/out"
  elif grep -qiF -e "$term1" -e "$term2" "$tmp/out"; then
    fail "$name (output contains a denylist entry)"
  else
    pass "$name"
  fi
}

new_repo() {
  rm -rf "$tmp/repo"
  git init -q -b main "$tmp/repo"
  git -C "$tmp/repo" config user.name tester
  git -C "$tmp/repo" config user.email tester@example.invalid
  git -C "$tmp/repo" config commit.gpgsign false
}
commit() { # <message>
  git -C "$tmp/repo" add -A
  git -C "$tmp/repo" commit -q --no-verify -m "$1"
}
in_repo() { (cd "$tmp/repo" && "$@"); }

new_repo
echo clean > "$tmp/repo/a.txt"
commit "first"
expect 0 "clean history passes" in_repo "$check" "$list"

printf 'x\nmentions Planted-TERM-Alpha here\n' > "$tmp/repo/b.txt"
commit "second"
expect 1 "content, case-insensitive" in_repo "$check" "$list"
if grep -q '^b.txt:2: matches denylist entry #1$' "$tmp/out"; then
  pass "reports file, line and ordinal"
else
  fail "reports file, line and ordinal"
fi

rm "$tmp/repo/b.txt"
commit "third"
expect 1 "content removed later is still in history" in_repo "$check" "$list"
expect 0 "--range limited to the clean last commit" in_repo "$check" "$list" --range HEAD~1..HEAD

new_repo
echo clean > "$tmp/repo/a.txt"
commit "first"
git -C "$tmp/repo" mv a.txt "$term1.txt"
commit "rename only"
git -C "$tmp/repo" mv "$term1.txt" a.txt
commit "rename back"
expect 1 "same blob under a historical denied path" in_repo "$check" "$list"
expect 1 "selected historical tree checks every path" in_repo "$check" "$list" --range HEAD~2..HEAD~1
expect 0 "selected clean tree excludes old paths" in_repo "$check" "$list" --range HEAD~1..HEAD

# Git paths are NUL-delimited: tabs/newlines must not change the file scanned or its label.
mkdir "$tmp/path-fixture"
for special in 'with space' $'with\ttab' $'with\nnewline' $'trailing\n' '中文' 'with\backslash'; do
  new_repo
  path="$term2-$special"
  printf 'clean\n' > "$tmp/repo/$path"
  printf 'clean\n' > "$tmp/path-fixture/$path"
  commit "unusual path"
  expect 1 "history: unusual path is scanned" in_repo "$check" "$list"
  expect 1 "index: unusual path is scanned" in_repo "$check" "$list" --index
  expect 1 "directory: unusual path is scanned" "$check" "$list" --dir "$tmp/path-fixture"
  if grep -q ': path name matches denylist entry #2$' "$tmp/out"; then
    pass "unusual path reports a path match"
  else
    fail "unusual path reports a path match"
  fi
  rm "$tmp/path-fixture/$path"
  path="clean-$special"
  new_repo
  printf 'contains %s\n' "$term1" > "$tmp/repo/$path"
  printf 'contains %s\n' "$term1" > "$tmp/path-fixture/$path"
  git -C "$tmp/repo" add -A
  expect 1 "index: unusual path content is scanned" in_repo "$check" "$list" --index
  expect 1 "directory: unusual path content is scanned" "$check" "$list" --dir "$tmp/path-fixture"
  if grep -q ':1: matches denylist entry #1$' "$tmp/out"; then
    pass "unusual path reports a content match"
  else
    fail "unusual path reports a content match"
  fi
  printf 'clean\n' > "$tmp/repo/$path"
  printf 'clean\n' > "$tmp/path-fixture/$path"
  git -C "$tmp/repo" add -A
  expect 0 "index: clean unusual path passes" in_repo "$check" "$list" --index
  expect 0 "directory: clean unusual path passes" "$check" "$list" --dir "$tmp/path-fixture"
  rm "$tmp/path-fixture/$path"
done

new_repo
echo clean > "$tmp/repo/a.txt"
commit "subject mentions $term2"
expect 1 "commit message, non-ASCII entry" in_repo "$check" "$list"

new_repo
mkdir "$tmp/repo/dir-$term1"
echo clean > "$tmp/repo/dir-$term1/a.txt"
commit "path"
expect 1 "path name" in_repo "$check" "$list"
if grep -q '^dir-\*\*\*/a.txt: path name matches denylist entry #1$' "$tmp/out"; then
  pass "path name is masked"
else
  fail "path name is masked"
fi

new_repo
printf 'bin\000\001%s\002' "$term1" > "$tmp/repo/blob.bin"
commit "binary"
expect 1 "binary file" in_repo "$check" "$list"

new_repo
echo clean > "$tmp/repo/a.txt"
commit "first"
git -C "$tmp/repo" -c user.name="dev $term1" commit -q --no-verify --allow-empty -m "identity"
expect 1 "author identity" in_repo "$check" "$list"

new_repo
echo "staged $term2" > "$tmp/repo/a.txt"
git -C "$tmp/repo" add a.txt
expect 1 "--index: staged content" in_repo "$check" "$list" --index
echo clean > "$tmp/repo/a.txt"
git -C "$tmp/repo" add a.txt
expect 0 "--index: clean staged tree" in_repo "$check" "$list" --index

printf 'fix: something\n\nsee %s\n' "$term1" > "$tmp/msg"
expect 1 "--message" "$check" "$list" --message "$tmp/msg"

mkdir -p "$tmp/dir/sub"
echo "packaged $term2" > "$tmp/dir/sub/README.md"
expect 1 "--dir" "$check" "$list" --dir "$tmp/dir"

expect 2 "missing denylist fails closed" "$check" "$tmp/none.txt"
printf '# only comments\n\n' > "$tmp/empty.txt"
expect 2 "empty denylist fails closed" "$check" "$tmp/empty.txt"

# Hooks, end to end: a commit hook refuses, and pre-push refuses to publish a planted commit.
new_repo
mkdir "$tmp/repo/scripts"
cp -R "$root/scripts/check-public.sh" "$root/scripts/check-commit-msg.sh" "$root/scripts/hooks" "$tmp/repo/scripts/"
git -C "$tmp/repo" config core.hooksPath scripts/hooks
git -C "$tmp/repo" config rivium.denylist "$list"
echo clean > "$tmp/repo/a.txt"
git -C "$tmp/repo" add -A
expect 0 "hooks: clean commit accepted" git -C "$tmp/repo" commit -q -m "chore: clean"
expect 1 "hooks: commit-msg refuses" git -C "$tmp/repo" commit -q --allow-empty -m "chore: about $term1"
expect 1 "hooks: commit-msg refuses a non-conventional message" git -C "$tmp/repo" commit -q --allow-empty -m "clean"
echo "x $term2" > "$tmp/repo/b.txt"
git -C "$tmp/repo" add b.txt
expect 1 "hooks: pre-commit refuses" git -C "$tmp/repo" commit -q -m "chore: b"
git -C "$tmp/repo" commit -q --no-verify -m "chore: b" # bypass the commit hooks to test pre-push
git init -q --bare "$tmp/remote.git"
git -C "$tmp/repo" remote add origin "$tmp/remote.git"
expect 1 "hooks: pre-push refuses planted commit" git -C "$tmp/repo" push -q origin main
git -C "$tmp/repo" rm -q b.txt
git -C "$tmp/repo" commit -q -m "chore: remove"
expect 1 "hooks: pre-push still refuses (history)" git -C "$tmp/repo" push -q origin main
git -C "$tmp/repo" reset -q --hard HEAD~2
expect 0 "hooks: pre-push accepts clean history" git -C "$tmp/repo" push -q origin main
expect 1 "hooks: pre-push refuses a planted ref name" git -C "$tmp/repo" push -q origin "main:$term1"
git -C "$tmp/repo" config --unset rivium.denylist
expect 1 "hooks: no denylist configured fails closed" git -C "$tmp/repo" push -q origin main:other

if [ "$failures" -ne 0 ]; then
  echo "check-public self-test: $failures failure(s)"
  exit 1
fi
echo "check-public self-test: all passed"
