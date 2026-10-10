//! The HTTP server service and its settings: the contract layer, the probes and the listener,
//! with the hooks for layers outside the contract layer and for TLS.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{Route, get};
use axum::serve::{Listener, ListenerExt};
use axum::{Extension, Router};
use rivium::config::de::{bytes, duration, serialize_bytes, serialize_duration};
use rivium::error::{OrErr, kinds};
use rivium::{
    AppContext, BoxFuture, Health, Readiness, Result, Service, ServiceContext, ServiceKind,
    StopSignal,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tower::Layer;
use tracing::{Instrument, Span};

use crate::{ApiResponse, ContractSettings, HttpObserver, NoEnvelope, contract};

/// How long an [`Acceptor`] may take for one connection.
const HANDSHAKE: Duration = Duration::from_secs(10);

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

/// Turns an accepted connection into the stream that HTTP is served on, such as a TLS stream
/// of the service's own TLS library: see [`HttpServer::accept_with`].
pub trait Acceptor: Send + Sync + 'static {
    /// The stream that HTTP is served on.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Turns `stream`, just accepted, into the stream that HTTP is served on, such as with a
    /// TLS handshake. An error drops the connection; the acceptor logs it if it should be.
    fn accept(&self, stream: TcpStream) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// An HTTP server, as a frontline service named `http` unless [named](Self::named) otherwise:
/// the router with the probes, in the [contract layer](crate::contract).
///
/// It binds `addr` while it starts, logs the `listening` event, and serves the router until the
/// service is asked to stop. Every request runs in the service's span, so the lines it logs
/// name the server, and carries its peer's address as `ConnectInfo<SocketAddr>` and the
/// service's [`StopSignal`] as an extension: a long response, such as server-sent events or a
/// streamed download, reads `Extension<StopSignal>` and ends itself before the stop deadline.
///
/// At the stop it stops listening at once, closes idle connections and waits for the requests
/// in flight. Requests still running at the stop deadline, and a connection whose first request
/// head has not fully arrived, make the stop time out; their connections end when the host
/// shuts the runtime down, except across an in-process restart, which keeps the runtime: there
/// they may run past the deadline, bounded by the request timeout. Connections that upgrade,
/// such as WebSocket sessions, are axum's own tasks and end at the stop only if they read the
/// stop signal.
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
    layers: Vec<Box<dyn FnOnce(Router) -> Router + Send>>,
    acceptor: Option<Arc<dyn Accept>>,
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
            layers: Vec::new(),
            acceptor: None,
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

    /// Adds `layer` outside the contract layer, as `Router::layer` adds it to the router the
    /// contract layer is on, so that it sees every final response, the answers to timeouts and
    /// panics among them: CORS, compression, or a layer that puts a
    /// [`RequestId`](crate::RequestId) of the service's own rules in the request. A layer added
    /// later runs outside those added before.
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        self.layers
            .push(Box::new(move |router| router.layer(layer)));
        self
    }

    /// Serves HTTP on the streams that `acceptor` makes of the accepted connections, such as
    /// the TLS streams of the service's own TLS library; the service still binds `addr`.
    /// Handshakes run side by side, each for at most 10 seconds, so that a client that never
    /// finishes one holds back no other. A connection whose handshake fails or takes longer is
    /// dropped, and the handshakes still running at the stop end with the listener.
    #[must_use]
    pub fn accept_with(mut self, acceptor: impl Acceptor) -> Self {
        self.acceptor = Some(Arc::new(acceptor));
        self
    }

    /// The router with the probes, in the contract layer, with the outer layers, the stop
    /// signal and the service's span.
    fn app(self, stop: StopSignal) -> Router {
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
        let mut app = contract(router, &settings, self.observers);
        for layer in self.layers {
            app = layer(app);
        }
        // axum serves each connection in a task of its own, outside the service's span.
        let span = Span::current();
        app.layer(Extension(stop))
            .layer(middleware::from_fn_with_state(span, in_span))
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
            let acceptor = self.acceptor.clone();
            let app = self.app(ctx.stop_signal());
            let app = app.into_make_service_with_connect_info::<SocketAddr>();
            ctx.ready();
            let stopped = ctx.stopped();
            let served = match acceptor {
                None => {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(stopped)
                        .await
                }
                Some(acceptor) => {
                    // `tap_io` gives the listener axum's connect info for its address.
                    let listener = Accepting::new(listener, acceptor).tap_io(|_| {});
                    axum::serve(listener, app)
                        .with_graceful_shutdown(stopped)
                        .await
                }
            };
            served.or_err(kinds::UNAVAILABLE, "serving HTTP")
        })
    }
}

/// Runs the request in `span`, the service's.
async fn in_span(State(span): State<Span>, request: Request, next: Next) -> Response {
    next.run(request).instrument(span).await
}

/// A stream that HTTP can be served on.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// An [`Acceptor`] whose stream type is erased, so that `HttpServer` stays one type.
trait Accept: Send + Sync + 'static {
    fn accept(&self, stream: TcpStream) -> BoxFuture<'_, io::Result<Box<dyn Io>>>;
}

impl<A: Acceptor> Accept for A {
    fn accept(&self, stream: TcpStream) -> BoxFuture<'_, io::Result<Box<dyn Io>>> {
        Box::pin(async move {
            let stream = Acceptor::accept(self, stream).await?;
            Ok(Box::new(stream) as Box<dyn Io>)
        })
    }
}

/// The listener of a server with an acceptor: it hands axum a connection once the acceptor is
/// done with it. Handshakes run side by side, each for at most [`HANDSHAKE`], and those still
/// running when axum drops the listener at the stop end with it.
struct Accepting {
    tcp: TcpListener,
    acceptor: Arc<dyn Accept>,
    handshakes: JoinSet<Option<(Box<dyn Io>, SocketAddr)>>,
}

impl Accepting {
    fn new(tcp: TcpListener, acceptor: Arc<dyn Accept>) -> Self {
        Accepting {
            tcp,
            acceptor,
            handshakes: JoinSet::new(),
        }
    }
}

impl Listener for Accepting {
    type Io = Box<dyn Io>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Box<dyn Io>, SocketAddr) {
        loop {
            tokio::select! {
                (stream, peer) = Listener::accept(&mut self.tcp) => {
                    let acceptor = Arc::clone(&self.acceptor);
                    self.handshakes.spawn(async move {
                        let handshake = tokio::time::timeout(HANDSHAKE, acceptor.accept(stream));
                        match handshake.await {
                            Ok(Ok(stream)) => Some((stream, peer)),
                            _ => None,
                        }
                    });
                }
                Some(done) = self.handshakes.join_next(), if !self.handshakes.is_empty() => {
                    if let Ok(Some(accepted)) = done {
                        return accepted;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
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
