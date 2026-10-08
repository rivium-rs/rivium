//! The service contract: what a service is, and what it gets from the supervisor while it runs.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use rivium_error::{Error, kinds};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{Instrument, Span};

use super::machine::Input;
use super::tasks::Run;
use crate::{BoxFuture, Result};

/// A long-running unit of work that the supervisor starts, watches and stops: a server, a
/// poller, a queue consumer. [`service`] and [`periodic`] build one from a closure.
///
/// Building a service only builds it; everything that waits, such as connecting, belongs in
/// [`run`](Service::run), before `ready()`. A failure before `ready()` fails the startup; after
/// it, it is a fault, and either way the whole run stops.
pub trait Service: Send + 'static {
    /// The name, in logs and in the names of its tasks; unique within a run.
    fn name(&self) -> Cow<'static, str>;

    /// How the supervisor treats the service: background unless it takes traffic from outside.
    fn kind(&self) -> ServiceKind {
        ServiceKind::Background
    }

    /// Runs the service: calls `ctx.ready()` once it can do its work, and returns once
    /// `ctx.stopped()` completes, by `ctx.deadline()`.
    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>>;
}

/// How the supervisor treats a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// Takes traffic from outside, such as a server: asked to stop first, and failed when it
    /// returns before that, even with `Ok`.
    Frontline,
    /// Works in the background: asked to stop once no frontline service runs. Returning `Ok`
    /// once ready is a normal end.
    Background,
}

/// A service named `name` that runs the closure.
///
/// ```
/// use rivium::ServiceKind;
///
/// let consumer = rivium::service("consumer", ServiceKind::Background, |ctx| async move {
///     ctx.ready();
///     ctx.stopped().await;
///     Ok(())
/// });
/// assert_eq!(consumer.name(), "consumer");
/// ```
pub fn service<F, Fut>(
    name: impl Into<Cow<'static, str>>,
    kind: ServiceKind,
    run: F,
) -> Box<dyn Service>
where
    F: FnOnce(ServiceContext) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Box::new(FnService {
        name: name.into(),
        kind,
        run,
    })
}

struct FnService<F> {
    name: Cow<'static, str>,
    kind: ServiceKind,
    run: F,
}

impl<F, Fut> Service for FnService<F>
where
    F: FnOnce(ServiceContext) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn name(&self) -> Cow<'static, str> {
        self.name.clone()
    }

    fn kind(&self) -> ServiceKind {
        self.kind
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin((self.run)(ctx))
    }
}

/// A background service that is ready at once and calls `tick` every `every`, the first time
/// at once. Ticks that fall due while one is still running are skipped: the next tick is the
/// next one on the schedule. Each tick has a span of its own. A tick that returns an error
/// fails the service, so a tick handles the errors that the next tick may not see again. When
/// the service is asked to stop, the tick in progress is dropped at its next `.await`: a tick
/// that must finish its work belongs in a [`service`] that watches `ctx.deadline()`.
///
/// ```
/// use std::time::Duration;
///
/// let poller = rivium::periodic("poller", Duration::from_secs(5), || async {
///     tracing::debug!("polling");
///     Ok(())
/// });
/// assert_eq!(poller.name(), "poller");
/// ```
pub fn periodic<F, Fut>(
    name: impl Into<Cow<'static, str>>,
    every: Duration,
    mut tick: F,
) -> Box<dyn Service>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    service(name, ServiceKind::Background, move |ctx| async move {
        if every.is_zero() {
            return Error::e_explain(kinds::INVALID_INPUT, "the period is zero");
        }
        ctx.ready();
        let mut stopped = pin!(ctx.stopped());
        let (start, mut wait) = (Instant::now(), Duration::ZERO);
        loop {
            tokio::select! {
                biased;
                () = &mut stopped => return Ok(()),
                () = tokio::time::sleep(wait) => {}
            }
            tokio::select! {
                biased;
                () = &mut stopped => return Ok(()),
                result = tick().instrument(tracing::info_span!("tick")) => result?,
            }
            // Until the next time on the schedule that is still ahead.
            let late = start.elapsed().as_nanos() % every.as_nanos();
            wait =
                every - Duration::new((late / 1_000_000_000) as u64, (late % 1_000_000_000) as u32);
        }
    })
}

/// What a running service gets from the supervisor: readiness, the stop request and its
/// deadline, tasks of its own, and restart requests.
pub struct ServiceContext {
    index: usize,
    name: Cow<'static, str>,
    span: Span,
    run: Arc<Run>,
    stop: StopSignal,
}

impl ServiceContext {
    pub(super) fn new(
        index: usize,
        name: Cow<'static, str>,
        span: Span,
        run: Arc<Run>,
        stop: StopSignal,
    ) -> Self {
        ServiceContext {
            index,
            name,
            span,
            run,
            stop,
        }
    }

