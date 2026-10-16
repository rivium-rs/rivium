use crate::{ContractSettings, Observer, body, contract};
use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request},
    http::StatusCode,
    response::Response,
    routing::get,
    serve::{IncomingStream, Listener},
};
use rivium::{
    BoxFuture, Context, Kind, Phase, Result, Service, Stop,
    scope::{Reservation, TaskScope},
};
use std::{
    convert::Infallible,
    fmt::Debug,
    io,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tower::{Service as TowerService, ServiceExt};
use tracing::{Instrument, instrument::WithSubscriber};

/// Per-listener connection budget, acquired before `Listener::accept` is polled.
#[derive(Clone, Copy, Debug)]
pub struct ServerLimits {
    pub connections: NonZeroUsize,
}
impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            connections: NonZeroUsize::new(256).unwrap(),
        }
    }
}
enum Binding {
    Address(SocketAddr),
    Listener(TcpListener),
}
/// A frontline HTTP/1 service with safe contracts and explicit connection ownership.
pub struct HttpServer {
    name: String,
    binding: Binding,
    router: Router,
    settings: ContractSettings,
    limits: ServerLimits,
    observers: Vec<Observer>,
    outer: Vec<Box<dyn FnOnce(Router) -> Router + Send>>,
}
impl HttpServer {
    /// Bind during supervised initialization.
    pub fn new(name: impl Into<String>, address: SocketAddr, router: Router) -> Self {
        Self {
            name: name.into(),
            binding: Binding::Address(address),
            router,
            settings: ContractSettings::default(),
            limits: ServerLimits::default(),
            observers: Vec::new(),
            outer: Vec::new(),
        }
    }
    /// Take ownership of an already bound listener, without beginning acceptance.
    pub fn from_listener(
        name: impl Into<String>,
        listener: TcpListener,
        router: Router,
    ) -> Result<Self> {
        let address = listener.local_addr()?;
        let mut server = Self::new(name, address, router);
        server.binding = Binding::Listener(listener);
        Ok(server)
    }
    /// Configure shared request admission and header/body budgets.
    pub fn settings(mut self, settings: ContractSettings) -> Self {
        self.settings = settings;
        self
    }
    /// Configure accepted-connection budget.
    pub fn limits(mut self, limits: ServerLimits) -> Self {
        self.limits = limits;
        self
    }
    /// Add a short synchronous access observer.
    pub fn observe(mut self, observer: Observer) -> Self {
        self.observers.push(observer);
        self
    }
    /// Apply application layers after the contract, on its outside.
    pub fn outer(mut self, layer: impl FnOnce(Router) -> Router + Send + 'static) -> Self {
        self.outer.push(Box::new(layer));
        self
    }
}
impl Service for HttpServer {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> Kind {
        Kind::Frontline
    }
    fn run(self: Box<Self>, context: Context) -> BoxFuture<Result<()>> {
        Box::pin(async move {
            let listener = match self.binding {
                Binding::Address(address) => TcpListener::bind(address).await?,
                Binding::Listener(listener) => listener,
            };
            let live = context.readiness();
            let ready = live.clone();
            let router = self
                .router
                .route(
                    "/livez",
                    get(move || {
                        let state = live.clone();
                        async move {
                            if state.phase() == Phase::Stopped {
                                StatusCode::SERVICE_UNAVAILABLE
                            } else {
                                StatusCode::OK
                            }
                        }
                    }),
                )
                .route(
                    "/readyz",
                    get(move || {
                        let state = ready.clone();
                        async move {
                            if state.is_ready() {
                                StatusCode::OK
                            } else {
                                StatusCode::SERVICE_UNAVAILABLE
                            }
                        }
                    }),
                );
            let mut router = contract(router, &self.settings, self.observers);
            for layer in self.outer {
                router = layer(router);
            }
            tracing::info!(
                service = %context.name(),
                address = %listener.local_addr()?,
                "listener bound"
            );
            serve_listener(listener, router, context, self.limits).await
        })
    }
}
struct ConnectionLease {
    _capacity: OwnedSemaphorePermit,
    _scope: Reservation,
}
struct OwnedIo<I> {
    inner: I,
    lease: Arc<ConnectionLease>,
}
impl<I: AsyncRead + Unpin> AsyncRead for OwnedIo<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buffer)
    }
}
impl<I: AsyncWrite + Unpin> AsyncWrite for OwnedIo<I> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buffer)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, buffers)
    }
}
struct Bounded<L> {
    inner: L,
    capacity: Arc<Semaphore>,
    scope: TaskScope,
    stop: Stop,
}
impl<L: Listener> Listener for Bounded<L> {
    type Io = OwnedIo<L::Io>;
    type Addr = L::Addr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let permit = tokio::select! {
                biased;
                _ = self.stop.stopped() => std::future::pending().await,
                permit = self.capacity.clone().acquire_owned() =>
                    permit.expect("connection capacity is never closed")
            };
            let (io, address) = tokio::select! {
                biased;
                _ = self.stop.stopped() => std::future::pending().await,
                accepted = self.inner.accept() => accepted
            };
            if let Some(mut reservations) = self.scope.reserve(1) {
                let lease = Arc::new(ConnectionLease {
                    _capacity: permit,
                    _scope: reservations.pop().unwrap(),
                });
                return (OwnedIo { inner: io, lease }, address);
            }
            // Closure races must release the accepted IO before its capacity permit.
            drop(io);
            drop(permit);
        }
    }
    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}
