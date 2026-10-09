#!/usr/bin/env bash
# Makes the offline bundle of the old-world LoongArch record (V-17), for a native machine without
# network: the source of a commit, every crate its Cargo.lock names (cargo vendor) with a cargo
# configuration that uses only them, and run.sh, which builds, tests and checks it there
# (scripts/old-world/run.sh). The toolchain directory on that machine comes from
# RIVIUM_V17_TOOLCHAIN, set in the environment or in .tools/local.env (ignored by git).
# Usage: scripts/old-world/bundle.sh [<commit>]   (default HEAD)
#   → target/old-world/rivium-v17-<commit>.tar.gz; on the machine:
#     tar xzf rivium-v17-<commit>.tar.gz && bash rivium-v17-<commit>/run.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
if [ -z "${RIVIUM_V17_TOOLCHAIN:-}" ] && [ -f "$root/.tools/local.env" ]; then
  # shellcheck disable=SC1091
  . "$root/.tools/local.env"
fi
[ -n "${RIVIUM_V17_TOOLCHAIN:-}" ] || {
  echo "bundle: set RIVIUM_V17_TOOLCHAIN to the toolchain directory on the machine (in .tools/local.env)" >&2
  exit 2
}
rev=$(git -C "$root" rev-parse --verify "${1:-HEAD}^{commit}")
name=rivium-v17-${rev:0:10}
out=$root/target/old-world
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dir=$work/$name
mkdir -p "$dir"

git -C "$root" archive --prefix=src/ "$rev" | tar -x -C "$dir"
[ -f "$dir/src/scripts/old-world/run.sh" ] || { echo "bundle: $rev has no scripts/old-world/run.sh" >&2; exit 2; }
(cd "$dir/src" && cargo vendor --locked --versioned-dirs --quiet ../vendor > /dev/null)
mkdir -p "$dir/src/.cargo"
cat > "$dir/src/.cargo/config.toml" <<'EOF'
# Written by scripts/old-world/bundle.sh: every crate comes from ../vendor, never the network.
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "../vendor"

[net]
offline = true
EOF
command cp -f "$dir/src/scripts/old-world/run.sh" "$dir/run.sh"
printf '%s\n' "$RIVIUM_V17_TOOLCHAIN" > "$dir/toolchain"
{
  echo "commit $rev ($(git -C "$root" show -s --format='%cs %s' "$rev"))"
  echo "bundled $(date -u +%FT%TZ) with $(cargo -V)"
  echo "crates $(find "$dir/vendor" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
} > "$dir/BUNDLE"

mkdir -p "$out"
# No macOS metadata in the archive: GNU tar on the machine would warn about every entry.
COPYFILE_DISABLE=1 tar -C "$work" --no-mac-metadata --no-xattrs -czf "$out/$name.tar.gz" "$name" 2> /dev/null ||
  COPYFILE_DISABLE=1 tar -C "$work" -czf "$out/$name.tar.gz" "$name"
sum=$(shasum -a 256 "$out/$name.tar.gz" | cut -d ' ' -f 1)
echo "$sum  $name.tar.gz" > "$out/$name.tar.gz.sha256"
cat "$dir/BUNDLE"
echo "bundle: $out/$name.tar.gz ($(($(wc -c < "$out/$name.tar.gz") / 1048576)) MiB, sha256 $sum)"
echo "on the machine: tar xzf $name.tar.gz && bash $name/run.sh"
