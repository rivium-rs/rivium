<p align="center">
  <img src="./docs/rivium.png" alt="rivium logo" width="70%" />
</p>

# Rivium

A small foundation for long-running Rust services, whether they run as standalone processes
or inside an Android app through JNI.

[![Crates.io](https://img.shields.io/crates/v/rivium.svg)](https://crates.io/crates/rivium)
[![Documentation](https://docs.rs/rivium/badge.svg)](https://docs.rs/rivium)

[API documentation](https://docs.rs/rivium) |
[Architecture](docs/architecture.md) |
[Platform support](docs/platforms.md) |
[Contributing](docs/contributing/README.md)

## Overview

Rivium handles the common parts of a service so applications can focus on their own work:

- **One lifecycle, two hosts.** The same application handles startup, readiness, graceful
  shutdown and restarts as a process or through an embedded host.
- **Configuration and logging.** Layered configuration with source tracking, and logging to
  the console, budgeted files and Android logcat.
- **Optional integrations.** Add HTTP with axum, a JNI bridge or test helpers through separate
  crates. Services without those integrations do not depend on them.

Rivium is built on Tokio; it is a service foundation, not a replacement async runtime.
See the [architecture guide](docs/architecture.md) for the lifecycle and crate boundaries.

## Getting started

Add the core crate to your service:

```toml
[dependencies]
rivium = "0.1"
```

Implement [`rivium::App`](https://docs.rs/rivium/latest/rivium/trait.App.html) with your
configuration type and services, then call `rivium::process::run::<YourApp>()` from `main`.
The [service guide](docs/architecture.md#a-service) explains the application model; the
[App API documentation](https://docs.rs/rivium/latest/rivium/trait.App.html) includes a complete
example. The same application can be exported as a JNI library with `rivium_jni::export!`.

Add only the adapters you need, keeping the Rivium crates on the same version:

```toml
[dependencies]
rivium = "0.1"
rivium-http = "0.1" # optional: an HTTP API
rivium-jni = "0.1"  # optional: a JNI library for an Android app

[dev-dependencies]
rivium-test = "0.1"
```

The examples shipped with 0.1.0 are available at the
[`v0.1.0` tag](https://github.com/rivium-rs/rivium/tree/v0.1.0/examples).
Before 1.0, a new minor version may break the API. See the
[release notes](https://github.com/rivium-rs/rivium/releases) for changes and the
[upgrade guide](docs/upgrading.md) for migration steps.

## Crates

| Crate | Purpose |
| --- | --- |
| [`rivium`](https://docs.rs/rivium) | Configuration, logging, lifecycle, process and embedded hosts |
| [`rivium-error`](https://docs.rs/rivium-error) | Structured errors with classification, context, causes and retry hints |
| [`rivium-http`](https://docs.rs/rivium-http) | axum integration, HTTP contracts, a server service and health probes |
| [`rivium-jni`](https://docs.rs/rivium-jni) | JNI bridge for the embedded host, with generated native entry points |
| [`rivium-test`](https://docs.rs/rivium-test) | Lifecycle, logging and process test helpers; a dev-dependency only |

## Supported Rust versions

The minimum supported Rust version (MSRV) is **1.92**, checked separately in CI.

## Platforms

Rivium targets GNU/Linux, macOS and Windows processes, and Android apps through JNI.
Verification differs by target and release: native tests, emulator tests and device tests are
not interchangeable. The current Android lane checks compilation and JNI ELF properties,
not execution on a device.

See [platform support](docs/platforms.md) for the current verification scope, known gaps and
the separate [0.1.0 verification record](docs/platforms.md#010).

## Getting help

Start with the [API documentation](https://docs.rs/rivium) and
[architecture guide](docs/architecture.md). If something is unclear or does not work,
[open an issue](https://github.com/rivium-rs/rivium/issues) with the Rivium version, target
platform and a small reproduction. Suggestions and documentation improvements are welcome too.

## Development

See [Contributing](docs/contributing/README.md) for the pull request workflow and
[development checks](docs/development.md) for local setup, native tests, JVM contracts and
cross-target verification. Maintainers can find dry-run and publishing instructions in the
[release guide](docs/releasing.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
