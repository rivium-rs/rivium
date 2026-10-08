//! The middleware of every request: its id, span and access log line, the observers, the
//! request timeout, the status envelope and panics.

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rivium::Error;
use rivium::error::{Class, ErrorKind, ErrorType, log_at_level, log_error};
use tracing::{Instrument, Level};
use uuid::Uuid;

use crate::ApiError;
use crate::response::{Failed, render};

/// A handler that ran longer than `http.request_timeout`.
const TIMED_OUT: ErrorType = &ErrorKind::new("RequestTimedOut", Class::Unavailable);
/// A handler that panicked.
const PANICKED: ErrorType = &ErrorKind::new("HandlerPanicked", Class::Internal);
/// The header that carries the request id.
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Hears about every response of an [`HttpServer`](crate::HttpServer), as its access log line
/// records it: for request counters and metrics.
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

/// The request's id, for the inner middleware.
#[derive(Clone)]
struct RequestId(Arc<str>);

/// What the outer middleware needs: whether the probes are served, and the observers.
pub(crate) struct Access {
    pub(crate) probes: bool,
    pub(crate) observers: Vec<Arc<dyn HttpObserver>>,
}

/// What the inner middleware needs: the request timeout, and whether server errors show their
/// detail.
pub(crate) struct Guard {
    pub(crate) timeout: Duration,
    pub(crate) expose: bool,
}

/// The outer middleware: the request id (the caller's if it is 1 to 128 visible ASCII
/// characters, else a new UUIDv7), a span with it for everything the request logs, one access
/// log line, the observers, and the id on the response.
pub(crate) async fn access(
    State(access): State<Arc<Access>>,
    mut request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let id = request_id(request.headers());
    request.extensions_mut().insert(RequestId(Arc::clone(&id)));
    let method = request.method().clone();
    let route = request.extensions().get::<MatchedPath>().cloned();
    let route = route.as_ref().map(MatchedPath::as_str);
    let span = tracing::info_span!("request", request_id = %id);
    let mut response = next.run(request).instrument(span).await;
    let (status, latency) = (response.status(), started.elapsed());
    let failed = response.extensions().get::<Failed>().cloned();
    let error = failed.as_ref().map(|failed| &*failed.0);
    let probe = access.probes && matches!(route, Some("/livez" | "/readyz"));
    let level = match status.as_u16() {
        _ if probe => Level::DEBUG,
        429 | 503 | 504 => Level::WARN,
        500.. => Level::ERROR,
        _ => Level::INFO,
    };
    let (status_code, latency_ms) = (status.as_u16(), latency.as_millis() as u64);
    let http_route = route.unwrap_or_default();
    match error {
        Some(error) => log_error!(level, error, http.method = %method, http.route = http_route,
            http.status = status_code, latency_ms, request_id = %id, "request"),
        None => log_at_level!(level, http.method = %method, http.route = http_route,
            http.status = status_code, latency_ms, request_id = %id, "request"),
    }
    let info = ResponseInfo {
        method: &method,
        route,
        status,
        latency,
        request_id: &id,
        error,
    };
    for observer in &access.observers {
        observer.on_response(&info);
    }
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID, value);
    }
    response
}

fn request_id(headers: &HeaderMap) -> Arc<str> {
    let given = headers.get(REQUEST_ID).map(HeaderValue::as_bytes);
    let valid = |id: &&[u8]| (1..=128).contains(&id.len()) && id.iter().all(u8::is_ascii_graphic);
    match given.filter(valid) {
        Some(id) => String::from_utf8_lossy(id).into(),
        None => Uuid::now_v7().to_string().into(),
    }
}

/// The inner middleware: the request timeout, a 500 for a panic, and the status envelope for
/// every error response.
pub(crate) async fn guard(
    State(guard): State<Arc<Guard>>,
    request: Request,
    next: Next,
) -> Response {
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
    render(response, id.as_ref().map(|id| &*id.0), guard.expose)
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
