#!/usr/bin/env bash
# Self-test of scripts/check-slice-deps.sh: a scratch workspace with one slice (service library,
# program, JNI library) passes, and each planted violation fails with its message.
set -euo pipefail
check=$(cd "$(dirname "$0")/.." && pwd)/check-slice-deps.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

package() { # <dir> <name> <kind: lib|bin|cdylib> <dependencies (TOML lines)> [<source>]
  mkdir -p "$1/src"
  local target=""
  case "$3" in
    bin) target=$(printf '[[bin]]\nname = "%s"\npath = "src/main.rs"' "$2") ;;
    cdylib) target=$(printf '[lib]\ncrate-type = ["cdylib"]') ;;
  esac
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\npublish = false\n\n%s\n\n[dependencies]\n%s\n' \
    "$2" "$target" "$4" > "$1/Cargo.toml"
  local file=src/lib.rs
  [ "$3" != bin ] || file=src/main.rs
  printf '%s\n' "${5:-// A test package.}" > "$1/$file"
}

base() { # <dir>: a workspace whose slice keeps the rules
  printf '[workspace]\nresolver = "3"\nmembers = ["crates/*", "examples/slices/*/*"]\n' > "$1/Cargo.toml"
  for crate in rivium rivium-jni rivium-http; do package "$1/crates/$crate" "$crate" lib ""; done
  local slice=$1/examples/slices/demo
  package "$slice/demo" demo lib 'rivium = { path = "../../../../crates/rivium" }'
  package "$slice/demo-bin" demo-bin bin $'demo = { path = "../demo" }\nrivium = { path = "../../../../crates/rivium" }\n\n[dev-dependencies]\nrivium-http = { path = "../../../../crates/rivium-http" }' \
    $'//! The program.\n\nfn main() {\n    demo::run();\n}'
  package "$slice/demo-jni" demo-jni cdylib $'demo = { path = "../demo" }\nrivium-jni = { path = "../../../../crates/rivium-jni" }' \
    $'//! The JNI library.\n#![forbid(unsafe_code)]\n\npub fn on_load() {}'
}

case_() { # <name> <expected exit status> <expected line or ""> <change to make in the workspace>
  local dir=$work/$1 status=0
  mkdir -p "$dir"
  base "$dir"
  (cd "$dir" && eval "$4")
  "$check" "$dir" > "$dir.out" 2>&1 || status=$?
  if [ "$status" -ne "$2" ] || { [ -n "$3" ] && ! grep -qF -- "$3" "$dir.out"; }; then
    echo "FAIL $1: exit $status (want $2)"
    cat "$dir.out"
    failures=$((failures + 1))
  else
    echo "ok   $1"
  fi
}

slice=examples/slices/demo
case_ clean 0 "ok   demo-jni (cdylib): depends on demo rivium-jni; 2 lines of code" ":"
case_ program-with-http 1 "FAIL demo-bin (bin) depends on: rivium-http; only on demo rivium" \
  "sed -i.bak '/^\[dev-dependencies\]/,\$d' $slice/demo-bin/Cargo.toml && printf 'rivium-http = { path = \"../../../../crates/rivium-http\" }\n' >> $slice/demo-bin/Cargo.toml"
case_ jni-with-rivium 1 "FAIL demo-jni (cdylib) depends on: rivium; only on demo rivium-jni" \
  "printf 'rivium = { path = \"../../../../crates/rivium\" }\n' >> $slice/demo-jni/Cargo.toml"
case_ jni-without-library 1 "FAIL demo-jni (cdylib) does not depend on the service library demo" \
  "sed -i.bak '/^demo = /d' $slice/demo-jni/Cargo.toml"
case_ long-program 1 "FAIL demo-bin (bin) has 11 lines of code, more than 10" \
  "for i in 1 2 3 4 5 6 7 8; do printf 'const C%s: u8 = 0;\n' \$i >> $slice/demo-bin/src/main.rs; done"
case_ two-libraries 1 "FAIL demo: 2 service libraries" \
  "mkdir -p $slice/other/src && printf '[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n' > $slice/other/Cargo.toml && : > $slice/other/src/lib.rs"
[ "$failures" -eq 0 ] || { echo "check-slice-deps self-test: $failures failure(s)"; exit 1; }
echo "check-slice-deps self-test: passed"
