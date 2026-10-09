# Upgrading

How a service built on Rivium takes a new release. Every Rivium crate (`rivium-error`,
`rivium`, `rivium-http`, `rivium-jni`, `rivium-test`) is released at the same version as the
others, and so is the template. The release notes of each version are on its
[GitHub release](https://github.com/rivium-rs/rivium/releases).

## A patch release: `cargo update`

A patch release (0.1.0 to 0.1.1) fixes without changing the API, the configuration or the exit
codes. Take it for every Rivium crate at once, so that they stay at one version:

```bash
cargo update -p rivium-error -p rivium -p rivium-test
```

(add `-p rivium-http` and `-p rivium-jni` if the service uses them; a project generated from the
template has `just update-rivium`). Only `Cargo.lock` changes. Then build for every platform,
run the tests, including the lifecycle contract tests, and deploy as usual.

## A breaking release: the steps below

Before 1.0, a new minor version (0.1 to 0.2) may change the API, configuration keys or their
meaning. Each such version has a section here, written as steps that a person or a coding agent
can follow in order. Every step says what to change (a file or a symbol), how, and how to check
it. To cross several versions, follow their sections from the oldest. Every section starts and
ends the same way:

1. In the workspace `Cargo.toml`, set every Rivium crate to the new version, then run `cargo
   update -p rivium-error -p rivium` (and the other Rivium crates the service uses).
2. The steps of the version.
3. Regenerate `configs/default.toml` (`just default-config`) and review its diff: it shows every
   default that changed.
4. `just template-diff <version>` shows what changed in the files that the template generates;
   merge what applies by hand.
5. `just check`: formatting, clippy, cargo-deny, the tests and the lifecycle contract.

## An urgent fix

When a service cannot wait for a release, point its Rivium crates at a branch with the fix:

```toml
[patch.crates-io]
rivium = { git = "https://github.com/rivium-rs/rivium", branch = "<the fix's branch>" }
```

(one line for each Rivium crate the service uses), and remove the patch once the release that
contains the fix is out.

## 0.1.0

The first release: nothing to upgrade from. A new service starts from the template:

```bash
cargo generate --git https://github.com/rivium-rs/rivium --tag v0.1.0 template
```
