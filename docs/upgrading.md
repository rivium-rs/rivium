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

## 0.1.0

The first release: nothing to upgrade from.
