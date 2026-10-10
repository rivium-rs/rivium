# rivium-http

The [axum](https://crates.io/crates/axum) integration: a fixed status envelope, error-class to status-code mapping, request ids, access logging and liveness/readiness probes, as a contract layer for any axum router and an HTTP server that runs as a Rivium service.

It comes in four layers, each built on the public API of the layers below, so that a service
can stop at any of them:

| Layer | Items | Use |
| --- | --- | --- |
| Vocabulary | `ApiResponse`, `ApiError`, `ApiResult`, `status_of`, `extract::{Json, Query, Path}`, `file_response` | what handlers return and extract |
| Primitives | `render`, `error_of`, `RequestId`, `NoEnvelope` | middleware of a server's own that keeps the contract |
| Contract layer | `contract`, `ContractSettings`, `HttpObserver`, `ResponseInfo` | the whole contract on any axum router, also in `oneshot` tests |
| Server | `HttpServer`, `HttpSettings`, `Acceptor` | a frontline Rivium service: the contract layer, the probes and the listener, with hooks for outer layers and TLS |

Handlers answer with `ApiResponse` or `ApiError`, whose class decides the status; the extractors
answer bad input with the envelope, and `file_response` serves downloads with ranges. The
contract layer gives every request an id, logs one access line per response with its error,
calls the observers, and answers timeouts, panics and every other error with the envelope.
`HttpServer` runs a `Router` in it, with the settings of an `[http]` section, and serves
`/livez` and `/readyz`. Its requests run in the service's span and carry the peer's address and
the service's stop signal, which long responses read to end before the stop deadline; layers
added with `HttpServer::layer` run outside the contract layer, and `accept_with` takes the
service's own TLS acceptor.

What each way of using the crate keeps of the contract; what the vocabulary alone leaves out,
it leaves out silently:

| Contract | Vocabulary | + primitives | Contract layer | `HttpServer` |
| --- | --- | --- | --- | --- |
| Success shapes, statuses, client error descriptions, extractor errors, `Retry-After` | yes | yes | yes | yes |
| Server errors with the request id, `expose_internal_detail`, the envelope for framework errors | no | with `render` | yes | yes |
| Errors logged once, the access log, observers | no | with `error_of` | yes | yes |
| Request ids made, checked, returned and extracted | no | with `RequestId` | yes | yes |
| 503 for a timeout, 500 for a panic, the body limit | no | own code | yes | yes |
| Probes | no | own code | own code | yes |
| The stop signal and `ConnectInfo` in requests, a TLS hook, layers outside the contract | own code | own code | own code | yes |

Part of [Rivium](https://github.com/rivium-rs/rivium): its crates are released together, at one
version, and the repository has the documentation.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
