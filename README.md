<p align="center">
  <img src="./docs/rivium.png" alt="rivium logo" width="70%" />
</p>

# Rivium

Rivium is a small foundation for long-running Rust services that run either as a standalone
process (under systemd, launchd or a Windows service wrapper) or embedded in an Android app
through JNI. Both forms share one service lifecycle, and the crates also provide an error model,
layered configuration, logging with a disk budget and an axum integration. They are published as
versioned crates, so a fix reaches every service through a dependency update.

> [!NOTE]
>
> **Status: 0.1.0, the first release.** The crates are on crates.io
> ([rivium](https://crates.io/crates/rivium), [rivium-error](https://crates.io/crates/rivium-error),
> [rivium-http](https://crates.io/crates/rivium-http), [rivium-jni](https://crates.io/crates/rivium-jni),
> [rivium-test](https://crates.io/crates/rivium-test)); each
> [release](https://github.com/rivium-rs/rivium/releases) lists its changes, and
> [docs/platforms.md](docs/platforms.md) how far each target is verified. Before 1.0, a new minor
> version may break the API; [docs/upgrading.md](docs/upgrading.md) lists the steps of each one.

## Getting started

A service depends on the Rivium crates it uses, all at one version:

```toml
[dependencies]
rivium = "0.1"
rivium-http = "0.1" # an HTTP API
rivium-jni = "0.1"  # a JNI library for an Android app

[dev-dependencies]
rivium-test = "0.1"
```

Its service library implements `rivium::App`, the composition root that builds the services from
the configuration, and its program's `main` is `rivium::process::run::<App>()`.
[docs/architecture.md](docs/architecture.md) describes how Rivium works, starting with
[a service](docs/architecture.md#a-service). The examples of 0.1.0 are at the tag
[`v0.1.0`](https://github.com/rivium-rs/rivium/tree/v0.1.0/examples).

## Crates

| Crate | Purpose |
| --- | --- |
| `rivium-error` | The error type: kind, class, responsible party, retry hint, context, cause chain and log fields |
| `rivium` | Configuration, logging, the service lifecycle kernel, the process host and the embedded host |
| `rivium-http` | axum integration: the HTTP contract (status envelope, request ids, access log) as a contract layer and its primitives, and an HTTP server service with probes |
| `rivium-jni` | JNI adapter for the embedded host (generated `JNI_OnLoad` and native methods) |
| `rivium-test` | Test support: scripted services, log capture, process tools, lifecycle contract suites (dev-dependency only) |

## Platforms

[docs/platforms.md](docs/platforms.md) lists the supported targets and how far each one was
verified for 0.1.0. The minimum supported Rust version is 1.92.

## Development

CI runs formatting, clippy, cargo-deny, feature checks and the workspace tests on Linux, macOS and Windows, and
the tests on Rust 1.92. The cross-target, hosting and release verification of 0.1.0 is being
rebuilt. Repository checks require tokei 14.0.0 (Rust code-line counting) and cargo-hack 0.6.45
(feature checks). Install these tools under `.tools/` and put `.tools/bin` on `PATH`:

```bash
cargo install --locked --root .tools --version 14.0.0 tokei
cargo install --locked --root .tools --version 0.6.45 cargo-hack
export PATH="$PWD/.tools/bin:$PATH"
```

The same checks locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo hack --each-feature --workspace check --locked
cargo build --package test-app --bins --locked
cargo test --workspace --locked
```

Build the test programs before running the process tests. CI also runs them with
cargo-nextest 0.9.145, without retries:

```bash
cargo nextest run --package tests-integration --profile ci --retries 0 --locked
```

Use the same toolchain, target, Cargo build profile and target directory for the prebuild and
tests. The nextest `--profile ci` option selects test-runner settings, not a Cargo build profile.
Unix signal cases are skipped on Windows; portable exit-code and command-line checks still run.

`cargo test --package tests-build --locked` runs the supply-chain checks separately: complete
normal/build dependency closures, JNI compile guards, package file boundaries and library code
budgets. The budgets include Rust tests inside each library directory, but exclude comments and
blank lines. A missing counter, missing crate or malformed measurement is a failure, not a skip.
The feature check uses normal build targets under resolver 3; it does not remove dev-dependencies
from manifests or regenerate the lockfile.

### JVM contracts

JVM tests need a full JDK via `JAVA_HOME`, with its `bin` directory on `PATH` (CI uses Temurin
21 and compiles the test Java sources for Java 17). A missing JDK is a test failure, not a skip.
Normal library builds and test compilation do not invoke Java or require a JDK.

```bash
export JAVA_HOME=/path/to/jdk-21
export PATH="$JAVA_HOME/bin:$PATH"
cargo test --package rivium-jni --test invocation --locked
```

The Invocation test creates a JVM in an isolated instance of its own Rust test executable;
it does not need a prebuilt cdylib. The parent bounds the run and cleans up after the JVM exits.
It tests the real native bridge, including Unicode, repeated start/stop, errors, flushed logs,
and a negative stop timeout. JNI checker warnings fail the test.

The separate loader contract uses the private `test-jni` cdylib and requires both normal and
fault builds. Keep their output directories separate and do not export the fault flags globally:

```bash
cargo build --package test-jni --lib --target-dir target/jvm-normal --locked
RUSTFLAGS="--cfg rivium_jni_fault" cargo build --package test-jni --lib --target-dir target/jvm-fault --locked
crates/rivium-jni/tests/jvm/run.sh target/jvm-normal/debug test_jni rivium/tests target/jvm-fault/debug
```

This checks real loading, mismatched signatures and panic containment in all five methods.
The normal library must ignore fault-injection settings. CI bounds the entire loader run with
a timeout; neither missing fault artifacts nor checker warnings count as success.

### Linux glibc baseline

The x86_64 GNU/Linux baseline is glibc 2.17. Install cargo-zigbuild 0.23.4 and Zig 0.15.2
in an isolated environment, plus GNU `readelf` and `jq`; add the Rust
`x86_64-unknown-linux-gnu` target. For example:

```bash
python3 -m venv .tools/glibc
.tools/glibc/bin/pip install cargo-zigbuild==0.23.4 ziglang==0.15.2
export PATH="$PWD/.tools/glibc/bin:$PATH"
rustup target add x86_64-unknown-linux-gnu
scripts/glibc.sh build
```

This links the real application binaries, workspace tests and JNI cdylib. It checks the ELF
architecture, dynamic loader and numeric GLIBC symbol versions of artifacts reported by those
Cargo builds, not unrelated files left in `target`. Missing tools, empty results and malformed
ELFs fail the check. The inventories are under `target/glibc`; a failed build is not runtime
verification. Cargo-zigbuild's `--tests` compiles without running; it does not take `--no-run`.

On a **native Linux x86_64 host with Docker running**, follow the build with:

```bash
scripts/glibc.sh run
```

This rechecks the artifacts and runs the library and process tests in a digest-pinned cross-rs
CentOS userland, first requiring `getconf` to report glibc 2.17. The checkout is mounted read-only
at the same absolute path used at compile time. Tests have time limits and must not be skipped
or retried. Invocation needs a JDK and supply-chain tests need Cargo/tokei, so those two suites
stay in their existing required native CI jobs rather than being counted as old-userland tests.
Linking and symbol inspection alone do not establish runtime compatibility.

PR CI calls the `glibc` workflow for linking/symbol inspection only. Manually dispatch `glibc`
with `run-runtime` enabled for the old-userland run; the workflow is also reusable by future
platform jobs. The `ci-pass` aggregate requires **every** CI prerequisite to succeed, including
all native matrix legs and `ci/zigbuild`; failure, cancellation or skipping fails the aggregate.
Maintainers can select `ci-pass` for branch protection; adding the job does not change repository
settings automatically. Guard self-tests run in `ci/lint`:

```bash
scripts/tests/check-glibc.sh
scripts/tests/check-ci.sh
```

### Extended-platform checks

The `daily` workflow declares a UTC 02:00 schedule, manual dispatch and a reusable entry point.
It is separate from pull-request CI. Every required lane must succeed; `daily-pass` rejects
failed, cancelled or skipped jobs, including failures on the experimental upstream LoongArch runner.

| Lane | What it checks |
| --- | --- |
| ARMv7 GNU/Linux + QEMU 11.0 | Library tests and actual target child-process contracts |
| LoongArch64 GNU/Linux + QEMU 11.0 | The same contracts; no allowed-failure exception |
| Windows GNU + Wine 10.0 | Library tests and target child-process contracts |
| Android arm64-v8a, API 30 | NDK compilation/linking, real JNI export and 16 KB ELF LOAD alignment |
| CentOS glibc 2.17 | Reuses the glibc workflow with its runtime check enabled |

On a Linux x86_64 build host, after configuring the matching linker/sysroot and
`CARGO_TARGET_<TRIPLE>_RUNNER` (the workflow uses a SHA-pinned cross-toolchain action):

```bash
scripts/test-platform.sh armv7-unknown-linux-gnueabihf
scripts/test-platform.sh loongarch64-unknown-linux-gnu
scripts/test-platform.sh x86_64-pc-windows-gnu
```

The script owns `RUSTFLAGS`: leave it and `CARGO_ENCODED_RUSTFLAGS` unset. It prebuilds the
application/JNI targets before tests, bounds each command, runs tests serially without retries,
and rejects empty/filtered results or missing child-process contracts. Only the four signal
scenarios and the signal-driven stop-deadline test are ignored under QEMU, with their names and
counts checked; native CI still runs all five. Wine permits no ignored tests. JVM Invocation and
Cargo/tokei-dependent build guards stay in native CI, not in these emulator runs.

For the Android compile-only check, install NDK **28.1.13356709**, GNU `readelf`, and:

```bash
cargo install cargo-ndk --version 4.1.2 --locked --root .tools
export PATH="$PWD/.tools/bin:$PATH"
export ANDROID_NDK_HOME=/path/to/android-ndk-r28b
rustup target add aarch64-linux-android
scripts/build-android.sh
```

The output is `target/android/aarch64-linux-android/debug/libtest_jni.so`. The checker requires
an AArch64 shared object linked to bionic, an API 30 note, a defined `JNI_OnLoad`, and valid
16 KB-or-greater LOAD alignments with congruent file offsets/virtual addresses. This is **not**
an ART/device test or a bionic runtime test. Driver and malformed-ELF self-tests run in `ci/lint`;
fixtures do not count as QEMU, Wine or device execution.

Hosting remains a separate acceptance boundary: `host-notify` currently runs the ordinary host,
not an sd-notify implementation. The daily workflow does not claim notification-protocol or real
systemd/launchd/WinSW service-manager verification, and has no disabled placeholder job reporting
those as passed.

Maintainers install the git hooks that run the public-content check once:

```bash
git config core.hooksPath scripts/hooks
git config rivium.denylist <absolute path of the private denylist>
```

Branches, commit messages (Conventional Commits) and pull requests follow
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
