#!/usr/bin/env bash
# Self-test for scripts/loc.sh: test modules are excluded (inline, out-of-line, nested files),
# other #[cfg(test)] items are counted, and --check fails on a planted budget overrun.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/loc-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
failures=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got $2, want $3)"; failures=$((failures + 1)); fi; }
count_of() { awk -v c="$1" '$1 == c { print $2 }' "$tmp/out"; }

src="$tmp/ws/crates/rivium-error/src"
mkdir -p "$src/foo/tests" "$tmp/ws/crates/rivium-error/tests"
cat > "$src/lib.rs" <<'EOF'
//! Crate docs are comments.
mod foo;

/// Counted: 3 code lines.
pub fn one() -> u32 {
    1
}

#[cfg(test)]
fn helper_is_counted() {}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[allow(clippy::all)]
mod inline {
    #[test]
    fn t() {
        let s = "}";
        assert_eq!(s, "}");
    }
}
EOF
cat > "$src/tests.rs" <<'EOF'
fn excluded_out_of_line() {}
EOF
cat > "$src/foo.rs" <<'EOF'
pub(crate) fn two() {}

#[cfg(test)]
mod tests;
EOF
cat > "$src/foo/tests.rs" <<'EOF'
mod nested;
fn excluded_foo_tests() {}
EOF
echo 'fn excluded_nested() {}' > "$src/foo/tests/nested.rs"
echo 'fn integration_tests_are_excluded() {}' > "$tmp/ws/crates/rivium-error/tests/it.rs"

"$root/scripts/loc.sh" --check "$tmp/ws" > "$tmp/out"
# lib.rs: `mod foo;` (1) + one() (3) + `#[cfg(test)]` helper with its attribute (2); foo.rs: two() (1).
check "test modules excluded, other items counted" "$(count_of rivium-error)" 7
check "missing crates count as 0" "$(count_of rivium)" 0

for i in $(seq 1 500); do echo "pub fn f$i() {}"; done >> "$src/foo.rs"
status=0
"$root/scripts/loc.sh" --check "$tmp/ws" > "$tmp/out" 2>&1 || status=$?
check "--check fails on a planted overrun" "$status" 1
marked=no
if grep -q '^rivium-error .* OVER$' "$tmp/out"; then marked=yes; fi
check "overrun is marked" "$marked" yes

if [ "$failures" -ne 0 ]; then
  echo "loc self-test: $failures failure(s)"
  exit 1
fi
echo "loc self-test: all passed"
