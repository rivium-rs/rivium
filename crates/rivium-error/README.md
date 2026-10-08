# rivium-error

The single error type of Rivium services: an error kind with a transport-independent class, the responsible party, a retry hint, context and a cause chain, plus structured log fields. It is part of every service's public API, so it stays small and depends only on `tracing`.

**Status:** pre-release skeleton. Nothing is implemented or published yet; see the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
