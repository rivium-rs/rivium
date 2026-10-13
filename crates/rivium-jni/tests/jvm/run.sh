#!/usr/bin/env bash
# JVM contract for a real application cdylib: normal, mismatched signatures and all panic modes.
# Requires a full JDK via JAVA_HOME. Java sources remain compatible with JDK 17; CI uses 21.
# Usage: run.sh <normal library dir> <library name> <bridge package> <fault library dir>
set -euo pipefail
[ $# -eq 4 ] || { echo "usage: run.sh <normal library dir> <library name> <bridge package> <fault library dir>" >&2; exit 2; }
: "${JAVA_HOME:?set JAVA_HOME to a full JDK}"
lib_dir=$(cd "$1" && pwd)
name=$2
pkg_dir=$3
fault_dir=$(cd "$4" && pwd)
[ "$lib_dir" != "$fault_dir" ] || { echo "normal and fault builds need different directories" >&2; exit 2; }
[[ "$pkg_dir" =~ ^[A-Za-z_][A-Za-z0-9_]*(/[A-Za-z_][A-Za-z0-9_]*)*$ ]] || { echo "invalid Java package" >&2; exit 2; }
pkg=${pkg_dir//\//.}
case "$(uname -s)" in
  Darwin) library="lib$name.dylib"; exe="" ;;
  Linux) library="lib$name.so"; exe="" ;;
  MINGW* | MSYS* | CYGWIN*) library="$name.dll"; exe=".exe" ;;
  *) echo "unsupported desktop platform" >&2; exit 2 ;;
esac
java="$JAVA_HOME/bin/java$exe"
javac="$JAVA_HOME/bin/javac$exe"
[ -x "$java" ] && [ -x "$javac" ] || { echo "JAVA_HOME must contain java and javac" >&2; exit 2; }
[ -f "$lib_dir/$library" ] && [ -f "$fault_dir/$library" ] || { echo "normal or fault cdylib is missing" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

compile() { # <output dir> <java file>...
  local out=$1 file
  shift
  mkdir -p "$work/src-$out/$pkg_dir" "$work/$out"
  for file in "$@"; do
    { printf 'package %s;\n\n' "$pkg"; cat "$here/$file"; } > "$work/src-$out/$pkg_dir/$(basename "$file")"
  done
  "$javac" --release 17 -encoding UTF-8 -proc:none --class-path "$work/$out" -d "$work/$out" "$work/src-$out/$pkg_dir"/*.java
}

run_java() {
  local status=0
  "$java" -Xcheck:jni --enable-native-access=ALL-UNNAMED -XX:-CreateCoredumpOnCrash \
    "-XX:ErrorFile=$work/hs_err_pid%p.log" "$@" > "$work/java.out" 2> "$work/java.err" || status=$?
  cat "$work/java.out"
  cat "$work/java.err" >&2
  if grep -Eq 'WARNING in native method|WARNING: JNI' "$work/java.out" "$work/java.err"; then
    echo "jvm contract: JNI checker warning" >&2
    return 1
  fi
  return "$status"
}

contract() { # <library dir> <mode> <methods that panic>
  local root
  root=$(mktemp -d "$work/root.XXXXXX")
  RIVIUM_JNI_PANIC=$3 run_java -Djava.library.path="$1" -cp "$work/ok" "$pkg.Contract" "$name" "$root" "$2"
}

compile ok RiviumBridge.java Contract.java
# A production build must ignore the fault-injection environment variable.
contract "$lib_dir" contract nativeStart,nativeStop,nativeStatus,nativeLastError,nativeVersion

compile mismatch mismatch/RiviumBridge.java
status=0
run_java -Djava.library.path="$lib_dir" -cp "$work/mismatch" "$pkg.RiviumBridge" "$name" || status=$?
if [ "$status" -ne 3 ]; then
  echo "jvm contract: a mismatched bridge was not rejected (exit $status)" >&2
  exit 1
fi

contract "$fault_dir" panics-a nativeStatus,nativeLastError
contract "$fault_dir" panics-b nativeStart,nativeStop,nativeVersion
echo "jvm contract: normal, mismatch and both panic modes passed"
