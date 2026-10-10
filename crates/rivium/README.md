# rivium

The service foundation: layered configuration with source tracking, logging with a disk budget, the service lifecycle kernel, the process host (signals, exit codes) and the embedded host (explicit start/stop for FFI callers).

A service implements `App`, its composition root: the configuration type and the services
(`Service`, or `service()` and `periodic()`). `process::run` runs it as a program of its own, with
signals, exit codes and restarts handled; `embedded::Host` runs it inside another program, such
as an Android app through rivium-jni. Configuration comes from defaults, a TOML file, environment
variables and `--set`, with every problem reported at once; logging goes to the console, files
with a disk budget and, on Android, logcat; with the feature `log-export`, log files can be
packed into an archive to download.

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the examples and the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
