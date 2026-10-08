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
> **Status: under construction.** Nothing is published yet. Implemented so far: the error type,
> configuration, logging with a disk budget, the lifecycle kernel, the process and embedded
> hosts, the JNI adapter and the test support. The HTTP integration and log export come next.

## Crates

| Crate | Purpose |
| --- | --- |
| `rivium-error` | The error type: kind, class, responsible party, retry hint, context, cause chain and log fields |
| `rivium` | Configuration, logging, the service lifecycle kernel, the process host and the embedded host |
| `rivium-http` | axum integration: HTTP server service, status envelope, request ids, access log, probes |
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
| `just loc`, `just deps` | Size budget and dependency closure of the library crates |

Developer tools are installed under `.tools/` (`just tools`), never globally. Maintainers install
the git hooks that run the public-content check with `just hooks <denylist>`. Branches, commit
messages (Conventional Commits) and pull requests follow [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
