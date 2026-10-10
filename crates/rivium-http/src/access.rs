//! The contract layer: middleware made of the primitives, for any axum server. It gives every
//! request an id and a span, logs one access line per response with its error, tells the
//! observers, and answers a timeout, a panic, a body that is too large and every other error
//! with the status envelope. The module is named after the access log, whose lines have the
//! target `rivium_http::access`.

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rivium::Error;
use rivium::config::de::{bytes, duration, serialize_bytes, serialize_duration};
use rivium::error::{Class, ErrorKind, ErrorType, log_at_level, log_error};
use serde::{Deserialize, Serialize};
use tracing::{Instrument, Level};

use crate::{ApiError, RequestId, error_of, render};

/// A handler that ran longer than the request timeout.
const TIMED_OUT: ErrorType = &ErrorKind::new("RequestTimedOut", Class::Unavailable);
/// A handler that panicked.
const PANICKED: ErrorType = &ErrorKind::new("HandlerPanicked", Class::Internal);

/// The contract layer's settings. They can be a table of a service's own configuration section;
/// `[http]` ([`HttpSettings`](crate::HttpSettings)) has the same three keys. The defaults:
///
/// ```toml
/// request_timeout = "30s"         # a slower handler is answered with 503 and Retry-After
/// body_limit = "1MiB"             # a larger request body is answered with 413
/// expose_internal_detail = false  # server errors show the whole error, not a fixed phrase
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContractSettings {
    /// How long a handler may take, from 1 second to 1 hour.
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "duration::<_, 1, 3600>"
    )]
    pub request_timeout: Duration,
    /// The largest request body, in bytes, at most 1 GiB, for the extractors that read a
    /// limited body (`Bytes`, `String`, `Json`, `Multipart`); a handler that reads the
    /// `Request` or its `Body` itself is not limited.
    #[serde(
        serialize_with = "serialize_bytes",
        deserialize_with = "bytes::<_, 0, { 1 << 30 }>"
    )]
    pub body_limit: u64,
    /// Whether server errors describe the whole error instead of a fixed phrase.
    pub expose_internal_detail: bool,
}

impl Default for ContractSettings {
    fn default() -> Self {
        ContractSettings {
            request_timeout: Duration::from_secs(30),
            body_limit: 1 << 20,
            expose_internal_detail: false,
        }
    }
}

/// Hears about every response of the contract layer, as its access log line records it: for
/// request counters and metrics.
pub trait HttpObserver: Send + Sync + 'static {
    /// Called once for each response, after its access log line.
    fn on_response(&self, info: &ResponseInfo<'_>);
}

/// A response, as an [`HttpObserver`] sees it.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ResponseInfo<'a> {
    /// The request's method.
    pub method: &'a Method,
    /// The route that matched, such as `/devices/{id}`; `None` when none did.
    pub route: Option<&'a str>,
    /// The response's status.
    pub status: StatusCode,
    /// How long the answer took.
    pub latency: Duration,
    /// The request id, as the header `x-request-id` carries it.
    pub request_id: &'a str,
    /// The error behind the response, if it failed with one.
    pub error: Option<&'a Error>,
}

/// What the outer middleware needs: the observers.
struct Access {
    observers: Vec<Arc<dyn HttpObserver>>,
}

/// What the inner middleware needs: the request timeout, and whether server errors show their
/// detail.
struct Guard {
    timeout: Duration,
    expose: bool,
}

/// Puts the contract layer on `router`, with these settings and observers, which are called in
/// the order given. It is a layer of the router, so it runs after routing: it knows the matched
/// route and covers the fallback and 405.
///
/// From the outside in, it has the request id and span, the access log and the observers, the
/// status envelope, the request timeout, panics and the body limit. Layers added to `router`
/// before run inside it, such as authentication per route (whose 401 gets the envelope unless
/// it carries [`NoEnvelope`](crate::NoEnvelope)). Layers added to the result run outside it and
/// see its final responses: CORS, which then covers the answers to timeouts and panics, and
/// whatever rewrites the body's encoding, such as compression, which must not run inside it.
pub fn contract(
    router: Router,
    settings: &ContractSettings,
    observers: Vec<Arc<dyn HttpObserver>>,
) -> Router {
    let limit = usize::try_from(settings.body_limit).unwrap_or(usize::MAX);
    let guard = Arc::new(Guard {
        timeout: settings.request_timeout,
        expose: settings.expose_internal_detail,
    });
    let access = Arc::new(Access { observers });
    // The last layer added runs first.
    router
        .layer(DefaultBodyLimit::max(limit))
        .layer(middleware::from_fn_with_state(guard, guard_requests))
        .layer(middleware::from_fn_with_state(access, log_requests))
}

