#!/usr/bin/env bash
# rivium-jni's desktop JVM contract on the snmp-lite JNI library, as a project made from the
# template runs it: the reference bridge class and contract of rivium-jni, in the bridge's
# package, with the start arguments that the defaults need in a test (a free port in place of
# 161). The business tests in a JVM are in snmp-lite-bin/tests/hosts.rs.
# Usage: run.sh <directory of libsnmp_lite_jni>
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: run.sh <library dir>" >&2; exit 2; }
lib_dir=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
reference=$here/../../../../../crates/rivium-jni/tests/jvm
pkg=com.example.snmplite
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src=$work/src/${pkg//.//}
mkdir -p "$src" "$work/root"
for file in RiviumBridge.java Contract.java; do
  { printf 'package %s;\n\n' "$pkg"; cat "$reference/$file"; } > "$src/$file"
done
javac -d "$work/classes" "$src"/*.java
java -Djava.library.path="$lib_dir" -cp "$work/classes" "$pkg.Contract" snmp_lite_jni "$work/root" contract \
  --set snmp.addr=127.0.0.1:0
