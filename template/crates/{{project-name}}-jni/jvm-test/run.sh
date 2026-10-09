#!/usr/bin/env bash
# The desktop JVM contract of the JNI library: loads it into a JVM (JDK 17) through the Java
# mirror of the app's bridge class, RiviumBridge.java, and checks its native methods as the app
# calls them (Contract.java). Both files come from rivium-jni, without a package: this script adds
# the app's. The arguments after the library directory go to every start, as `--set` overrides
# that the defaults need in a test.
# Usage: bash crates/{{project-name}}-jni/jvm-test/run.sh <directory of lib{{crate_name}}_jni> [start argument]...
# (`just jvm` runs it.)
set -euo pipefail
[ $# -ge 1 ] || { echo "usage: run.sh <library dir> [start argument]..." >&2; exit 2; }
lib_dir=$(cd "$1" && pwd)
shift
pkg={{bridge_package}}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src=$work/src/${pkg//.//}
mkdir -p "$src" "$work/root"
for file in RiviumBridge.java Contract.java; do
  { printf 'package %s;\n\n' "$pkg"; cat "$here/$file"; } > "$src/$file"
done
javac -d "$work/classes" "$src"/*.java
java -Djava.library.path="$lib_dir" -cp "$work/classes" "$pkg.Contract" {{crate_name}}_jni "$work/root" contract "$@"
