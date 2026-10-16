# rivium-http

HTTP vocabulary and bounded Axum integration for Rivium. Enable only the layers you use:

| Feature | Behavior |
| --- | --- |
| default | `ApiResponse`, `ApiError`, `ApiResult`, safe rendering and `Json`/`Query`/`Path` extractors |
| `contract` | Request IDs, body limits, header deadlines, panic isolation, observations and shared in-flight admission |
| `server` | A supervised HTTP/1 listener, readiness probes and connection ownership; includes `contract` |
| `fs` | Authorized file downloads using Tower's HEAD/Range behavior |
| `full` | `server` and `fs` |

```rust,ignore
use std::num::NonZeroUsize;
use rivium_http::{ContractSettings, HttpServer, ServerLimits};

let server = HttpServer::new("api", address, routes)
    .limits(ServerLimits { connections: NonZeroUsize::new(256).unwrap() })
    .settings(ContractSettings {
        in_flight: NonZeroUsize::new(128).unwrap(),
        request_timeout_ms: 30_000,
        body_limit: 2 * 1024 * 1024,
    });
let supervisor = rivium::Supervisor::default().with(Box::new(server));
let report = supervisor.run().await;
```

`HttpServer` binds and declares its own readiness before waiting for the supervisor's
`Running` phase. It then accepts connections. `/livez` reports supervision liveness;
`/readyz` checks the complete readiness and health state. Probes share the listener's
budgets; separate probe capacity is an application choice.

A connection permit is obtained **before** polling the listener's accept operation. At
capacity, acceptance pauses; TCP backlog behavior is the operating system's responsibility.
Request saturation immediately produces 503 and a request ID without an internal queue.
Request and response bodies share admission ownership with the handler. Returning headers,
finishing one body, or aborting the server root does not release other owners' permits.
The supervisor's drain report remains incomplete while registered connection resources live.

The contract preserves valid caller request IDs and generates UUIDv7 IDs for missing or
invalid ones. Handler logs retain the service dispatch/span and a request-ID span. Access
events use DEBUG for probes, WARN for 429/503/504, ERROR for other 5xx, and INFO otherwise.

The header deadline does not cover a returned streaming response. Streaming applications
must use their own idle/deadline policy and the request's `Extension<rivium::Stop>`.
Declared oversized Content-Length is rejected before the handler. Unknown-length or
underreported bodies are limited as they are polled: the supported JSON collector maps an
observed overflow to 413, while raw streams produce a body error. An unread suffix is not
validated, and an already-sent status cannot be changed.

Diagnostic `Error` message, context and sources stay private. Callers may explicitly use
`Error::with_public_message` for safe caller-facing 4xx text. Authentication, authorization,
upstream and 5xx errors retain safe descriptions. `NoEnvelope` preserves application response
bodies while retaining resource limits. `ApiResponse::Data` and `Created` return bare JSON;
serialization failure returns a safe 500 even without the contract layer.

`Observer` is `Arc<dyn Fn(&Access) + Send + Sync>`; callbacks must be short and nonblocking.
Observer panics are counted by `observer_panics()` and do not change the response. Outer
CORS/compression layers can be applied with `.outer(|router| router.layer(layer))`.

For custom TLS or other Axum listeners use
`serve_listener(listener, router, context, limits)`. The explicit fourth parameter supplies
connection limits. This entry adds connection ownership, the startup gate, `ConnectInfo`
and `Stop`; apply `contract(router, &settings, observers)` yourself if desired. The listener
must be cancellation-safe, with bounded handshake work inside its accept future or explicitly
registered by its owner. Hidden prefetch or detached handshake queues are not constrained by
this adapter. TLS is an application dependency; this crate has no production TLS dependency.
Upgrade/WebSocket ownership is also the application's responsibility.

`file_response(request, authorized_path, download_name)` performs no authorization or tenant
isolation. Resolve and authorize untrusted path input before calling it.
