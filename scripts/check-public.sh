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
mkdir "$work/files"

# Entries, one per line, trimmed. Ordinals count entries only (comments and blanks are skipped).
awk '{ sub(/\r$/, ""); gsub(/^[ \t]+|[ \t]+$/, "") } $0 != "" && substr($0, 1, 1) != "#"' \
  "$list" > "$work/entries"
[ -s "$work/entries" ] || die "denylist has no entries: $list"

# Scanned items: files/<n> holds the content, labels has "<n><TAB><kind><TAB><label>" where kind is
# `file` (label is a path, masked on output) or `meta` (label is printed as is). Callers write the
# content to "$work/files/$n" right after new_item; nothing here runs in a pipeline subshell, so the
# counter stays in this shell.
n=0
new_item() { # <kind> <label>
  n=$((n + 1))
  printf '%s\t%s\t%s\n' "$n" "$1" "$2" >> "$work/labels"
}
: > "$work/labels"
: > "$work/paths"

add_blobs() { # <file of "<sha> <path>" lines>; each distinct blob is added once, labelled by its first path
  local sha path
  while IFS=' ' read -r sha path; do
    [ -n "$sha" ] || continue
    printf '%s\n' "$path" >> "$work/paths"
    if [ ! -e "$work/seen.$sha" ]; then
      : > "$work/seen.$sha"
      new_item file "$path"
      git cat-file blob "$sha" > "$work/files/$n"
    fi
  done < "$1"
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
    done < "$work/commits"
    git rev-list --objects "$@" | awk 'NF >= 2' > "$work/objects"
    cut -d' ' -f1 "$work/objects" | git cat-file --batch-check='%(objectname) %(objecttype)' \
      | awk '$2 == "blob" { print $1 }' > "$work/blobs"
    # Trees: their path names are checked, their content is not (it is just names and modes).
    awk 'NR == FNR { blob[$1] = 1; next } !($1 in blob) { sub(/^[^ ]+ /, ""); print }' \
      "$work/blobs" "$work/objects" >> "$work/paths"
    awk 'NR == FNR { blob[$1] = 1; next } $1 in blob' "$work/blobs" "$work/objects" > "$work/blob-paths"
    add_blobs "$work/blob-paths"
    what="$(wc -l < "$work/commits" | tr -d ' ') commit(s)"
    ;;
  --index)
    git -c core.quotePath=false ls-files -s \
      | awk '{ sha = $2; sub(/^[^\t]*\t/, ""); print sha " " $0 }' > "$work/blob-paths"
    add_blobs "$work/blob-paths"
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
    (cd "$2" && find . -type f | sed 's|^\./||' | sort) > "$work/dirfiles"
    while IFS= read -r f; do
      printf '%s\n' "$f" >> "$work/paths"
      new_item file "$f"
      cat "$2/$f" > "$work/files/$n"
    done < "$work/dirfiles"
    what="$(wc -l < "$work/dirfiles" | tr -d ' ') file(s) under $2"
    ;;
  *)
    die "unknown mode: $mode"
    ;;
esac

# Raw hits: "<entry ordinal><TAB><item>:<line>" for content, "<entry ordinal><TAB>path:<n>" for names.
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
  rc=0
  out=$(grep -naiF -e "$entry" -- "$work/paths") || rc=$?
  [ "$rc" -le 1 ] || die "grep failed while scanning path names"
  [ -z "$out" ] || printf '%s\n' "$out" | awk -F: -v i="$i" '{ print i "\tpath:" $1 }' >> "$work/hits"
done < "$work/entries"

if [ ! -s "$work/hits" ]; then
  printf 'check-public: clean (%s, %s entries)\n' "$what" "$(wc -l < "$work/entries" | tr -d ' ')"
  exit 0
fi

awk -F'\t' -v entries="$work/entries" -v labels="$work/labels" -v paths="$work/paths" '
  function mask(s,    out, low, j, e, p) {
    for (j = 1; j <= ne; j++) {
      e = ent[j]
      while ((p = index(tolower(s), e)) > 0) s = substr(s, 1, p - 1) "***" substr(s, p + length(e))
    }
    return s
  }
  BEGIN {
    while ((getline line < entries) > 0) ent[++ne] = tolower(line)
    while ((getline line < labels) > 0) { split(line, f, "\t"); kind[f[1]] = f[2]; label[f[1]] = f[3] }
    while ((getline line < paths) > 0) path[++np] = line
  }
  {
    split($2, loc, ":")
    if (loc[1] == "path") {
      printf "%s: path name matches denylist entry #%s\n", mask(path[loc[2]]), $1
    } else {
      where = kind[loc[1]] == "file" ? mask(label[loc[1]]) : label[loc[1]]
      printf "%s:%s: matches denylist entry #%s\n", where, loc[2], $1
    }
  }' "$work/hits" | sort -u
printf 'check-public: FAILED in %s; entries are not printed, look them up in the private denylist\n' "$what" >&2
exit 1
