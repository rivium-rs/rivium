#!/usr/bin/env bash
# Shows what changed in the Rivium template between the version that generated this project
# (.rivium-template) and <version>: it generates the template at both versions with the answers
# recorded in .rivium-template and prints the difference as a patch, to merge by hand. Nothing in
# the project changes. Needs cargo-generate.
# RIVIUM_TEMPLATE_GIT names another repository of the template (default: Rivium's on GitHub);
# its tags are v<version>.
# Usage: scripts/template-diff.sh <version>
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: scripts/template-diff.sh <version>" >&2; exit 2; }
cd "$(dirname "$0")/.."
repo=${RIVIUM_TEMPLATE_GIT:-https://github.com/rivium-rs/rivium}
recorded() { # <key>: the value of a key in .rivium-template
  sed -n -E "s/^$1 = \"?([^\"]*)\"?\$/\1/p" .rivium-template | head -n 1
}
from=$(recorded version)
name=$(recorded project-name)
defines=()
for key in http jni bridge_package platforms component; do
  if grep -q "^$key = " .rivium-template; then defines+=(--define "$key=$(recorded "$key")"); fi
done
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for version in "$from" "$1"; do
  mkdir -p "$work/$version"
  cargo generate --git "$repo" --tag "v$version" template --name "$name" --destination "$work/$version" \
    --vcs none --silent "${defines[@]}" > "$work/$version.log" 2>&1 \
    || { cat "$work/$version.log" >&2; echo "template-diff: cannot generate the template at v$version" >&2; exit 1; }
done
echo "# The template's changes from $from to $1, as a patch against a project generated at $from"
cd "$work"
diff -ruN "$from/$name" "$1/$name" || true
