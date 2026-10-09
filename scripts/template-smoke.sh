#!/usr/bin/env bash
# ci/template: generates projects from template/ with cargo-generate, points their rivium
# dependencies at this workspace with [patch.crates-io], and checks them as their own CI would:
#   - the template's version and dependencies match the workspace's, its manifests are
#     Cargo.toml.liquid (no Cargo.toml for cargo to parse in a git checkout), and its JVM files are
#     rivium-jni's reference files;
#   - two variants, plain (no HTTP, no JNI) and full (HTTP and JNI): `just check`, with port 8080
#     taken so that the full variant's tests must pass its port override; the full variant's
#     JVM contract; packaging for linux_amd64 and linux_arm32hf (glibc 2.17, and the arm32hf
#     configuration restarting in the process); `template-diff` against a repository with two
#     tags; at most 300 lines of Rust besides the business samples;
#   - planted violations that must fail: a detached thread (clippy), panic = "abort", a stale
#     configs/default.toml, an OpenSSL dependency (cargo-deny).
# Needs cargo-generate, just, cargo-deny and shellcheck; packaging needs cargo-zigbuild (and zig),
# the JVM contract a JDK 17. Outside CI, steps whose tools are missing are skipped; in CI they fail.
# Usage: scripts/template-smoke.sh [<output dir>]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$(mktemp -d)}
mkdir -p "$out"
out=$(cd "$out" && pwd)
export CARGO_TARGET_DIR=$out/target
failures=0
fail() { echo "FAIL $*"; failures=$((failures + 1)); }
ok() { echo "ok   $*"; }
has() { # <tool>...: true when every tool is there; outside CI a missing one skips the step
  local tool
  for tool in "$@"; do
    if ! command -v "$tool" > /dev/null; then
      [ "${CI:-}" = true ] && { fail "$tool is not installed"; return 1; }
      echo "skip ($tool is not installed)"
      return 1
    fi
  done
}

version=$(sed -n -E 's/^version = "([^"]+)"$/\1/p' "$root/Cargo.toml" | head -n 1)
minor=${version%.*}
tpl=$root/template
if grep -q "^version = \"$version\"$" "$tpl/.rivium-template"; then ok "template version $version"; else fail "template version is not $version"; fi
# cargo parses every Cargo.toml in a git dependency's checkout: the template's are .liquid.
manifests=$(find "$tpl" -name Cargo.toml)
if [ -z "$manifests" ]; then ok "the template's manifests are Cargo.toml.liquid"; else fail "rename to Cargo.toml.liquid: $manifests"; fi
for file in RiviumBridge.java Contract.java; do
  if cmp -s "$tpl/crates/{{project-name}}-jni/jvm-test/$file" "$root/crates/rivium-jni/tests/jvm/$file"; then
    ok "$file is rivium-jni's"
  else
    fail "template's $file differs from crates/rivium-jni/tests/jvm/$file"
  fi
done

generate() { # <name> [--define key=value]...
  local name=$1
  shift
  rm -rf "${out:?}/$name"
  cargo generate --path "$tpl" --name "$name" --destination "$out" --vcs none --silent "$@" > "$out/$name.log" 2>&1 \
    || { cat "$out/$name.log"; fail "cargo generate $name"; return 1; }
  {
    echo
    echo "[patch.crates-io]"
    for crate in rivium-error rivium rivium-http rivium-jni rivium-test; do
      echo "$crate = { path = \"$root/crates/$crate\" }"
    done
  } >> "$out/$name/Cargo.toml"
}
in_project() { (cd "$out/$1" && shift && "$@"); }

generate plain-svc
generate full-svc --define http=true --define jni=true --define bridge_package=com.example.full
deps=$(grep -cE "^rivium(-[a-z]+)? = \"$minor\"\$" "$out/full-svc/Cargo.toml" || true)
if [ "$deps" -eq 4 ]; then ok "the projects depend on rivium $minor"; else fail "the full project's rivium dependencies are not \"$minor\""; fi
for name in plain-svc full-svc; do (cd "$out/$name" && cargo generate-lockfile --quiet); done
if has shellcheck; then
  if (cd "$out/full-svc" && shellcheck scripts/*.sh crates/*/jvm-test/run.sh); then ok "the projects' scripts pass shellcheck"; else fail "shellcheck"; fi
fi

# Rust besides the business samples (heartbeat, api): non-blank lines that are not comments.
lines=$(find "$out/full-svc/crates" -name '*.rs' ! -name heartbeat.rs ! -name api.rs -print0 \
  | xargs -0 cat | grep -cvE '^[[:space:]]*(//.*)?$')
if [ "$lines" -le 300 ]; then ok "full-svc has $lines lines of Rust besides the samples"; else fail "full-svc has $lines lines of Rust (more than 300)"; fi

if has just cargo-deny; then
  for name in plain-svc full-svc; do
    taker=""
    if [ "$name" = full-svc ]; then
      # Port 8080, the default address, is taken: the tests must pass their override.
      python3 -c 'import socket, time; s = socket.socket(); s.bind(("127.0.0.1", 8080)); s.listen(); time.sleep(3600)' &
      taker=$!
    fi
    if in_project "$name" just check > "$out/$name-check.log" 2>&1; then ok "$name: just check"; else tail -n 60 "$out/$name-check.log"; fail "$name: just check"; fi
    [ -z "$taker" ] || kill "$taker" 2> /dev/null || true
  done
  if in_project full-svc just update-rivium > "$out/update.log" 2>&1; then ok "full-svc: just update-rivium"; else cat "$out/update.log"; fail "full-svc: just update-rivium"; fi
