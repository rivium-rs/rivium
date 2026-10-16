#!/usr/bin/env bash
set -euo pipefail
: "${JAVA_HOME:?Set JAVA_HOME to a complete JDK 17 or later}"
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
case "$(uname -s)" in
  Darwin) library=libtest_jni.dylib ;;
  Linux) library=libtest_jni.so ;;
  *) library=test_jni.dll ;;
esac
[ "$#" -le 2 ] || { echo "usage: test-jvm.sh [normal-dir [fault-dir]]" >&2; exit 2; }
normal=${1:-target/debug}
fault=${2:-}
case "$normal" in /*) normal_dir=$normal ;; *) normal_dir="$root/$normal" ;; esac
[ -s "$normal_dir/$library" ] || { echo 'normal cdylib is missing' >&2; exit 2; }
fault_dir=
if [ -n "$fault" ]; then
  case "$fault" in /*) fault_dir=$fault ;; *) fault_dir="$root/$fault" ;; esac
  [ "$normal_dir" != "$fault_dir" ] || { echo 'normal and fault builds need different directories' >&2; exit 2; }
  [ -s "$fault_dir/$library" ] || { echo 'fault cdylib is missing' >&2; exit 2; }
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
"$JAVA_HOME/bin/javac" --release 17 -d "$work/classes" crates/rivium-jni/tests/java/rivium/tests/Bridge.java
if ! "$JAVA_HOME/bin/java" --enable-native-access=ALL-UNNAMED -Xcheck:jni -cp "$work/classes" rivium.tests.Bridge "$normal_dir/$library" "$work/日志😀" > "$work/normal.log" 2>&1; then
  cat "$work/normal.log" >&2
  exit 1
fi
cat "$work/normal.log"
if grep -E 'WARNING in native method|FATAL ERROR in native method' "$work/normal.log"; then exit 1; fi
"$JAVA_HOME/bin/javac" --release 17 -d "$work/mismatch" crates/rivium-jni/tests/java-mismatch/rivium/tests/Bridge.java
if ! "$JAVA_HOME/bin/java" --enable-native-access=ALL-UNNAMED -Xcheck:jni -cp "$work/mismatch" rivium.tests.Bridge "$normal_dir/$library" > "$work/mismatch.log" 2>&1; then
  cat "$work/mismatch.log" >&2
  exit 1
fi
cat "$work/mismatch.log"
if grep -E 'WARNING in native method|FATAL ERROR in native method' "$work/mismatch.log"; then exit 1; fi
if [ -n "$fault" ]; then
  if ! RIVIUM_JNI_PANIC=nativeStart,nativeStop,nativeStatus,nativeLastError,nativeVersion \
    "$JAVA_HOME/bin/java" --enable-native-access=ALL-UNNAMED -Xcheck:jni -cp "$work/classes" rivium.tests.Bridge "$fault_dir/$library" "$work/fault" fault > "$work/fault.log" 2>&1; then
    cat "$work/fault.log" >&2
    exit 1
  fi
  cat "$work/fault.log"
  if grep -E 'WARNING in native method|FATAL ERROR in native method' "$work/fault.log"; then exit 1; fi
fi
