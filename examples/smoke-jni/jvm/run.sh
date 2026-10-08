#!/usr/bin/env bash
# Desktop-JVM smoke: loads the host cdylib from LIB_DIR into a JVM and checks the native methods;
# then loads it with a mismatched Java declaration, which must fail at load time.
# Usage: examples/smoke-jni/jvm/run.sh <dir containing libsmoke_jni.so|.dylib>
set -euo pipefail
lib_dir=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

javac -d "$out/ok" "$here/smoke/Smoke.java"
java -Djava.library.path="$lib_dir" -cp "$out/ok" smoke.Smoke

javac -d "$out/mismatch" "$here/mismatch/smoke/Smoke.java"
status=0
java -Djava.library.path="$lib_dir" -cp "$out/mismatch" smoke.Smoke || status=$?
if [ "$status" -ne 3 ]; then
  echo "jvm smoke: the mismatched declaration was not rejected (exit $status)" >&2
  exit 1
fi
echo "jvm smoke: passed"