fi

jdk() { # a JDK that runs: macOS has a javac stub that only offers to install one
  javac -version > /dev/null 2>&1 && return 0
  [ "${CI:-}" = true ] && { fail "no JDK"; return 1; }
  echo "skip (no JDK)"
  return 1
}
if jdk; then
  if in_project full-svc just jvm > "$out/jvm.log" 2>&1; then ok "full-svc: just jvm"; else cat "$out/jvm.log"; fail "full-svc: just jvm"; fi
fi

# cargo-zigbuild finds zig on PATH or as the ziglang Python package (as CI installs it).
if has cargo-zigbuild; then
  if in_project plain-svc just package linux_amd64 linux_arm32hf > "$out/package.log" 2>&1; then
    dist=$out/plain-svc/dist/plain-svc
    for platform in linux_amd64 linux_arm32hf; do
      dir=$dist/$platform/0.1.0
      unpacked=$out/unpacked-$platform
      rm -rf "$unpacked" && mkdir -p "$unpacked"
      if (cd "$dir" && { sha256sum -c plain-svc.tar.gz.sha256 > /dev/null 2>&1 || shasum -a 256 -c plain-svc.tar.gz.sha256 > /dev/null; }) \
        && tar -xzf "$dir/plain-svc.tar.gz" -C "$unpacked" \
        && "$root/scripts/check-glibc.sh" 2.17 "$unpacked/plain-svc" > /dev/null; then
        ok "$platform: archive, checksum, glibc 2.17"
      else
        fail "$platform: archive, checksum or glibc"
      fi
    done
    if cmp -s "$out/unpacked-linux_amd64/configs/default.toml" "$out/plain-svc/configs/default.toml"; then ok "linux_amd64 ships configs/default.toml"; else fail "linux_amd64's configuration is not configs/default.toml"; fi
    if grep -q '^restart = "in-process"$' "$out/unpacked-linux_arm32hf/configs/default.toml"; then ok "linux_arm32hf restarts in the process"; else fail "linux_arm32hf's configuration does not restart in the process"; fi
  else
    tail -n 40 "$out/package.log"
    fail "plain-svc: just package"
  fi
fi

# template-diff, against a repository whose tags are this template and a planted change.
repo=$out/template-repo
rm -rf "$repo" && mkdir -p "$repo"
cp -R "$tpl" "$repo/template"
next=$minor.$((${version##*.} + 1))
git -C "$repo" init -q
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm template
git -C "$repo" tag "v$version"
echo "A planted change of the next template version." >> "$repo/template/AGENTS.md"
git -C "$repo" -c user.name=t -c user.email=t@t commit -qam next
git -C "$repo" tag "v$next"
if RIVIUM_TEMPLATE_GIT=$repo in_project plain-svc scripts/template-diff.sh "$next" > "$out/template-diff.log" 2>&1 \
  && grep -q '^+A planted change of the next template version\.$' "$out/template-diff.log" \
  && [ "$(grep -c '^+++ ' "$out/template-diff.log")" -eq 1 ]; then
  ok "template-diff shows the next version's change, and nothing else"
else
  cat "$out/template-diff.log"
  fail "template-diff"
fi

# Planted violations, each in a copy of the plain project, must fail with the expected error.
plant() { # <what> <command that plants it> <command that must fail> <what its output must say>
  local copy=$out/plant
  rm -rf "$copy" && cp -R "$out/plain-svc" "$copy"
  if (cd "$copy" && eval "$2" && eval "$3") > "$out/plant.log" 2>&1; then
    fail "planted $1 was accepted"
  elif grep -q -- "$4" "$out/plant.log"; then
    ok "planted $1 fails"
  else
    tail -n 30 "$out/plant.log"
    fail "planted $1 failed otherwise"
  fi
  rm -rf "$copy"
}
lib=crates/plain-svc/src/lib.rs
plant "detached thread" "printf '/// Planted.\npub fn detached() {\n    let _ = std::thread::spawn(|| ());\n}\n' >> $lib" \
  'cargo clippy --locked -p plain-svc -- -D warnings' 'use of a disallowed method'
plant 'panic = "abort"' "printf '[profile.release]\npanic = \"abort\"\n' >> Cargo.toml" 'scripts/check-panic.sh' 'keep panic = "unwind"'
plant "stale configs/default.toml" "sed -i.bak 's/^every = \"1m\"\$/every = \"2m\"/' configs/default.toml" \
  'cargo test --locked -p plain-svc-bin --test default_config' 'is not the default configuration'
if has cargo-deny; then
  plant "OpenSSL dependency" 'cargo add --quiet -p plain-svc openssl-sys@0.9' 'cargo deny check bans' 'openssl-sys'
fi

[ "$failures" -eq 0 ] || { echo "template-smoke: $failures failure(s)"; exit 1; }
echo "template-smoke: passed"
