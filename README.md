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
[a service](docs/architecture.md#a-service). The examples show the crates at work:

| Example | What it shows |
| --- | --- |
| [`udp-echo`](examples/udp-echo) | A service that is not HTTP: a blocking socket loop, a periodic job, restart requests, log export without HTTP |
| [`http-api`](examples/http-api) | An HTTP API with the status envelope, the probes and an HTTP binding of log export |
| [`embedded`](examples/embedded) | One service library run as a program (`beacon-bin`) and inside an Android app (`beacon-jni`) |
| [`multi-bin`](examples/multi-bin) | One service library and two programs, each its own composition root |

## Crates

| Crate | Purpose |
| --- | --- |
| `rivium-error` | The error type: kind, class, responsible party, retry hint, context, cause chain and log fields |
| `rivium` | Configuration, logging, the service lifecycle kernel, the process host and the embedded host |
| `rivium-http` | axum integration: the HTTP contract (status envelope, request ids, access log) as a contract layer and its primitives, and an HTTP server service with probes |
| `rivium-jni` | JNI adapter for the embedded host (generated `JNI_OnLoad` and native methods) |
| `rivium-test` | Test support: scripted services, log capture, process tools, lifecycle contract suites (dev-dependency only) |

## Platforms

[docs/platforms.md](docs/platforms.md) lists the supported targets, how far each one is verified
and the CI job that verifies it. The minimum supported Rust version is 1.92.

## Development

`just` lists the tasks. The main ones:

| Task | What it does |
| --- | --- |
| `just check` | Formatting, clippy, cargo-deny, the feature matrix, script self-tests and the workspace tests |
| `just msrv` | The tests with Rust 1.92, installed under `.tools/` |
| `just cross`, `just android`, `just glibc`, `just jvm`, `just systemd` | Local runs of the cross-target, glibc-baseline, JVM and systemd CI jobs with Docker |
| `just drills` | The scenario A and C drills |
| `just loc`, `just deps` | Size budget and dependency closure of the library crates |

Developer tools are installed under `.tools/` (`just tools`), never globally. Maintainers install
the git hooks that run the public-content check with `just hooks <denylist>`. Branches, commit
messages (Conventional Commits) and pull requests follow [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
