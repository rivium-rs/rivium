#!/usr/bin/env bash
# Desktop JVM contract of rivium-jni: loads a JNI library built with rivium_jni::export! into a
# JVM (JDK 17) and checks the native methods of its bridge class (Contract.java), then loads it
# with a bridge class that declares them otherwise, which must fail. Given a build of the same
# library with `--cfg rivium_jni_fault`, it also checks that panics in native methods return
# -99 or an empty string, are logged, and the JVM goes on. The Java files have no package: this
# script adds the package of the bridge class.
# Usage: run.sh <library dir> <library name> <bridge package, as com/example/svc> [<fault build's library dir>]
set -euo pipefail
[ $# -ge 3 ] || { echo "usage: run.sh <library dir> <library name> <bridge package> [<fault library dir>]" >&2; exit 2; }
lib_dir=$(cd "$1" && pwd)
name=$2
pkg_dir=$3
pkg=${pkg_dir//\//.}
fault_dir=""
if [ $# -ge 4 ]; then fault_dir=$(cd "$4" && pwd); fi
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

compile() { # <output dir> <java file>...: compiles the files in the bridge's package
  local out=$1 file
  shift
  mkdir -p "$work/src-$out/$pkg_dir"
  for file in "$@"; do
    { printf 'package %s;\n\n' "$pkg"; cat "$here/$file"; } > "$work/src-$out/$pkg_dir/$(basename "$file")"
  done
  javac -d "$work/$out" "$work/src-$out/$pkg_dir"/*.java
}

contract() { # <library dir> <mode> <methods that panic>: runs Contract below a new root
  local root
  root=$(mktemp -d "$work/root.XXXXXX")
  RIVIUM_JNI_PANIC=$3 java -Djava.library.path="$1" -cp "$work/ok" "$pkg.Contract" "$name" "$root" "$2"
}

compile ok RiviumBridge.java Contract.java
contract "$lib_dir" contract ""

compile mismatch mismatch/RiviumBridge.java
status=0
java -Djava.library.path="$lib_dir" -cp "$work/mismatch" "$pkg.RiviumBridge" "$name" || status=$?
if [ "$status" -ne 3 ]; then
  echo "jvm contract: a bridge class that declares the methods otherwise was not rejected (exit $status)" >&2
  exit 1
fi

if [ -n "$fault_dir" ]; then
  contract "$fault_dir" panics-a nativeStatus,nativeLastError
  contract "$fault_dir" panics-b nativeStart,nativeStop,nativeVersion
fi
echo "jvm contract: passed"
