# Upgrading

How a service built on Rivium takes a new release. Every Rivium crate (`rivium-error`,
`rivium`, `rivium-http`, `rivium-jni`, `rivium-test`) is released at the same version as the
others. The release notes of each version are on its
[GitHub release](https://github.com/rivium-rs/rivium/releases).

## A patch release: `cargo update`

A patch release (0.1.0 to 0.1.1) fixes without changing the API, the configuration or the exit
codes. Take it for every Rivium crate at once, so that they stay at one version:

```bash
cargo update -p rivium-error -p rivium -p rivium-test
```

(add `-p rivium-http` and `-p rivium-jni` if the service uses them). Only `Cargo.lock` changes.
Then build for every platform, run the tests, including the lifecycle contract tests, and deploy
as usual.

## A breaking release: the steps below

Before 1.0, a new minor version (0.1 to 0.2) may change the API, configuration keys or their
meaning. Each such version has a section here, written as steps that a person or a coding agent
can follow in order. Every step says what to change (a file or a symbol), how, and how to check
it. To cross several versions, follow their sections from the oldest. Every section starts and
ends the same way:

1. In the workspace `Cargo.toml`, set every Rivium crate to the new version, then run `cargo
   update -p rivium-error -p rivium` (and the other Rivium crates the service uses).
2. The steps of the version.
3. If the service keeps its default configuration in a file, regenerate it with the program's
   `default-config` and review its diff: it shows every default that changed.
4. Build for every platform and run the service's checks and tests, including the lifecycle
   contract tests.

## An urgent fix

When a service cannot wait for a release, point its Rivium crates at a branch with the fix:

```toml
[patch.crates-io]
rivium = { git = "https://github.com/rivium-rs/rivium", branch = "<the fix's branch>" }
```

(one line for each Rivium crate the service uses), and remove the patch once the release that
contains the fix is out.

## 0.1.1

This is a compatible maintenance update of the five library crates. Their production Rust
implementation, public APIs, configuration defaults, exit codes and Rust 1.92 MSRV are unchanged
from 0.1.0. Existing services do not need source or configuration migration. Keep all Rivium
crates on the same version and use the [patch-update commands](#a-patch-release-cargo-update)
above; dependency requirements such as `rivium = "0.1"` already allow 0.1.1.

The repository workflow changed even though the library contract did not:

- **The project template was removed from `main`.** A `cargo generate` invocation that uses this
  repository's default branch as its template source will no longer work. For the old scaffold,
  explicitly select the [`v0.1.0` tag](https://github.com/rivium-rs/rivium/tree/v0.1.0/template),
  or start a normal Cargo application using the [App example](https://docs.rs/rivium/latest/rivium/trait.App.html).
  The historical scaffold command is:

  ```bash
  cargo generate --git https://github.com/rivium-rs/rivium --tag v0.1.0 template
  ```

  Already-generated projects keep their own source; they do not need to be regenerated to
  update their Rivium dependencies. The generated release notes retain this repository-level
  breaking change; it is not a breaking change to the published library APIs.
- **The old repository examples and validation fixtures were removed from `main`.**
  The released examples remain at [`v0.1.0/examples`](https://github.com/rivium-rs/rivium/tree/v0.1.0/examples).
  Repository contributors should follow [development checks](development.md), not old paths
  under `examples/` or `fixtures/`. The new `test-app`, `test-jni`, `tests-integration` and
  `tests-build` packages are private validation tools, not new application dependencies.
- **Verification claims are version-specific.** Consult the [0.1.1 record](platforms.md#011)
  for the checked targets and omissions. A restored CI lane does not establish Android device,
  service-manager or hardware verification.

Build and test the updated service on its deployment targets before rolling it out. Retain
explicitly unverified platform claims; the 0.1.0 hardware/hosting record is not automatically
renewed by a patch version bump.

## 0.1.0

The first release: nothing to upgrade from.
