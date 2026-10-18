<p align="center">
  <img src="./docs/rivium.png" alt="Rivium logo" width="60%" />
</p>

<h1 align="center">Rivium</h1>

<p align="center">
  <strong>Rust service foundation for long-running applications</strong><br />
  One lifecycle across standalone processes and Android JNI hosts.
</p>

<p align="center">
  <strong>English</strong> | <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://crates.io/crates/rivium"><img src="https://img.shields.io/crates/v/rivium.svg" alt="Crates.io" /></a>
  <a href="https://docs.rs/rivium"><img src="https://docs.rs/rivium/badge.svg" alt="Documentation" /></a>
  <a href="https://github.com/rivium-rs/rivium/actions/workflows/ci.yml"><img src="https://github.com/rivium-rs/rivium/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/MSRV-1.92%2B-orange.svg" alt="MSRV 1.92+" />
  <img src="https://img.shields.io/crates/l/rivium.svg" alt="License" />
</p>

<p align="center">
  <a href="https://docs.rs/rivium">Published API documentation</a> |
  <a href="docs/architecture.md">Architecture</a> |
  <a href="docs/platforms.md">Platform support</a> |
  <a href="docs/contributing/README.md">Contributing</a>
</p>

## Overview

Rivium handles the common parts of a service so applications can focus on their own work:

- **One lifecycle, two hosts.** The same application handles startup, readiness, graceful
  shutdown and restarts as a process or through an embedded host.

- **Configuration and logging.** Layered configuration with source tracking, validated reloads,
  and logging to the console, budgeted files and Android logcat. Applications choose which
  settings can change while running.

- **Explicit resource ownership.** Task scopes, service supervision and host reports distinguish
  a stop request from completed cleanup. Tasks, HTTP admission and file-log queues have finite
  limits; a restart waits for the previous generation to release its resources.

- **Optional integrations.** Add HTTP with axum, a JNI bridge or test helpers through separate
  crates. Services without those integrations do not depend on them.

Rivium is built on Tokio; it is a service foundation, not a replacement async runtime.
See the [architecture guide](docs/architecture.md) for the lifecycle and crate boundaries.

## Getting started

**The 0.2.0 workspace is released.** Use Rust 1.92 or newer and run the basic
service directly from this checkout:

```sh
cargo run -p rivium-examples --example basic-service -- --root target/basic-example
```

The service reports readiness and waits for Ctrl-C to shut down. For a standalone application
using this source, add the core crate and the process host. In these examples, `../rivium` is
the path from your application to this checkout; adjust it to match your directory layout:

```toml
[dependencies]
rivium = { path = "../rivium/crates/rivium", features = ["rt"] }
rivium-host = { path = "../rivium/crates/rivium-host", features = ["process"] }
serde = { version = "1", features = ["derive"] }
```

Implement `rivium_host::App` with your configuration type and services, then call
`rivium_host::process::run::<YourApp>()` from `main`. Services declare readiness after
initialization, observe their stop signal and release their resources before returning.

The [host guide](docs/architecture.md#hosts) explains the application model; the
[basic service example](examples/basic-service.rs) includes a complete implementation.

The same application can be exported as a JNI library with `rivium_jni::export!`.

Add only the integrations you need, using the same checkout for all Rivium crates:

```toml
[dependencies]
rivium-http = { path = "../rivium/crates/rivium-http", features = ["server"] } # HTTP API
rivium-jni = { path = "../rivium/crates/rivium-jni" } # Android JNI library

[dev-dependencies]
rivium-test = { path = "../rivium/crates/rivium-test" }
```

See the [example guide](examples/README.md) for runnable services and JNI integration.

This README describes the **0.2 workspace**, whose Rust APIs and crate layout differ from 0.1.
Before 1.0, a new minor version may break the API. See the
[release notes](https://github.com/rivium-rs/rivium/releases) for released versions and the
[upgrade guide](docs/upgrading.md) for migration steps.

The badges and API links above refer
to published releases; the examples and guides in this checkout follow its source.

## Crates

| Crate | Purpose |
| --- | --- |
| [`rivium`](crates/rivium/README.md) | Structured errors; owned task scopes with `task`; service supervision, readiness and shutdown reports with `rt` |
| [`rivium-util`](crates/rivium-util/README.md) | Independent configuration, logging, log export and persistent-task reconciliation, plus atomic file replacement |
| [`rivium-host`](crates/rivium-host/README.md) | Application composition, configuration reload, runtime ownership, process and embedded hosts |
| [`rivium-http`](crates/rivium-http/README.md) | HTTP response vocabulary, optional bounded request contracts, supervised listeners and file downloads |
| [`rivium-jni`](crates/rivium-jni/README.md) | JNI bridge for the embedded host, with generated native entry points |
| [`rivium-test`](crates/rivium-test/README.md) | Temporary directories, captured logs and lifecycle helpers; a dev-dependency only |

The core, utility, host and HTTP crates have no default features. The core error API needs no
external dependencies; the host always includes configuration, logging and runtime dependencies,
and HTTP always includes axum. See each crate's documentation for the capabilities to enable.

## Supported Rust versions

The minimum supported Rust version (MSRV) is **1.92**, checked separately in CI.

## Platforms

Rivium targets GNU/Linux, macOS and Windows processes, and Android apps through JNI.

Verification differs by target and release: native tests, emulator tests and device tests are
not interchangeable. The current Android lane checks compilation and JNI ELF properties,
not execution on a device. Historical platform results do not establish support for this rewrite.
The 0.2.0 scope and task-management regressions have been reduced; remaining
performance acceptance work is still open.

See [platform support](docs/platforms.md) for the current verification scope, known gaps and
[version-specific verification records](docs/platforms.md#release-records).

## Getting help

Start with the [Published API documentation](https://docs.rs/rivium) and
[architecture guide](docs/architecture.md).

If something is unclear or does not work,
[open an issue](https://github.com/rivium-rs/rivium/issues) with the Rivium version, target
platform and a small reproduction. Suggestions and documentation improvements are welcome too.

## Development

See [Contributing](docs/contributing/README.md) for the pull request workflow and
[development checks](docs/development.md) for local setup, native tests, JVM contracts and
cross-target verification.

Maintainers can find dry-run and publishing instructions in the
[release guide](docs/releasing.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
