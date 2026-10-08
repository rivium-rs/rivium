# rivium

The service foundation: layered configuration with source tracking, logging with a disk budget, the service lifecycle kernel, the process host (signals, exit codes) and the embedded host (explicit start/stop for FFI callers).

**Status:** pre-release, not published yet. Implemented so far: configuration loading (defaults,
a TOML file, environment variables and `--set` overrides, with every problem reported at once),
`config::Paths`, the value checks in `config::de` and `fs::atomic_write`. See the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
