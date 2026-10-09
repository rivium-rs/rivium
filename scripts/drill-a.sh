#!/usr/bin/env bash
# Scenario A (V-7): a fix to Rivium reaches its consumers through `cargo update`. With a local
# registry `drill` (a static sparse index served by python3 -m http.server):
#   1. release N of the library crates carries a planted shutdown defect (fixtures/drill-a.patch:
#      background services are never asked to stop); release N+1 is this tree as it is;
#   2. two consumers, fixtures/consumer-process and fixtures/consumer-embedded, which differ from
#      real consumers only in `registry = "drill"`, resolve N: their lifecycle contract tests fail;
#   3. N+1 is published; the consumers run `cargo update` on the Rivium crates, fetch with
#      --locked and test offline: the contract tests pass;
#   4. the consumers' only change is their Cargo.lock;
#   5. cargo-semver-checks finds N+1 compatible with N, and fails on a planted breaking change
#      released as a patch.
# Needs python3 and cargo-semver-checks. CARGO_HOME defaults to .tools/drill-cargo, so that
# crates.io downloads are kept between runs; the drill registry's own cache is cleared each run.
# Usage: scripts/drill-a.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
server=""
cleanup() {
  if [ -n "$server" ]; then kill "$server" 2> /dev/null && wait "$server" 2> /dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
export CARGO_HOME=${CARGO_HOME_DRILL:-$root/.tools/drill-cargo}
export CARGO_TARGET_DIR=$root/target/drill-a
packages=(-p rivium-error -p rivium -p rivium-http -p rivium-jni -p rivium-test)
step() { echo; echo "drill-a: $*"; }

version=$(sed -n -E 's/^version = "([^"]+)"$/\1/p' "$root/Cargo.toml" | head -n 1)
next=${version%.*}.$((${version##*.} + 1))
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
index=sparse+http://127.0.0.1:$port/
export CARGO_REGISTRIES_DRILL_INDEX=$index
registry=$work/registry
mkdir -p "$registry"
printf '{"dl":"http://127.0.0.1:%s/crates/{crate}/{version}.crate","api":null}\n' "$port" > "$registry/config.json"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$registry" > "$work/server.log" 2>&1 &
server=$!
for _ in $(seq 50); do curl -fs "http://127.0.0.1:$port/config.json" > /dev/null && break; sleep 0.1; done
rm -rf "$CARGO_HOME"/registry/{index,cache,src}/127.0.0.1-*

# A copy of the library crates at <version>, depending on each other through the drill registry.
release() { # <dir> <version>
  mkdir -p "$1"
  # The crates' license files link to the top-level ones.
  (cd "$root" && git ls-files -z --cached --others --exclude-standard -- Cargo.toml Cargo.lock LICENSE-* crates) \
    | (cd "$root" && xargs -0 tar -cf -) | tar -xf - -C "$1"
  sed -i.bak -E "s/^version = \"$version\"$/version = \"$2\"/;
    s/^(rivium[a-z-]* = \\{ path = \"[^\"]*\", version = )\"$version\" \\}/\\1\"$2\", registry = \"drill\" }/" "$1/Cargo.toml"
  # The workspace's other members are not part of a release.
  sed -i.bak -E '/^members = \[/,/^\]/c\
members = ["crates/*"]' "$1/Cargo.toml"
}

# Packs the crates of <dir> and adds them to the registry's index.
publish() { # <dir>
  (cd "$1" && CARGO_TARGET_DIR=$1/target cargo package --quiet --no-verify --allow-dirty --registry drill "${packages[@]}")
  python3 - "$1/target/package" "$registry" "$index" <<'PY'
import hashlib, json, pathlib, sys, tarfile, tomllib
packages, registry, index = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
CRATES_IO = "https://github.com/rust-lang/crates.io-index"
for crate in sorted(packages.glob("*.crate")):
    with tarfile.open(crate) as tar:
        top = tar.getnames()[0].split("/")[0]
        manifest = tomllib.load(tar.extractfile(f"{top}/Cargo.toml"))
    package = manifest["package"]
    name, version = package["name"], package["version"]
    deps = []
    tables = [(None, manifest)] + [(cfg, t) for cfg, t in manifest.get("target", {}).items()]
    for target, table in tables:
        for kind, key in (("normal", "dependencies"), ("dev", "dev-dependencies"), ("build", "build-dependencies")):
            for dep_name, dep in table.get(key, {}).items():
                dep = {"version": dep} if isinstance(dep, str) else dep
                registry_index = dep.get("registry-index")
                deps.append({
                    "name": dep_name,
                    "req": dep.get("version", "*"),
                    "features": dep.get("features", []),
                    "optional": dep.get("optional", False),
                    "default_features": dep.get("default-features", True),
                    "target": target,
                    "kind": kind,
                    # A dependency of this registry has its index; any other comes from crates.io.
                    "registry": None if registry_index == index else (registry_index or CRATES_IO),
                    **({"package": dep["package"]} if "package" in dep else {}),
                })
    features = manifest.get("features", {})
    new_syntax = {k: v for k, v in features.items() if any("dep:" in f or "?/" in f for f in v)}
    entry = {
        "name": name, "vers": version, "deps": deps,
        "cksum": hashlib.sha256(crate.read_bytes()).hexdigest(),
        "features": {k: v for k, v in features.items() if k not in new_syntax},
        "features2": new_syntax, "yanked": False, "links": package.get("links"), "v": 2,
        "rust_version": package.get("rust-version"),
    }
    lower = name.lower()
    path = {1: f"1/{lower}", 2: f"2/{lower}", 3: f"3/{lower[0]}/{lower}"}.get(len(lower), f"{lower[:2]}/{lower[2:4]}/{lower}")
    (registry / path).parent.mkdir(parents=True, exist_ok=True)
    with open(registry / path, "a") as out:
        out.write(json.dumps(entry) + "\n")
    target = registry / "crates" / name / f"{version}.crate"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(crate.read_bytes())
    print(f"published {name} {version}")
PY
}

# Runs the consumers' tests; `expect` is pass or fail.
consumers_test() { # <pass|fail> [cargo test options]
  local expect=$1 name status
  shift
  for name in process embedded; do
    status=0
    (cd "$work/consumers/$name" && cargo test --locked "$@") > "$work/$name.log" 2>&1 || status=$?
    case "$expect:$status" in
      pass:0) echo "ok   consumer-$name passes its lifecycle contract test" ;;
      fail:0) cat "$work/$name.log"; echo "drill-a: consumer-$name passed against release $version, which has the defect" >&2; exit 1 ;;
      pass:*) cat "$work/$name.log"; echo "drill-a: consumer-$name fails against release $next" >&2; exit 1 ;;
      fail:*)
        grep -q "the_.*_lifecycle_contract.*FAILED" "$work/$name.log" \
          || { cat "$work/$name.log"; echo "drill-a: consumer-$name failed, but not its contract" >&2; exit 1; }
        echo "ok   consumer-$name fails its lifecycle contract test: $(grep -m1 -E 'SIGTERM|StopTimedOut' "$work/$name.log" | cut -c 1-120)"
        ;;
    esac
  done
}

step "release $version, with the planted defect"
release "$work/release-n" "$version"
git -C "$work/release-n" apply "$root/fixtures/drill-a.patch" 2> /dev/null || patch -s -p1 -d "$work/release-n" < "$root/fixtures/drill-a.patch"
publish "$work/release-n"

step "the consumers, at release $version"
mkdir -p "$work/consumers"
for name in process embedded; do cp -R "$root/fixtures/consumer-$name" "$work/consumers/$name"; done
git -C "$work/consumers" init -q
for name in process embedded; do (cd "$work/consumers/$name" && cargo generate-lockfile --quiet); done
git -C "$work/consumers" add -A
git -C "$work/consumers" -c user.name=drill -c user.email=drill@example.com commit -qm "at $version"
consumers_test fail

step "release $next, the fix"
release "$work/release-n1" "$next"
publish "$work/release-n1"

step "the consumers update the Rivium crates, fetch with --locked and test offline"
for name in process embedded; do
  dir=$work/consumers/$name
  # Every Rivium crate in the lockfile, together.
  updates=$(sed -n -E 's/^name = "(rivium(-[a-z]+)?)"$/-p \1/p' "$dir/Cargo.lock" | tr '\n' ' ')
  # shellcheck disable=SC2086
  (cd "$dir" && cargo update --quiet $updates && cargo fetch --locked --quiet)
done
consumers_test pass --offline
changed=$(git -C "$work/consumers" diff --name-only | tr '\n' ' ')
if [ "$changed" != "embedded/Cargo.lock process/Cargo.lock " ]; then
  echo "drill-a: the consumers changed more than their Cargo.lock: $changed" >&2
  exit 1
fi
echo "ok   only the consumers' Cargo.lock changed: $(git -C "$work/consumers" diff --shortstat)"

step "API compatibility of $next with $version"
semver() { # <new release dir> <-p crate>...: cargo-semver-checks against release N
  local dir=$1
  shift
  cargo semver-checks --manifest-path "$dir/Cargo.toml" --baseline-root "$work/release-n" "$@"
}
semver "$work/release-n1" "${packages[@]}" > "$work/semver.log" 2>&1 || { cat "$work/semver.log"; echo "drill-a: $next is not compatible with $version" >&2; exit 1; }
echo "ok   cargo-semver-checks: $next is compatible with $version"
# Planted violation: a breaking change released as a patch must fail the check.
cp -R "$work/release-n1" "$work/release-broken"
rm -rf "$work/release-broken/target"
sed -i.bak 's/    pub fn retry(&self) -> bool {/    pub(crate) fn retry(\&self) -> bool {/' "$work/release-broken/crates/rivium-error/src/error.rs"
if semver "$work/release-broken" -p rivium-error > "$work/semver-plant.log" 2>&1; then
  cat "$work/semver-plant.log"
  echo "drill-a: cargo-semver-checks accepted a breaking change released as a patch" >&2
  exit 1
fi
grep -q "inherent_method_missing" "$work/semver-plant.log" \
  || { cat "$work/semver-plant.log"; echo "drill-a: the planted breaking change failed otherwise" >&2; exit 1; }
echo "ok   cargo-semver-checks rejects a breaking change released as a patch"
echo
echo "drill-a: passed"
