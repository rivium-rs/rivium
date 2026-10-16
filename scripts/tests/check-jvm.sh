#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/jdk/bin"
printf '#!/bin/sh\necho unexpected-Java-execution\nexit 99\n' > "$work/jdk/bin/java"
cp "$work/jdk/bin/java" "$work/jdk/bin/javac"
chmod +x "$work/jdk/bin/"*
if JAVA_HOME="$work/jdk" "$root/scripts/test-jvm.sh" "$work/missing" > "$work/result" 2>&1; then exit 1; fi
grep -q 'cdylib is missing' "$work/result"
if grep -q unexpected-Java-execution "$work/result"; then exit 1; fi
echo 'JVM artifact preflight passed'
