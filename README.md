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

CI runs formatting, clippy, cargo-deny and the workspace tests on Linux, macOS and Windows, and
the tests on Rust 1.92. The cross-target, hosting and release verification of 0.1.0 is being
rebuilt. The same checks locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

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
