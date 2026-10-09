# Working on {{project-name}}

{{project-name}} is a service built on [Rivium](https://github.com/rivium-rs/rivium). Rivium owns
what every service needs and gets wrong in the same ways: the service lifecycle (start,
readiness, stop with a deadline, restarts, exit codes), configuration loading, logging with a disk
budget, and the hosts that run the services{% if jni %} as a program or inside an Android app{% endif %}. This project owns
the business. Rivium's fixes reach it through `cargo update`; do not copy Rivium code into it.

## Layout

| Path | What goes there |
| --- | --- |
| `crates/{{project-name}}/` | The service library: the business, and the composition root (`{{project-name | pascal_case}}` in `src/lib.rs`), which builds the services from the configuration |
| `crates/{{project-name}}-bin/` | The program: `main` hands the composition root to `rivium::process::run` and nothing else |{% if jni %}
| `crates/{{project-name}}-jni/` | The JNI library for the Android app: one `rivium_jni::export!` line. `jvm-test/` holds the Java mirror of the app's bridge class and the JVM contract |{% endif %}
| `configs/default.toml` | The default configuration, exactly what `{{project-name}} default-config` prints: `just default-config` rewrites it, a test checks it |
| `scripts/package.sh` | Builds and archives the program for each platform, with that platform's configuration |
| `.rivium-template` | The template version and answers that generated this project, for `just template-diff` |

Inside the service library, keep the domain, the application logic and the adapters to the
outside (network, devices, files) in modules of their own, and keep each module's visibility as
narrow as it can be. Split a crate off only for a reason: code shared by several programs, a
heavy dependency to isolate, a crate to publish, or a measured build-time problem.

## Adding a service, a section or a program

1. A service is a `Service`, or one built with `rivium::service(name, kind, |ctx| async move { .. })`
   or `rivium::periodic(name, every, tick)`. Add it to the vector that `services()` returns.
2. Its settings are a section of `Config`: a struct with its own `Default`, as `[heartbeat]` is.
   Then run `just default-config`.
3. A second program is a bin package (or `src/bin/<name>.rs`) with a second `App` in the service
   library, which picks the services it runs.

Nothing in Rivium needs to change for any of this. If something seems to, report it to Rivium
instead of working around it here.

## Rules for services

- **Build, then run.** `services()` only constructs: it never spawns, waits or does I/O. Work
  that waits, such as connecting, goes into the service's `run`, before `ctx.ready()`.
- **Frontline or background.** A frontline service takes outside traffic; it stops first, and
  returning before it is asked to stop is a failure. A background service stops after them, and
  may finish early.
- **No detached tasks.** `tokio::spawn`, `tokio::task::spawn`, `tokio::task::spawn_blocking` and
  `std::thread::spawn` are forbidden (clippy.toml). Start tasks with `ctx.spawn(name, future)`
  and blocking work with `ctx.spawn_blocking(name, f)`: they belong to the service, get its stop
  request and deadline, and their failure is the service's.
- **Blocking work** runs only in `ctx.spawn_blocking`. Give every blocking call a timeout below
  `lifecycle.stop_timeout`, and check the service's stop signal between calls
  (`let stop = ctx.stop_signal();` … `while !stop.is_stopping() { .. }`): a deadline can drop an
  async task, but not a blocked thread.
- **Blocking reads** retry on `std::io::ErrorKind::Interrupted` as on a timeout: on Linux a
  signal interrupts a read with a timeout, even with `SA_RESTART`, and so does stopping and
  continuing the process.
- **Readiness.** Call `ctx.ready()` once the service really takes traffic. A service whose
  blocking loop reads a socket is ready once the loop has started to read: have the loop send on
  a `tokio::sync::oneshot` channel just before its first read, and call `ready()` when it
  arrives. Rivium's `udp-echo` example shows it.
- **Stopping.** `ctx.stopped()` completes when it is the service's turn to stop; plan the
  remaining work with `ctx.deadline()`, such as `tokio::time::timeout_at(deadline, flush())`.
- **Never exit the process** (`std::process::exit`), write a PID file or daemonize: the host
  owns the process. To restart after a new configuration, check it with `ctx.check_config(..)`,
  write it with `rivium::fs::atomic_write(path, bytes, true)` and call `ctx.restarter().request(..)`.

## Errors, configuration and logs

- Errors are `rivium::error::Error` values. Define each kind once, as a constant:
  `const DEVICE: ErrorType = &ErrorKind::new("DeviceError", Class::Unavailable);`. Add context
  with `or_err`, `explain_err` and `more_context`; mark the party responsible (`into_up()`,
  `into_down()`) last. A failure is logged once, where it is handled: Rivium logs the failures of
  services, and rivium-http those of requests.
- The configuration is plain serde: `Option` fields and maps are fine; `#[serde(flatten)]` and
  untagged enums are not (problems could not name the key). The top-level sections `log` and
  `lifecycle` are Rivium's. Check ranges with `rivium::config::de` (durations, byte sizes,
  integers). Paths in the configuration are relative to the root: `ctx.paths().resolve(path)`.
- Each setting can be overridden as `--set section.key=value` or by the environment variable
  `{{project-name | shouty_snake_case}}_SECTION__KEY`.
- Log with `tracing`. Spans of services and tasks are added by Rivium.

## Dependencies

- **TLS**: rustls with the ring provider, never aws-lc or OpenSSL (deny.toml bans them, and the
  dependency-closure test checks every package). Install the provider when building the
  services; a later start in the same process finds it installed, which is fine:

  ```rust
  // In services(): the result is an error only when a provider is installed already.
  let _ = rustls::crypto::ring::default_provider().install_default();
  ```
- No C or C++ build chain beyond `cc`, which ring needs (no CMake). Keep default features off
  where you do not need them.
- Features that exclude each other (one per vendor, say) are better chosen at run time from the
  configuration. If they must be compile-time features, list them for `just check-variants`;
  never build with `--all-features`.
- Keep `panic = "unwind"` (the default) in every profile: Rivium turns a panic into a service
  failure{% if jni %} and, in the JNI library, into a result code; with `abort` a panic would end the app{% endif %}.
  `scripts/check-panic.sh` checks it.{% if jni %}

## The JNI library

- `crates/{{project-name}}-jni` is one `rivium_jni::export!` line and keeps
  `#![forbid(unsafe_code)]`: the unsafe code is in rivium-jni.
- The app declares the native methods on `{{bridge_package}}.RiviumBridge` exactly as
  `jvm-test/RiviumBridge.java` does (in Kotlin: an `object` with `@JvmStatic external fun`).
  A declaration that differs fails when the library loads.
- The app calls `nativeStart` on a background thread, never the main thread, with `--root` set to
  its files directory and any `--set` overrides; it may call `nativeStop` on the main thread
  with at most 2000 ms. `just jvm` runs the contract on a desktop JVM.{% endif %}

## Checks and upgrades

- `just check` before every commit: formatting, clippy, cargo-deny, the panic check, and the
  tests. The tests include the lifecycle contract that Rivium's hosts keep (as a program{% if jni %} and
  embedded{% endif %}), the dependency closure of each package, and `configs/default.toml`.
- Rivium's crates are released together, at one version. A patch release fixes without changing
  the API: `just update-rivium` moves every Rivium crate to it (only Cargo.lock changes), then
  build and test. A new minor version is breaking in 0.x: change the versions in Cargo.toml,
  follow Rivium's upgrading guide, run `just default-config`, and run `just template-diff
  <version>` to see what changed in the files the template generated; merge what you need by
  hand.
