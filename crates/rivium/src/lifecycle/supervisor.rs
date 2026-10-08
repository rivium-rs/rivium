//! The supervisor: starts the services, sends everything that happens to them through one
//! ordered channel to the state machine, and carries out what the machine decides. Readiness,
//! ends, failures, stop and restart requests and timers are handled one at a time, so the same
//! inputs always end the same way, whatever the scheduling.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tracing::Instrument;

use super::health::{HealthRegistry, Phase, Readiness};
use super::machine::{Effect, Input, State, Timer};
use super::service::{Restarter, Service, ServiceContext, ServiceKind, StopSignal};
use super::tasks::Run;
use crate::BError;

/// Why the host asks a run to stop.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// An operating system signal or console event, with the number of the signal it stands
    /// for: `SIGTERM` is 15 and `SIGINT` 2; on Windows `CTRL_C` and `CTRL_BREAK` count as 2,
    /// `CTRL_CLOSE`, `CTRL_LOGOFF` and `CTRL_SHUTDOWN` as 15.
    Signal {
        /// The name, such as `SIGTERM`.
        name: &'static str,
        /// The number.
        number: u8,
    },
    /// The embedding application stops the host, leaving the services this long at most.
    Host {
        /// The time the services get to stop; zero aborts them at once.
        budget: Duration,
    },
}

impl StopReason {
    /// The time the stop request leaves the services, when it sets one.
    pub(crate) fn budget(&self) -> Option<Duration> {
        match self {
            StopReason::Host { budget } => Some(*budget),
            StopReason::Signal { .. } => None,
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::Signal { name, .. } => f.write_str(name),
            StopReason::Host { .. } => f.write_str("stopped by the host"),
        }
    }
}

/// Creates the way a host stops a run: it keeps the [`StopHandle`] and gives the
/// [`StopReceiver`] to [`Supervisor::run`].
#[must_use]
pub fn stop_channel() -> (StopHandle, StopReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (StopHandle(sender), StopReceiver(receiver))
}

/// Asks a run to stop. Clones ask the same run.
#[derive(Clone, Debug)]
pub struct StopHandle(mpsc::UnboundedSender<StopReason>);

impl StopHandle {
    /// Asks the run to stop: the first request stops it, the second aborts it. Requests after
    /// the run has ended change nothing.
    pub fn stop(&self, reason: StopReason) {
        let _ = self.0.send(reason);
    }
}

/// Where a run receives its stop requests.
#[derive(Debug)]
pub struct StopReceiver(mpsc::UnboundedReceiver<StopReason>);

/// How a run ended.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// Every service finished, or stopped in time after a stop request.
    Stopped,
    /// After a stop request, services were still running at the deadline and were abandoned.
    StopTimedOut,
    /// A service failed before it was ready, or the services were not ready in time.
    StartupFailed(BError),
    /// A service failed after it was ready.
    Fault(BError),
    /// A service asked for a restart, for this reason.
    RestartRequested(Cow<'static, str>),
    /// A second stop request cut the stop short.
    Aborted(StopReason),
}

impl Outcome {
    /// A short name, as logged.
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Outcome::Stopped => "stopped",
            Outcome::StopTimedOut => "stop-timed-out",
            Outcome::StartupFailed(_) => "startup-failed",
            Outcome::Fault(_) => "fault",
            Outcome::RestartRequested(_) => "restart-requested",
            Outcome::Aborted(_) => "aborted",
        }
    }
}

/// Runs services: starts them, waits until each is ready, and stops them on request, on a
/// restart request or when one fails. The hosts build one per run; see the module
/// documentation.
pub struct Supervisor {
    services: Vec<Box<dyn Service>>,
    startup_timeout: Duration,
    stop_timeout: Duration,
    run: Arc<Run>,
    inputs: mpsc::UnboundedReceiver<Input>,
    phase: watch::Sender<Phase>,
    health: HealthRegistry,
}

impl Supervisor {
    /// A supervisor whose services must all be ready within `startup_timeout`, and must stop
    /// within `stop_timeout` of the moment the run starts to stop.
    #[must_use]
    pub fn new(startup_timeout: Duration, stop_timeout: Duration) -> Self {
        let (inputs, received) = mpsc::unbounded_channel();
        Supervisor {
            services: Vec::new(),
            startup_timeout,
            stop_timeout,
            run: Arc::new(Run::new(inputs)),
            inputs: received,
            phase: watch::Sender::new(Phase::Starting),
            health: HealthRegistry::default(),
        }
    }

    /// Adds a service.
    #[must_use]
    pub fn with(mut self, service: Box<dyn Service>) -> Self {
        self.services.push(service);
        self
    }

    /// Runs the services until the run ends, and says how it ended. Services and tasks still
    /// running at the end are aborted, as they are when this future is dropped; blocking tasks
    /// cannot be, and keep running.
    pub async fn run(self, stop: StopReceiver) -> Outcome {
        self.supervise(stop).await.0
    }