/// The outer middleware: the request id, a span with it for everything the request logs, one
/// access log line (DEBUG for the probes `/livez` and `/readyz`), the observers, and the id on
/// the response.
async fn log_requests(
    State(access): State<Arc<Access>>,
    mut request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    // An outer layer may have set one by its own rules.
    let id = match request.extensions().get::<RequestId>() {
        Some(id) => id.clone(),
        None => RequestId::from_headers(request.headers()),
    };
    request.extensions_mut().insert(id.clone());
    let method = request.method().clone();
    let route = request.extensions().get::<MatchedPath>().cloned();
    let route = route.as_ref().map(MatchedPath::as_str);
    let span = tracing::info_span!("request", request_id = %id.as_str());
    let mut response = next.run(request).instrument(span).await;
    let (status, latency) = (response.status(), started.elapsed());
    let error = error_of(&response);
    let level = match status.as_u16() {
        _ if matches!(route, Some("/livez" | "/readyz")) => Level::DEBUG,
        429 | 503 | 504 => Level::WARN,
        500.. => Level::ERROR,
        _ => Level::INFO,
    };
    let (status_code, latency_ms) = (status.as_u16(), latency.as_millis() as u64);
    let (http_route, request_id) = (route.unwrap_or_default(), id.as_str());
    match error.as_deref() {
        Some(error) => log_error!(level, error, http.method = %method, http.route = http_route,
            http.status = status_code, latency_ms, request_id = %request_id, "request"),
        None => log_at_level!(level, http.method = %method, http.route = http_route,
            http.status = status_code, latency_ms, request_id = %request_id, "request"),
    }
    let info = ResponseInfo {
        method: &method,
        route,
        status,
        latency,
        request_id,
        error: error.as_deref(),
    };
    for observer in &access.observers {
        observer.on_response(&info);
    }
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(RequestId::HEADER, value);
    }
    response
}

/// The inner middleware: the status envelope for every error response, the request timeout and
/// a 500 for a panic.
async fn guard_requests(State(guard): State<Arc<Guard>>, request: Request, next: Next) -> Response {
    let id = request.extensions().get::<RequestId>().cloned();
    let run = CatchUnwind(Box::pin(next.run(request)));
    let response = match tokio::time::timeout(guard.timeout, run).await {
        Ok(Ok(response)) => response,
        // The panic hook logs the panic itself.
        Ok(Err(())) => ApiError(Error::explain(PANICKED, "the handler panicked")).into_response(),
        Err(_) => {
            let mut error = Error::explain(TIMED_OUT, "the handler took too long");
            error.set_retry(true);
            ApiError(error).into_response()
        }
    };
    render(response, id.as_ref().map(RequestId::as_str), guard.expose)
}

/// A response future whose panic is caught.
struct CatchUnwind(Pin<Box<dyn Future<Output = Response> + Send>>);

impl Future for CatchUnwind {
    type Output = Result<Response, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match panic::catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(response)) => Poll::Ready(Ok(response)),
            Err(_) => Poll::Ready(Err(())),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ContractSettings;

    #[test]
    fn settings_write_and_read_durations_and_sizes_as_text() {
        let defaults = ContractSettings::default();
        let written = serde_json::to_value(&defaults).unwrap();
        let expected = json!({
            "request_timeout": "30s",
            "body_limit": "1MiB",
            "expose_internal_detail": false,
        });
        assert_eq!(written, expected);
        assert_eq!(
            serde_json::from_value::<ContractSettings>(written).unwrap(),
            defaults
        );
        for (key, value, why) in [
            ("request_timeout", "0s", "must be between 1s and 1h"),
            ("request_timeout", "2h", "must be between 1s and 1h"),
            ("body_limit", "2GiB", "must be between 0 and 1GiB"),
        ] {
            let mut settings = expected.clone();
            settings[key] = value.into();
            let error = serde_json::from_value::<ContractSettings>(settings).unwrap_err();
            assert!(error.to_string().contains(why), "{key}: {error}");
        }
    }
}
