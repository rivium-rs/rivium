# Rivium examples

Each Rust file is an explicit Cargo `[[example]]` target. The private `rivium-examples`
package keeps its dependencies in `[dev-dependencies]`, as Tokio's example workspace does.
Each example contains its own application so it can be read without following a shared demo.

Run these commands from the repository root. Process examples create logs under `--root`;
use a separate root for each running application. Ctrl-C requests graceful shutdown.

| Example | Demonstrates | Command |
| --- | --- | --- |
| [basic-service](basic-service.rs) | Service readiness and process shutdown | `cargo run -p rivium-examples --example basic-service -- --root target/basic-example` |
| [http-server](http-server.rs) | JSON response, request IDs and health probes | `cargo run -p rivium-examples --example http-server -- --root target/http-example --set listen=127.0.0.1:8080` |
| [embedded-host](embedded-host.rs) | Two start/stop cycles without signal handlers | `cargo run -p rivium-examples --example embedded-host -- target/embedded-example` |
| [config-reload](config-reload.rs) | Host reload policy with hot and startup-only settings | See below |
| [jni-service](jni-service.rs) | A native library controlled by a Java bridge | See below |

For the HTTP example, request `http://127.0.0.1:8080/`, `/livez` or `/readyz`.

The process examples also accept `--help`, `--version`, `default-config` and `check-config`.
For example:

```sh
cargo run -p rivium-examples --example basic-service -- default-config
```

## Configuration reload

```sh
mkdir -p target/config-reload-example
cp examples/config-reload.toml target/config-reload-example/config-reload.toml
cargo run -p rivium-examples --example config-reload -- --root target/config-reload-example --config config-reload.toml
```

Edit `target/config-reload-example/config-reload.toml` while the program runs:

- `message` and `interval_ms` are hot settings. The printing service reads their updated
  values through `Context::config()` and resets its timer when a new snapshot arrives.

- `prefix` is copied when the printing service starts. A change is reported in
  `requires_restart`; the service keeps printing the old prefix until the next run.

- Invalid input is reported without replacing the running configuration. The accepted
  interval is 100–60000 milliseconds.

The example's `reload` service calls `Context::reload().await` once per second. This
polling is application code; the host does not watch files or install a reload signal.

Writing the file and requesting reload are separate operations. The host validates the
candidate, calls `App::reload_config` to select effective settings, validates those settings,
and publishes only actual changes.

Generation starts at zero for each run; unchanged or
restart-only edits do not advance it. `published` lists changed fields available to readers,
not acknowledgements from every consumer. Pending restart fields are reported on each poll.

This is a process-host example with the standard `--config`, `--root`, `--set`, inspection
commands and graceful shutdown. CLI or environment overrides still take precedence over
the file.

The example's unit test checks its hot-field selection policy; host reload
contracts are tested in `crates/rivium-host/tests/`.

## JNI library

A full JDK 17 or newer is needed to run the Java example. Set `JAVA_HOME` to its home.
Build the Cargo example and compile its [bridge](java/rivium/examples/Bridge.java):

```sh
cargo build -p rivium-examples --example jni-service --locked
"$JAVA_HOME/bin/javac" --release 17 -d target/example-java examples/java/rivium/examples/Bridge.java
```

On macOS:

```sh
"$JAVA_HOME/bin/java" -Xcheck:jni -cp target/example-java rivium.examples.Bridge \
  target/debug/examples/libjni_service.dylib target/jni-example
```

Use `libjni_service.so` on Linux or `jni_service.dll` on Windows. For JDK versions that
warn about native access, add `--enable-native-access=ALL-UNNAMED` to the Java invocation.

The bridge prints the version and running/stopped states. Android calls to `nativeStart`
should run on a background thread; Android project setup is outside this example.

## Building and testing

```sh
cargo build -p rivium-examples --examples --locked
cargo test -p rivium-examples --example config-reload --locked
```

Fault injection is separate: `tests-integration/test-app/src/lib.rs` supplies the `test-host`
binary and `tests-integration/jni/` test library. They are used by process and JNI contracts;
examples do not expose failure modes or serve as test-fixture dependencies.