    /// The readiness of this supervisor's run.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the hosts use it; they are not written yet")
    )]
    pub(crate) fn readiness(&self) -> Readiness {
        Readiness::new(self.phase.subscribe(), self.health.clone())
    }

    /// The health items of this supervisor's run.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the hosts use it; they are not written yet")
    )]
    pub(crate) fn health(&self) -> HealthRegistry {
        self.health.clone()
    }

    /// Asks this supervisor's run for a restart.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the hosts use it; they are not written yet")
    )]
    pub(crate) fn restarter(&self) -> Restarter {
        Restarter::new(self.run.inputs.clone())
    }

    /// Like [`run`](Self::run), and also says which services and tasks were abandoned.
    pub(crate) async fn supervise(self, stop: StopReceiver) -> (Outcome, Vec<String>) {
        let Supervisor {
            services,
            startup_timeout,
            stop_timeout,
            run,
            mut inputs,
            phase,
            health: _,
        } = self;
        let named: Vec<_> = (services.iter())
            .map(|service| (service.name(), service.kind()))
            .collect();
        let mut state = State::new(named.clone(), startup_timeout, stop_timeout);
        // Dropping the driver aborts every task of the run, also when this future is dropped.
        let mut driver = Driver {
            run: Arc::clone(&run),
            phase,
            stops: Vec::new(),
            deadline: None,
            handles: Vec::new(),
        };
        if let Some(end) = driver.apply(state.start()) {
            return end;
        }
        driver.forward(stop);
        for (index, (service, (name, kind))) in services.into_iter().zip(named).enumerate() {
            let (stop, stopping) = watch::channel(false);
            driver.stops.push((kind, stop));
            let span = tracing::info_span!("service", service.name = %name);
            let signal = StopSignal::new(stopping, run.deadline.subscribe());
            let ctx =
                ServiceContext::new(index, name.clone(), span.clone(), Arc::clone(&run), signal);
            let future = Box::pin(async move { service.run(ctx).await }.instrument(span));
            run.spawn(index, name.into_owned(), None, future);
        }
        while let Some(input) = inputs.recv().await {
            if let Some(end) = driver.apply(state.step(input)) {
                return end;
            }
        }
        unreachable!("the run holds a sender of its inputs")
    }
}

/// Carries out the machine's effects.
struct Driver {
    run: Arc<Run>,
    phase: watch::Sender<Phase>,
    /// Each service's kind and stop flag.
    stops: Vec<(ServiceKind, watch::Sender<bool>)>,
    deadline: Option<Instant>,
    /// The timers and the stop forwarder.
    handles: Vec<AbortHandle>,
}

impl Driver {
    /// Carries out the effects; returns how the run ended once it has.
    fn apply(&mut self, effects: Vec<Effect>) -> Option<(Outcome, Vec<String>)> {
        for effect in effects {
            match effect {
                Effect::Phase(phase, reason) => {
                    self.phase.send_replace(phase);
                    tracing::info!(phase = phase.as_str(), reason = %reason, "phase changed");
                }
                Effect::Cancel(i) => {
                    self.stops[i].1.send_replace(true);
                }
                Effect::StopFrontline => self.stop(ServiceKind::Frontline),
                Effect::StopBackground => self.stop(ServiceKind::Background),
                Effect::StartTimer(timer, after) => self.start_timer(timer, after),
                Effect::Finish(outcome) => return Some(self.finish(outcome)),
            }
        }
        None
    }

    fn stop(&self, kind: ServiceKind) {
        for (_, stop) in self.stops.iter().filter(|(k, _)| *k == kind) {
            stop.send_replace(true);
        }
    }

    fn start_timer(&mut self, timer: Timer, after: Duration) {
        let at = Instant::now() + after;
        if timer == Timer::Deadline {
            // A deadline only ever moves earlier.
            if self.deadline.is_some_and(|deadline| deadline <= at) {
                return;
            }
            self.deadline = Some(at);
            self.run.deadline.send_replace(Some(at));
        }
        let inputs = self.run.inputs.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep_until(at).await;
            let _ = inputs.send(Input::Timer(timer));
        });
        self.handles.push(handle.abort_handle());
    }

    /// Passes the host's stop requests on to the machine.
    fn forward(&mut self, mut stop: StopReceiver) {
        let inputs = self.run.inputs.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(reason) = stop.0.recv().await {
                let _ = inputs.send(Input::Stop(reason));
            }
        });
        self.handles.push(forwarder.abort_handle());
    }

    fn finish(&mut self, outcome: Outcome) -> (Outcome, Vec<String>) {
        let (abandoned, blocking) = self.run.close();
        let name = outcome.name();
        if abandoned.is_empty() {
            tracing::info!(outcome = name, "outcome");
        } else {
            tracing::warn!(outcome = name, abandoned = %abandoned.join(", "), "outcome");
        }
        if !blocking.is_empty() {
            let tasks = blocking.join(", ");
            tracing::warn!(tasks, "blocking tasks cannot be aborted and keep running");
        }
        (outcome, abandoned)
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.run.close();
        self.handles.iter().for_each(AbortHandle::abort);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Outcome, Supervisor, stop_channel};
    use crate::lifecycle::health::{Health, Phase};
    use crate::{ServiceKind, service};

    #[tokio::test(start_paused = true)]
    async fn the_hosts_see_the_readiness_of_a_run_and_can_restart_it() {
        let echo = service("echo", ServiceKind::Frontline, |ctx| async move {
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        });
        let supervisor =
            Supervisor::new(Duration::from_secs(30), Duration::from_secs(3)).with(echo);
        let (readiness, restarter) = (supervisor.readiness(), supervisor.restarter());
        let store = supervisor.health().register("store");
        let (_stop, stops) = stop_channel();
        let run = tokio::spawn(supervisor.run(stops));
        let mut phase = readiness.phase.clone();
        phase
            .wait_for(|phase| *phase == Phase::Running)
            .await
            .unwrap();
        assert!(readiness.is_ready());
        store.set(Health::Unhealthy("disk full".into()));
        assert!(!readiness.is_ready());
        restarter.request("configuration changed");
        let Outcome::RestartRequested(why) = run.await.unwrap() else {
            panic!("not a restart");
        };
        assert_eq!(why, "configuration changed");
        assert_eq!(readiness.phase(), Phase::Stopped);
    }
}
