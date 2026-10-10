//! The HTTP server service and its settings: the contract layer, the probes and the listener.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rivium::config::de::{bytes, duration, serialize_bytes, serialize_duration};
use rivium::error::{OrErr, kinds};
use rivium::{
    AppContext, BoxFuture, Health, Readiness, Result, Service, ServiceContext, ServiceKind,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::{ApiResponse, ContractSettings, HttpObserver, NoEnvelope, contract};

/// `[http]`: an HTTP server's settings, for the service's own configuration; the defaults:
///
/// ```toml
/// [http]
/// addr = "127.0.0.1:8080"      # where to listen
/// request_timeout = "30s"      # a slower handler is answered with 503 and Retry-After
/// body_limit = "1MiB"          # a larger request body read by an extractor is answered with 413
/// expose_internal_detail = false  # server errors show the whole error, not a fixed phrase
/// probes = true                # serve /livez and /readyz
/// ```
///
/// The three keys in the middle are the contract layer's [`ContractSettings`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HttpSettings {
    /// Where to listen.
    pub addr: SocketAddr,
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
    /// Whether to serve `/livez` and `/readyz`.
    pub probes: bool,
}

impl Default for HttpSettings {
    fn default() -> Self {
        HttpSettings {
            addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            request_timeout: Duration::from_secs(30),
            body_limit: 1 << 20,
            expose_internal_detail: false,
            probes: true,
        }
    }
}

/// An HTTP server, as a frontline service named `http` unless [named](Self::named) otherwise:
/// the router with the probes, in the [contract layer](crate::contract).
///
/// It binds `addr` while it starts, logs the `listening` event, and serves the router until the
/// service is asked to stop. Then it stops listening at once, closes idle connections and waits
/// for the requests in flight. Requests still running at the stop deadline, and a connection
/// whose first request head has not fully arrived, make the stop time out; their connections
/// end when the host shuts the runtime down, except across an in-process restart, which keeps
/// the runtime: there they may run past the deadline, bounded by the request timeout.
///
/// With probes on, it serves `/livez` (always 200) and `/readyz` (200 once the service runs and
/// every health item is healthy, else 503), and the router must not have these two routes
/// itself. Static files can be served with tower-http's `ServeDir` as a nested service or the
/// router's fallback.
pub struct HttpServer {
    name: Cow<'static, str>,
    settings: HttpSettings,
    router: Router,
    readiness: Readiness,
    observers: Vec<Arc<dyn HttpObserver>>,
}

impl HttpServer {
    /// A server of `router` with these settings, reporting the readiness of `ctx`'s run.
    #[must_use]
    pub fn new(settings: &HttpSettings, router: Router, ctx: &AppContext) -> Self {
        HttpServer {
            name: Cow::Borrowed("http"),
            settings: settings.clone(),
            router,
            readiness: ctx.readiness().clone(),
            observers: Vec::new(),
        }
    }

    /// The same server under another service name, for a service with more than one.
    #[must_use]
    pub fn named(self, name: impl Into<Cow<'static, str>>) -> Self {
        HttpServer {
            name: name.into(),
            ..self
        }
    }

    /// Adds an observer of every response, to the contract layer's.
    #[must_use]
    pub fn observe(mut self, observer: Arc<dyn HttpObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    /// The router with the probes, in the contract layer.
    fn app(self) -> Router {
        let mut router = self.router;
        if self.settings.probes {
            let live = || async { ApiResponse::Data(BTreeMap::from([("status", "live")])) };
            router = router
                .route("/livez", get(live))
                .route("/readyz", get(ready).with_state(self.readiness));
        }
        let mut settings = ContractSettings::default();
        settings.request_timeout = self.settings.request_timeout;
        settings.body_limit = self.settings.body_limit;
        settings.expose_internal_detail = self.settings.expose_internal_detail;
        contract(router, &settings, self.observers)
    }
}

impl Service for HttpServer {
    fn name(&self) -> Cow<'static, str> {
        self.name.clone()
    }

    fn kind(&self) -> ServiceKind {
        ServiceKind::Frontline
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let addr = self.settings.addr;
            let listener = TcpListener::bind(addr).await;
            let listener =
                listener.or_err_with(kinds::UNAVAILABLE, || format!("binding {addr}"))?;
            let bound = listener.local_addr();
            ctx.listening(bound.or_err(kinds::UNAVAILABLE, "reading the bound address")?);
            let app = self.app();
            ctx.ready();
            let served = axum::serve(listener, app).with_graceful_shutdown(ctx.stopped());
            served.await.or_err(kinds::UNAVAILABLE, "serving HTTP")
        })
    }
}

/// `/readyz`: 200 or 503, with the phase and each health item's state, without reasons.
async fn ready(State(readiness): State<Readiness>) -> Response {
    let checks: BTreeMap<String, &str> = (readiness.checks().into_iter())
        .map(|(name, health)| {
            let state = match health {
                Health::Healthy => "healthy",
                Health::Unhealthy(_) => "unhealthy",
            };
            (name.into_owned(), state)
        })
        .collect();
    let (status, word) = match readiness.is_ready() {
        true => (StatusCode::OK, "ready"),
        false => (StatusCode::SERVICE_UNAVAILABLE, "not ready"),
    };
    #[derive(Serialize)]
    struct Ready<'a> {
        status: &'a str,
        phase: &'a str,
        checks: BTreeMap<String, &'a str>,
    }
    let phase = readiness.phase().as_str();
    let ready = Ready {
        status: word,
        phase,
        checks,
    };
    // Not the envelope, even when not ready.
    (status, NoEnvelope, ApiResponse::Data(ready)).into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::HttpSettings;

    #[test]
    fn settings_write_and_read_durations_and_sizes_as_text() {
        let defaults = HttpSettings::default();
        let written = serde_json::to_value(&defaults).unwrap();
        let expected = json!({
            "addr": "127.0.0.1:8080",
            "request_timeout": "30s",
            "body_limit": "1MiB",
            "expose_internal_detail": false,
            "probes": true,
        });
        assert_eq!(written, expected);
        assert_eq!(
            serde_json::from_value::<HttpSettings>(written).unwrap(),
            defaults
        );
        for (key, value, why) in [
            ("request_timeout", "0s", "must be between 1s and 1h"),
            ("body_limit", "2GiB", "must be between 0 and 1GiB"),
        ] {
            let mut settings = expected.clone();
            settings[key] = value.into();
            let error = serde_json::from_value::<HttpSettings>(settings).unwrap_err();
            assert!(error.to_string().contains(why), "{key}: {error}");
        }
    }
}
