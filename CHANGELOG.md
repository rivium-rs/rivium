# Changelog

Every release of Rivium, newest first. The crates, `rivium-error`, `rivium`, `rivium-http`,
`rivium-jni` and `rivium-test`, are released together at one version, and so is the template.
Each release has five parts:

- **Added**, **Changed**, **Fixed**: what the release brings.
- **Breaking**: what a consumer has to change. Before 1.0, a new minor version is a breaking
  release, and [docs/upgrading.md](docs/upgrading.md) lists its steps; a patch release is never
  breaking, so `cargo update` takes it.
- **Template**: changes to the files that the template generates, which a project merges by
  hand: `just template-diff <version>` shows them.

## [0.1.0] - unreleased

The first release.

### Added

- `rivium-error`: one error type for services: a kind (a stable name and a class), the party
  responsible, a retry hint, context and a cause chain; log fields and `log_error!`.
- `rivium`:
  - the service model: `App` (the composition root) and `Service`, frontline and background,
    with sub-tasks, `periodic` jobs, readiness and health, a stop deadline and restart requests;
  - the process host, `process::run`: arguments, configuration, logging, SIGTERM and SIGINT (and
    Windows console events), exit codes, restarts by the supervisor or in the process;
  - the embedded host, `embedded::Host`: start, stop and status for a service inside another
    program, such as an Android app;
  - configuration: defaults, a file, environment variables and `--set`, with `Option` fields,
    maps, the source of every value, and every problem reported at once; `check-config` and
    `default-config`;
  - logging: the console, files by category and, on Android, logcat; a disk budget for the log
    directory, rolling, compression and a flush barrier; log export behind the feature
    `log-export`.
- `rivium-http`: an HTTP server service on axum, with a status envelope for answers and errors,
  request ids, an access log, the probes `/livez` and `/readyz`, and file responses.
- `rivium-jni`: `export!`, which generates the JNI library of the embedded host, while the
  service's crate keeps `forbid(unsafe_code)`.
- `rivium-test`: scripted services, log capture, tools to test a program as a process, the
  lifecycle contract suites that consumers run (as a process and embedded), and dependency
  closure checks.
- Platforms (see [docs/platforms.md](docs/platforms.md)), with Rust 1.92 or later:
  - tested natively: x86_64 and aarch64 Linux (glibc 2.17 and later), aarch64 macOS, x86_64
    Windows (GNU toolchain);
  - verified under emulation: armv7 Linux (glibc 2.17 and later), LoongArch64 Linux (new-world
    ABI, glibc 2.36 and later), Android arm64 (the JNI library on a desktop JVM, the other tests
    under a bionic runner);
  - unverified: the LoongArch old-world ABI 1.0, Android devices, armv7 and LoongArch hardware.

### Changed

None.

### Fixed

None.

### Breaking

None.

### Template

- The first template: `cargo generate --git https://github.com/rivium-rs/rivium --tag v0.1.0
  template`. It generates a service library with the composition root, a program, and with
  `jni` a JNI library and its desktop JVM contract; `configs/default.toml`; AGENTS.md with the
  rules for services; clippy, cargo-deny and panic checks; packaging for each platform; and
  `just template-diff`.