    /// Declares the service ready. Calling it again changes nothing.
    pub fn ready(&self) {
        // Sending fails only once the run is over.
        let _ = self.run.inputs.send(Input::Ready(self.index));
    }

    /// Logs the standard `listening` event, with the fields `service.name` and `listen.addr`:
    /// call it once the service is bound, so operators and test tools learn the address.
    pub fn listening(&self, addr: SocketAddr) {
        tracing::info!(service.name = %self.name, listen.addr = %addr, "listening");
    }

    /// Completes once the service is asked to stop, or the run has ended. The future owns what
    /// it needs, so it can be handed to a server's graceful shutdown.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        self.stop.stopped()
    }

    /// The stop request, for code deep inside the service.
    #[must_use]
    pub fn stop_signal(&self) -> StopSignal {
        self.stop.clone()
    }

    /// Whether the service has been asked to stop, or the run has ended.
    #[must_use]
    pub fn is_stopping(&self) -> bool {
        self.stop.is_stopping()
    }

    /// When the run is stopping, the moment by which the service must have ended: whatever
    /// still runs then is abandoned. Plan the last work with it, such as
    /// `tokio::time::timeout_at(deadline, flush())`. A run whose future is dropped before it
    /// stops sets none.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.stop.deadline()
    }

    /// Runs a task that belongs to the service: it has a span with `task.name`, shares the
    /// service's stop request, and its failure or panic fails the service at once. The
    /// service has ended only when its run future and every task it spawned have ended.
    pub fn spawn<F>(&self, name: impl Into<Cow<'static, str>>, task: F)
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let (path, name, span) = self.task(name.into());
        let task = Box::pin(task.instrument(span));
        self.run.spawn(self.index, path, Some(name), task);
    }

    /// Runs blocking work as a task of the service, as [`spawn`](Self::spawn) does for async
    /// work. Nothing can abort blocking work: give each blocking call a timeout no longer than
    /// the stop timeout, and check [`StopSignal::is_stopping`] between calls.
    pub fn spawn_blocking<F>(&self, name: impl Into<Cow<'static, str>>, task: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let (path, name, span) = self.task(name.into());
        self.run
            .spawn_blocking(self.index, path, (name, span), task);
    }

    /// Asks for a restart of the run.
    #[must_use]
    pub fn restarter(&self) -> Restarter {
        Restarter::new(self.run.inputs.clone())
    }

    fn task(&self, name: Cow<'static, str>) -> (String, Cow<'static, str>, Span) {
        let span = tracing::info_span!(parent: &self.span, "task", task.name = %name);
        (format!("{}/{name}", self.name), name, span)
    }
}

impl fmt::Debug for ServiceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceContext")
            .field("name", &self.name)
            .field("stop", &self.stop)
            .finish_non_exhaustive()
    }
}

/// Whether, and by when, a service is to stop. Clones are cheap.
#[derive(Clone, Debug)]
pub struct StopSignal {
    stopping: watch::Receiver<bool>,
    deadline: watch::Receiver<Option<Instant>>,
}

impl StopSignal {
    pub(super) fn new(
        stopping: watch::Receiver<bool>,
        deadline: watch::Receiver<Option<Instant>>,
    ) -> Self {
        StopSignal { stopping, deadline }
    }

    /// Whether the service has been asked to stop, or the run has ended: a run can end
    /// before every service is asked, at the deadline or at a second stop request.
    #[must_use]
    pub fn is_stopping(&self) -> bool {
        // As in `stopped`: a closed channel means the run is gone.
        *self.stopping.borrow() || self.stopping.has_changed().is_err()
    }

    /// Completes once the service is asked to stop, or the run has ended.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut stopping = self.stopping.clone();
        async move {
            // An error means the run is gone, which is a reason to stop as well.
            let _ = stopping.wait_for(|stopping| *stopping).await;
        }
    }

    /// When the run is stopping, the moment by which the service must have ended. A run whose
    /// future is dropped before it stops sets none.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        *self.deadline.borrow()
    }
}

/// Asks for a restart: the run stops as for a stop request, and the host then starts the
/// services again, in the same process or by exiting with code 75 (`lifecycle.restart`).
/// Clones ask the same run.
#[derive(Clone, Debug)]
pub struct Restarter {
    inputs: mpsc::UnboundedSender<Input>,
}

impl Restarter {
    pub(super) fn new(inputs: mpsc::UnboundedSender<Input>) -> Self {
        Restarter { inputs }
    }

    /// Asks for a restart, for this reason, which is logged. Requests after the run started
    /// to stop change nothing.
    pub fn request(&self, reason: impl Into<Cow<'static, str>>) {
        // Sending fails only once the run is over.
        let _ = self.inputs.send(Input::Restart(reason.into()));
    }
}
