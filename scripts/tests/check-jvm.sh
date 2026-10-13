#!/usr/bin/env bash
# Input failures must stop the JVM runner before it starts Java, even without a JDK installed.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
runner="$root/crates/rivium-jni/tests/jvm/run.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/normal" "$work/fault" "$work/jdk/bin"
cat > "$work/jdk/bin/java" <<'JAVA'
#!/usr/bin/env bash
echo 'unexpected Java execution' >&2
exit 99
JAVA
cp "$work/jdk/bin/java" "$work/jdk/bin/javac"
chmod +x "$work/jdk/bin/java" "$work/jdk/bin/javac"
# The Windows path uses explicit .exe suffixes; these stand-ins must never be executed.
cp "$work/jdk/bin/java" "$work/jdk/bin/java.exe"
cp "$work/jdk/bin/javac" "$work/jdk/bin/javac.exe"

reject() { # <expected diagnostic> <arguments...>
  local expected=$1 status=0
  shift
  JAVA_HOME="$work/jdk" "$runner" "$@" > "$work/output" 2>&1 || status=$?
  [ "$status" -eq 2 ] || { cat "$work/output" >&2; echo "expected input failure, got $status" >&2; exit 1; }
  grep -Fq "$expected" "$work/output"
  if grep -Fq 'unexpected Java execution' "$work/output"; then
    cat "$work/output" >&2
    exit 1
  fi
}

reject 'usage:' "$work/normal" test_jni rivium/tests
reject 'different directories' "$work/normal" test_jni rivium/tests "$work/normal"
reject 'invalid Java package' "$work/normal" test_jni ../escape "$work/fault"
reject 'cdylib is missing' "$work/normal" test_jni rivium/tests "$work/fault"
echo 'JVM runner input checks: passed'
