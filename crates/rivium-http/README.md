# rivium-http

The [axum](https://crates.io/crates/axum) integration: an HTTP server that runs as a Rivium service, a fixed status envelope, error-class to status-code mapping, request ids, access logging and liveness/readiness probes.

`HttpServer` runs an axum `Router` as a frontline Rivium service, with the settings of an
`[http]` section: it serves `/livez` and `/readyz`, gives every request an id, logs one access
line per request, and answers with `ApiResponse` or `ApiError`, whose class decides the status.
The extractors `Json`, `Query` and `Path` answer bad input with the envelope, and
`file_response` serves downloads with ranges.

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the template, the examples and the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
