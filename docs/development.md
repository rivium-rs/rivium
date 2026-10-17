# Development checks

Use Rust 1.92 or newer. Repository tools belong in `.tools/bin`; CI pins tokei 14.0.0,
cargo-hack, cargo-deny and other tools in its workflows. A complete JDK 17+ is required
for Java loading contracts; set JAVA_HOME to that JDK rather than a JRE.

## Native and feature contracts

```sh
export PATH="$PWD/.tools/bin:$PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo build -p rivium-examples --examples --locked
cargo build -p tests-integration --bin test-host --locked
cargo test --workspace --all-features --locked
cargo hack --each-feature --workspace check --locked
cargo test -p rivium-util --no-default-features --features task,log --locked
cargo +1.92 check --workspace --all-targets --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
cargo deny --locked check
```

Also run package contracts in the independently selected core none/task/rt, utility
none/config/log/log-export/task, host process/embedded, and HTTP none/contract/server/fs
builds. All-features success does not prove a minimal product closure. Build tests use
real workspace-external consumers without development dependencies, verify six public
packages, and reject missing or malformed LOC measurements. Source layout and the
aggregate LOC ceiling are described below.

Private packages are tests-build, tests-integration, test-app, test-jni, rivium-examples
and rivium-benches. Fault applications belong to tests-integration/test-app; examples
remain ordinary usage demonstrations. Process tests require the sibling test-host binary
for the same profile, target and target directory, with target runners propagated.

## Source layout and LOC budget

Separate imports from definitions, independent items from each other, and methods and
tests with blank lines. Keep attributes and documentation attached to their declarations;
keep related associated types and constants together. Use blank lines to mark meaningful
steps in longer functions. Run rustfmt and also inspect macro bodies and item spacing:
passing the formatter alone does not establish readable layout.

The aggregate ceiling is **18,150 Rust code lines**, measured with tokei 14.0.0 across all
six library directories, including their tests and fixtures. Keep the counter version,
file scope and measurement checks unchanged; do not compress code, remove error handling
or assertions, or move tests outside that scope to satisfy the ceiling.

The 2026-10-16 review replaces the inherited 17,710-line ceiling with 18,000, an increase
of 290 lines (1.64%). After restoring readable layout, the fixed counter reports 17,828
lines, leaving 172 lines (about 1%) for small maintenance changes. This is an explicit
maintenance allowance, not an estimate of capacity for new features. Further increases
require a review of implementation and validation costs. The ceiling is not a target or
evidence of correctness, performance, or release readiness.

The 2026-10-17 performance repair review adds 150 lines (0.83%) of maintenance
allowance, bringing the ceiling to 18,150. The final implementation uses 18,132 lines,
including the planner split, owned sorted-Vec lookup and regression coverage for retry
deadlines crossing awaited stops, unordered input, separated duplicate keys and Send
specifications that are not Sync. This allowance preserves readable control flow and
behavioral assertions; it does not change the counting scope or the P-CORE/P-TASK
performance thresholds.

## Models and sustained workloads

The independent tests-loom workspace compiles the new production scope implementation
under a model cfg. Its synchronization/token substitutes do not prove Tokio's scheduling,
Notify implementation, arbitrary destructors or operating-system behavior.

```sh
env -u LOOM_MAX_PREEMPTIONS -u LOOM_MAX_PERMUTATIONS -u LOOM_MAX_DURATION \
  RUSTFLAGS='--cfg rivium_scope_model' CARGO_TARGET_DIR=target/loom \
  cargo test --manifest-path tests-loom/Cargo.toml --locked
cargo fmt --manifest-path tests-loom/Cargo.toml -- --check
RIVIUM_STRESS_SECONDS=600 RIVIUM_STRESS_SEED=6633769275985317889 \
  cargo test --release -p rivium-util --features task --locked --test task_stress \
  concurrent_lifecycle_stress -- --ignored --exact --nocapture
RIVIUM_STRESS_SECONDS=180 \
  cargo test -p rivium-host --all-features --locked --test scope \
  surviving_descendants_restart_stress -- --ignored --exact --nocapture
```

Use a real external watchdog for unattended stress. An async timeout on a blocked executor
cannot terminate non-yielding work. The seed reproduces workload choices, not OS scheduling.
Report the actual elapsed time and iteration counts. Ignored sustained tests are not passed
by an ordinary smoke run. Keep model cfg and build output separate from ordinary Cargo jobs.

## Benchmarks

```sh
cargo bench -p rivium-benches --bench task_lifecycle --locked
cargo bench -p rivium-benches --bench task_latency --locked
cargo bench -p rivium-benches --bench log_directory --locked
```

Run at least three samples on an otherwise idle machine after builds/stress complete.
Record source identity, toolchain, CPU/OS, runtime flavor, workload and dirty state. Existing
lifecycle and directory harness outputs are distributions of batch means, not request p95/p99.
The task latency harness records single-round replacement and mixed recovery-episode
percentiles separately; see the [benchmark guide](../benches/README.md) for boundaries.
The directory benchmark
includes explicit flush cost and filesystem/cache noise; it does not establish device throughput.
Use the same workload and separate target directories for old/new revision comparisons.

## JVM and platform boundaries

```sh
cargo build -p test-jni --lib --target-dir target/jvm-normal --locked
RUSTFLAGS='--cfg rivium_jni_fault' cargo build -p test-jni --lib \
  --target-dir target/jvm-fault --locked
scripts/test-jvm.sh target/jvm-normal/debug target/jvm-fault/debug
```

Build test-jni alone so workspace feature unification does not silently add process/HTTP
capabilities to the embedded product. The Java launcher uses -Xcheck:jni and rejects native
warnings. Normal and fault products use separate target directories. Tests cover true dynamic
loading, signature mismatch, Unicode, null/invalid input, repeated lifecycle calls and all five
panic boundaries. Java error summaries intentionally omit raw configuration diagnostics.

- `scripts/build-android.sh`: NDK 28.1.13356709, cargo-ndk 4.1.2, arm64-v8a/API30, 16 KB ELF alignment.
- `scripts/test-platform.sh <target>`: target linkers and QEMU/Wine for the selected platform.
- `scripts/glibc.sh build`: cargo-zigbuild 0.23.4, Zig 0.15.2, readelf and jq for glibc 2.17 artifacts.
- `scripts/glibc.sh run`: native x86_64 Linux with Docker and the pinned old-userland image.

Compilation is not execution. Desktop JVM is not Android ART, Wine/QEMU is not hardware,
and ordinary processes do not verify systemd/launchd/WinSW or sd-notify. Keep omissions visible
in [platform records](platforms.md), especially when tools are unavailable locally.

## Verification tooling and publication

```sh
shellcheck scripts/*.sh scripts/hooks/* scripts/tests/*.sh
for check in scripts/tests/*.sh; do "$check" || exit 1; done
```

Each guard needs a valid case and a planted violation. A skipped job, empty test list, malformed
counter output or missing secret must not report success. Public-content checks use the private
configured denylist and report locations without revealing its contents. Release previews and
publication are separate operations; see [releasing](releasing.md). The release aggregate must
require the dedicated Loom result as well as native, daily, package and content checks.
