//! A middleware that keeps the contract with the public parts alone, as the server of a service
//! of its own would: the vocabulary and the primitives `render`, `error_of` and `RequestId`. It
//! passes the same checks as the contract layer, which shows that the public parts suffice.
//! This file is a crate of its own, so it cannot reach anything private.

mod common;

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rivium::error::{Class, Error, ErrorKind, ErrorType, log_at_level, log_error};
use rivium_http::{ApiError, RequestId, error_of, render};
use tracing::{Instrument, Level};

const TIMED_OUT: ErrorType = &ErrorKind::new("RequestTimedOut", Class::Unavailable);
const PANICKED: ErrorType = &ErrorKind::new("HandlerPanicked", Class::Internal);

/// Whether server errors show their detail.
#[derive(Clone, Copy)]
struct Expose(bool);

/// The whole contract in one middleware: the request id and its span, the timeout, panics, the
/// envelope, one access log line with the error, and the id on the response.
async fn keep(State(Expose(expose)): State<Expose>, mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    let id = match request.extensions().get::<RequestId>() {
        Some(id) => id.clone(),
        None => RequestId::from_headers(request.headers()),
    };
    request.extensions_mut().insert(id.clone());
    let method = request.method().clone();
    let route = request.extensions().get::<MatchedPath>();
    let route = route.map(|route| route.as_str().to_string());
    let span = tracing::info_span!("request", request_id = %id.as_str());
    let run = CatchUnwind(Box::pin(next.run(request)));
    let response = match tokio::time::timeout(common::TIMEOUT, run)
        .instrument(span)
        .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(())) => ApiError(Error::explain(PANICKED, "the handler panicked")).into_response(),
        Err(_) => {
            let mut error = Error::explain(TIMED_OUT, "the handler took too long");
            error.set_retry(true);
            ApiError(error).into_response()
        }
    };
    let mut response = render(response, Some(id.as_str()), expose);
    let status = response.status().as_u16();
    let level = match status {
        _ if matches!(route.as_deref(), Some("/livez" | "/readyz")) => Level::DEBUG,
        429 | 503 | 504 => Level::WARN,
        500.. => Level::ERROR,
        _ => Level::INFO,
    };
    let http_route = route.as_deref().unwrap_or_default();
    let latency_ms = started.elapsed().as_millis() as u64;
    match error_of(&response).as_deref() {
        Some(error) => log_error!(level, error, http.method = %method, http.route = http_route,
            http.status = status, latency_ms, request_id = %id.as_str(), "request"),
        None => log_at_level!(level, http.method = %method, http.route = http_route,
            http.status = status, latency_ms, request_id = %id.as_str(), "request"),
    }
    if let Ok(value) = HeaderValue::from_str(id.as_str()) {
        response.headers_mut().insert(RequestId::HEADER, value);
    }
    response
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

#[tokio::test(start_paused = true)]
async fn the_public_parts_keep_the_contract() {
    let logs = rivium_test::capture_logs();
    let limit = usize::try_from(common::BODY_LIMIT).unwrap();
    let keep = |routes: axum::Router, expose| {
        routes
            .layer(DefaultBodyLimit::max(limit))
            .layer(middleware::from_fn_with_state(Expose(expose), keep))
    };
    common::check(keep, &logs).await;
}
