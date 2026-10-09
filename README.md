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
> **Status: 0.1.0 is being prepared.** Nothing is published yet: the crates, the template and
> the release process are complete and checked in CI, and the first release follows once the
> maintainer confirms it. Before 1.0, a new minor version may break the API;
> [docs/upgrading.md](docs/upgrading.md) lists the steps of each one.

## Getting started

A new service starts from the template, which generates a service library with its composition
root, a program, optionally an HTTP API and a JNI library for Android, the configuration, the
checks and the packaging for each platform:

```bash
cargo generate --git https://github.com/rivium-rs/rivium --tag v0.1.0 template
```

(before the first release: `--branch main` in place of `--tag v0.1.0`). The generated AGENTS.md
describes the rules for services; [docs/architecture.md](docs/architecture.md) describes how
Rivium works. The examples show the crates at work:

| Example | What it shows |
| --- | --- |
| [`udp-echo`](examples/udp-echo) | A service that is not HTTP: a blocking socket loop, a periodic job, restart requests, log export without HTTP |
| [`http-api`](examples/http-api) | An HTTP API with the status envelope, the probes and an HTTP binding of log export |
| [`embedded`](examples/embedded) | One service library run as a program (`beacon-bin`) and inside an Android app (`beacon-jni`) |
| [`multi-bin`](examples/multi-bin) | One service library and two programs, each its own composition root |
| [`slices`](examples/slices) | Validation slices written as services are: `snmp-lite`, an SNMP agent run as a program and inside an Android app; `edge-lite`, an edge collector with supervised units, an HTTP API and a configuration that it replaces |

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
| `just template`, `just drills`, `just slices` | The template's projects checked as their CI would; the scenario A and C drills; the validation slices as `ci/slices` checks them |
| `just loc`, `just deps` | Size budget and dependency closure of the library crates |

Developer tools are installed under `.tools/` (`just tools`), never globally. Maintainers install
the git hooks that run the public-content check with `just hooks <denylist>`. Branches, commit
messages (Conventional Commits) and pull requests follow [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or conditions.
