#!/usr/bin/env bash
# Public-content check: fails when anything that is (or will be) public contains an entry of a
# private denylist. Matching is case-insensitive fixed-string substring matching (grep -iF).
#
# Usage:
#   scripts/check-public.sh <denylist>                     all commits reachable from HEAD
#   scripts/check-public.sh <denylist> --range <args>...   commits selected by `git rev-list <args>`
#   scripts/check-public.sh <denylist> --index             staged tree and configured identity
#   scripts/check-public.sh <denylist> --message <file>    a commit message
#   scripts/check-public.sh <denylist> --dir <dir>         every file under <dir> (e.g. an unpacked .crate)
#
# For commits, every blob that any selected commit's tree contains is checked (so content that was
# added and later removed is still found), together with path names and the raw commit object
# (author, committer, message).
#
# Denylist format: one entry per line; blank lines and lines starting with `#` are ignored.
# Output never contains an entry: a hit is reported as its location plus the entry's ordinal
# number, and path names are printed with matching parts masked, because CI logs are public.
# Exit status: 0 clean, 1 hits found, 2 usage or denylist error (a missing or empty list fails).
set -euo pipefail
export LC_ALL=C

die() {
  printf 'check-public: %s\n' "$*" >&2
  exit 2
}

[ $# -ge 1 ] || die "usage: check-public.sh <denylist> [--range <rev-list args>... | --index | --message <file> | --dir <dir>]"
list=$1
shift
[ -f "$list" ] || die "denylist not found: $list"

work=$(mktemp -d "${TMPDIR:-/tmp}/check-public.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/files" "$work/labels"

# Entries, one per line, trimmed. Ordinals count entries only (comments and blanks are skipped).
awk '{ sub(/\r$/, ""); gsub(/^[ \t]+|[ \t]+$/, "") } $0 != "" && substr($0, 1, 1) != "#"' \
  "$list" > "$work/entries"
[ -s "$work/entries" ] || die "denylist has no entries: $list"

# Scanned items: files/<n> holds content, labels/<n> the raw label, and items the numeric id/kind.
# Paths are separate items so every name is checked even when several trees share a blob.
# Labels are separate files because Git permits tabs and newlines in path names. Callers write the
# content to "$work/files/$n" right after new_item; nothing here runs in a pipeline subshell, so the
# counter stays in this shell.
n=0
new_item() { # <kind> <label>
  n=$((n + 1))
  printf '%s\t%s\n' "$n" "$1" >> "$work/items"
  printf '%s\n' "$2" > "$work/labels/$n"
}
: > "$work/items"

add_path() {
  new_item path "$1"
  printf '%s\n' "$1" > "$work/files/$n"
}

add_blob() { # <sha> <path>; content is checked once, independently of path names
  if [ ! -e "$work/seen.$1" ]; then
    : > "$work/seen.$1"
    new_item file "$2"
    git cat-file blob "$1" > "$work/files/$n"
  fi
}

mode=${1:-}
case "$mode" in
  "" | --range)
    [ "$mode" = "" ] || shift
    if [ "$mode" = "" ]; then
      git rev-parse --verify -q HEAD > /dev/null || { echo "check-public: no commits to check"; exit 0; }
      set -- HEAD
    fi
    [ $# -ge 1 ] || die "--range needs git rev-list arguments"
    git rev-list "$@" > "$work/commits"
    while read -r c; do
      new_item meta "commit $(git rev-parse --short "$c")"
      git cat-file commit "$c" > "$work/files/$n"
      # rev-list --objects gives only one name per object, hiding same-blob renames.
      git ls-tree -r -t --full-tree -z "$c" > "$work/tree"
      while IFS= read -r -d '' record; do
        metadata=${record%%$'\t'*}
        path=${record#*$'\t'}
        read -r _ type sha <<< "$metadata"
        add_path "$path"
        [ "$type" != blob ] || add_blob "$sha" "$path"
      done < "$work/tree"
    done < "$work/commits"
    what="$(wc -l < "$work/commits" | tr -d ' ') commit(s)"
    ;;
  --index)
    git ls-files -s -z > "$work/index"
    while IFS= read -r -d '' record; do
      metadata=${record%%$'\t'*}
      path=${record#*$'\t'}
      read -r mode sha _ <<< "$metadata"
      add_path "$path"
      [ "$mode" = 160000 ] || add_blob "$sha" "$path"
    done < "$work/index"
    new_item meta "author/committer identity"
    { git var GIT_AUTHOR_IDENT; git var GIT_COMMITTER_IDENT; } > "$work/files/$n"
    what="staged tree"
    ;;
  --message)
    [ $# -eq 2 ] || die "--message needs a file"
    new_item meta "commit message"
    cat "$2" > "$work/files/$n"
    what="commit message"
    ;;
  --dir)
    [ $# -eq 2 ] || die "--dir needs a directory"
    [ -d "$2" ] || die "not a directory: $2"
    (cd "$2" && find . -type f -print0) > "$work/dirfiles"
    count=0
    while IFS= read -r -d '' f; do
      f=${f#./}
      count=$((count + 1))
      add_path "$f"
      new_item file "$f"
      cat "$2/$f" > "$work/files/$n"
    done < "$work/dirfiles"
    what="$count file(s)"
    ;;
  *)
    die "unknown mode: $mode"
    ;;
esac

# Raw hits: "<entry ordinal><TAB><item>:<line>" for content or path names.
# grep output carries the matched line; it is cut down to the location before anything is printed.
: > "$work/hits"
i=0
while IFS= read -r entry; do
  i=$((i + 1))
  rc=0
  out=$(cd "$work/files" && grep -rnaiF -e "$entry" -- .) || rc=$?
  [ "$rc" -le 1 ] || die "grep failed while scanning"
  [ -z "$out" ] || printf '%s\n' "$out" \
    | awk -F: -v i="$i" '{ sub(/^\.\//, "", $1); print i "\t" $1 ":" $2 }' >> "$work/hits"
done < "$work/entries"

if [ ! -s "$work/hits" ]; then
  printf 'check-public: clean (%s, %s entries)\n' "$what" "$(wc -l < "$work/entries" | tr -d ' ')"
  exit 0
fi

awk -F'\t' -v entries="$work/entries" -v labels="$work/labels" -v items="$work/items" '
  function mask(s,    out, j, e, p) {
    for (j = 1; j <= ne; j++) {
      e = ent[j]
      out = ""
      while ((p = index(tolower(s), e)) > 0) {
        out = out substr(s, 1, p - 1) "***"
        s = substr(s, p + length(e))
      }
      s = out s
    }
    return s
  }
  function display(s,    out, j, c) {
    out = ""
    for (j = 1; j <= length(s); j++) {
      c = substr(s, j, 1)
      out = out (c == "\n" ? "\\n" : c == "\t" ? "\\t" : c == "\r" ? "\\r" : c == "\\" ? "\\\\" : c)
    }
    return out
  }
  function label(id,    file, line, s, first) {
    file = labels "/" id
    first = 1
    while ((getline line < file) > 0) {
      s = s (first ? "" : "\n") line
      first = 0
    }
    close(file)
    return display(mask(s))
  }
  BEGIN {
    while ((getline line < entries) > 0) ent[++ne] = tolower(line)
    while ((getline line < items) > 0) { split(line, f, "\t"); kind[f[1]] = f[2] }
  }
  {
    split($2, loc, ":")
    where = label(loc[1])
    if (kind[loc[1]] == "path") {
      printf "%s: path name matches denylist entry #%s\n", where, $1
    } else {
      printf "%s:%s: matches denylist entry #%s\n", where, loc[2], $1
    }
  }' "$work/hits" | sort -u
printf 'check-public: FAILED in %s; entries are not printed, look them up in the private denylist\n' "$what" >&2
exit 1