#[derive(Clone)]
struct ConnectionService<A> {
    router: Router,
    address: A,
    lease: Arc<ConnectionLease>,
    dispatch: tracing::Dispatch,
    span: tracing::Span,
}
impl<A: Clone + Send + Sync + 'static> TowerService<Request> for ConnectionService<A> {
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<std::result::Result<Response, Infallible>>;
    fn poll_ready(&mut self, _: &mut TaskContext<'_>) -> Poll<std::result::Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, mut request: Request) -> Self::Future {
        request
            .extensions_mut()
            .insert(ConnectInfo(self.address.clone()));
        let (parts, incoming) = request.into_parts();
        let lease = self.lease.clone();
        let router = self.router.clone();
        Box::pin(
            (async move {
                let response = router
                    .oneshot(Request::from_parts(
                        parts,
                        body::hold(incoming, lease.clone()),
                    ))
                    .await?;
                let (parts, outgoing) = response.into_parts();
                Ok(Response::from_parts(parts, body::hold(outgoing, lease)))
            })
            .instrument(self.span.clone())
            .with_subscriber(self.dispatch.clone()),
        )
    }
}
/// Serve a cancellation-safe Axum listener, including a TLS adapter, with owned connections.
/// The router is used as supplied; call `contract` for request budgets and envelopes.
/// `limits` is explicit. The listener must not hide unbounded prefetch/handshake workers.
pub async fn serve_listener<L>(
    listener: L,
    router: Router,
    context: Context,
    limits: ServerLimits,
) -> Result<()>
where
    L: Listener,
    L::Addr: Clone + Debug + Send + Sync + 'static,
{
    let stop = context.stop();
    context.ready();
    let mut readiness = context.readiness();
    while readiness.phase() != Phase::Running {
        tokio::select! {
            biased;
            _ = stop.stopped() => return Ok(()),
            changed = readiness.changed() => if changed.is_none() {
                return Ok(());
            }
        }
    }
    let scope = context.task_scope();
    let listener = Bounded {
        inner: listener,
        capacity: Arc::new(Semaphore::new(limits.connections.get())),
        scope: scope.clone(),
        stop: stop.clone(),
    };
    let router = router.layer(Extension(stop.clone()));
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let span = tracing::Span::current();
    let make = tower::service_fn(move |incoming: IncomingStream<'_, Bounded<L>>| {
        let lease = incoming.io().lease.clone();
        let address = incoming.remote_addr().clone();
        std::future::ready(Ok::<_, Infallible>(ConnectionService {
            router: router.clone(),
            address,
            lease,
            dispatch: dispatch.clone(),
            span: span.clone(),
        }))
    });
    axum::serve(listener, make)
        .with_graceful_shutdown(async move { stop.stopped().await })
        .await?;
    scope.wait().await;
    Ok(())
}
