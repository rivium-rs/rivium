# rivium

The service foundation: layered configuration with source tracking, logging with a disk budget, the service lifecycle kernel, the process host (signals, exit codes) and the embedded host (explicit start/stop for FFI callers).

**Status:** pre-release, not published yet. Implemented so far: configuration loading (defaults,
a TOML file, environment variables and `--set` overrides, with every problem reported at once),
`config::Paths`, the value checks in `config::de`, logging (console, files with rolling,
compression and a disk budget, Android's log, the flush barrier, the panic hook), the service
contract (`Service`, `ServiceContext`, `service()`, `periodic()`, readiness), the lifecycle
kernel that supervises services (`lifecycle::Supervisor`), the process host (`App`,
`process::run`, the exit codes of `Code`, in-process restarts), `fs::atomic_write` and
`stats()`. The embedded host comes next. See the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
