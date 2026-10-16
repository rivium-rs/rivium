use crate::{ApiError, RequestId, body, error_of, render};
use axum::{
    Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use futures_util::FutureExt;
use rivium::{Error, error::kinds};
use std::{
    num::NonZeroUsize,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tracing::Instrument;

/// Request budgets for one shared contract instance.
#[derive(Clone, Debug)]
pub struct ContractSettings {
    /// Maximum handler/request/response ownership groups, with immediate 503 on saturation.
    pub in_flight: NonZeroUsize,
    /// Budget to obtain response headers; does not bound streaming afterward.
    pub request_timeout_ms: u64,
    /// Maximum declared or actually consumed request body bytes.
    pub body_limit: usize,
}
impl Default for ContractSettings {
    fn default() -> Self {
        Self {
            in_flight: NonZeroUsize::new(256).unwrap(),
            request_timeout_ms: 30_000,
            body_limit: 2 * 1024 * 1024,
        }
    }
}
/// Header-completion observation; payload completion is a separate ownership event.
#[derive(Clone, Debug)]
pub struct Access {
    pub request_id: String,
    pub method: String,
    /// Matched route template, or URI path when routing did not match; excludes query text.
    pub path: String,
    pub status: u16,
    pub elapsed: Duration,
    /// Private diagnostic failure; never copy its message into client responses.
    pub error: Option<Error>,
}
/// Synchronous short callback. Panics are contained; blocking callbacks remain caller misuse.
pub type Observer = Arc<dyn Fn(&Access) + Send + Sync>;
static OBSERVER_PANICS: AtomicU64 = AtomicU64::new(0);
/// Process-cumulative contained observer panics.
pub fn observer_panics() -> u64 {
    OBSERVER_PANICS.load(Ordering::Relaxed)
}
#[derive(Clone)]
struct Contract {
    settings: ContractSettings,
    capacity: Arc<Semaphore>,
    observers: Vec<Observer>,
}
/// Add request IDs, bounded admission/bodies, header deadlines, panic isolation and safe envelopes.
pub fn contract(router: Router, settings: &ContractSettings, observers: Vec<Observer>) -> Router {
    let state = Contract {
        settings: settings.clone(),
        capacity: Arc::new(Semaphore::new(settings.in_flight.get())),
        observers,
    };
    router
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(state, enforce))
}
async fn enforce(State(contract): State<Contract>, mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    let id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| RequestId::validated(id.as_str()).unwrap_or_default())
        .unwrap_or_else(|| RequestId::from_headers(request.headers()));
    request.extensions_mut().insert(id.clone());
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str())
        .unwrap_or(request.uri().path())
        .to_owned();
    let mut lease = None;
    let response = match contract.capacity.clone().try_acquire_owned() {
        Err(_) => ApiError(Error::new(
            kinds::UNAVAILABLE,
            "HTTP in-flight limit reached",
        ))
        .into_response(),
        Ok(permit) => {
            let owned = Arc::new(permit);
            lease = Some(owned.clone());
            let declared = content_length(request.headers());
            match declared {
                Err(()) => ApiError(Error::invalid("invalid content length")).into_response(),
                Ok(Some(size)) if size > contract.settings.body_limit as u64 => ApiError(
                    Error::new(kinds::PAYLOAD_TOO_LARGE, "declared body exceeds budget"),
                )
                .into_response(),
                _ => {
                    let (parts, incoming) = request.into_parts();
                    let incoming =
                        body::hold(body::limit(incoming, contract.settings.body_limit), owned);
                    let request = Request::from_parts(parts, incoming);
                    match tokio::time::timeout(
                        Duration::from_millis(contract.settings.request_timeout_ms),
                        AssertUnwindSafe(
                            next.run(request)
                                .instrument(tracing::info_span!("request", request_id = %id)),
                        )
                        .catch_unwind(),
                    )
                    .await
                    {
                        Ok(Ok(response)) => response,
                        Ok(Err(_)) => {
                            ApiError(Error::internal("HTTP handler panicked")).into_response()
                        }
                        Err(_) => ApiError(Error::new(
                            kinds::TIMEOUT,
                            "HTTP response header deadline exceeded",
                        ))
                        .into_response(),
                    }
                }
            }
        }
    };
    let mut response = render(response, Some(id.as_str()));
    response.headers_mut().insert(
        RequestId::HEADER,
        HeaderValue::from_str(id.as_str()).expect("validated request id"),
    );
    let access = Access {
        request_id: id.0,
        method,
        path,
        status: response.status().as_u16(),
        elapsed: started.elapsed(),
        error: error_of(&response),
    };
    for observer in &contract.observers {
        if std::panic::catch_unwind(AssertUnwindSafe(|| observer(&access))).is_err() {
            OBSERVER_PANICS.fetch_add(1, Ordering::Relaxed);
        }
    }
    log_access(&access);
    if let Some(lease) = lease {
        let (parts, response_body) = response.into_parts();
        response = Response::from_parts(parts, body::hold(response_body, lease));
    }
    response
}

fn content_length(headers: &axum::http::HeaderMap) -> std::result::Result<Option<u64>, ()> {
    let mut length = None;
    for value in headers.get_all(header::CONTENT_LENGTH) {
        for item in value.to_str().map_err(|_| ())?.split(',') {
            let value = item.trim().parse::<u64>().map_err(|_| ())?;
            if length.is_some_and(|previous| previous != value) {
                return Err(());
            }
            length = Some(value);
        }
    }
    Ok(length)
}

fn log_access(access: &Access) {
    macro_rules! record {
        ($level:expr) => {
            tracing::event!(
                $level,
                request_id = %access.request_id,
                method = %access.method,
                path = %access.path,
                status = access.status,
                elapsed_ms = access.elapsed.as_millis() as u64,
                error = ?access.error,
                "request"
            )
        };
    }
    match (access.path.as_str(), access.status) {
        ("/livez" | "/readyz", _) => record!(tracing::Level::DEBUG),
        (_, 429 | 503 | 504) => record!(tracing::Level::WARN),
        (_, 500..) => record!(tracing::Level::ERROR),
        _ => record!(tracing::Level::INFO),
    }
}
