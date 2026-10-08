# rivium-http

The [axum](https://crates.io/crates/axum) integration: an HTTP server that runs as a Rivium service, a fixed status envelope, error-class to status-code mapping, request ids, access logging and liveness/readiness probes.

**Status:** pre-release, not published yet. Implemented: `HttpServer` (a frontline Rivium
service with `[http]` settings), the status envelope (`ApiResponse`, `ApiError`, `status_of`),
the extractors `Json`, `Query` and `Path`, request ids, the access log and `HttpObserver`, the
probes `/livez` and `/readyz`, and `file_response` for downloads. See the
[repository README](https://github.com/rivium-rs/rivium).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
