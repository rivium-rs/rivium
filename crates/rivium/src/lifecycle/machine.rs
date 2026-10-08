//! The supervisor's decisions as a state machine: one input in, the effects out. Nothing here
//! waits, spawns or reads a clock; time arrives as timer inputs, so every cell of the rule table
//! is tested directly. The machine logs failures itself, where it decides whether a failure is
//! why the run stops (an error) or happened while it was already stopping for another failure
//! (a warning).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::time::Duration;

use rivium_error::{BError, Class, Error, ErrorKind, ErrorType, log_error};
use tracing::Level;

use super::health::Phase;
use super::service::ServiceKind;
use super::supervisor::{Outcome, StopReason};
use crate::config::de::format_duration;

/// A service returned before it was ready, or a frontline service before it was asked to stop.
const SERVICE_EXITED: ErrorType = &ErrorKind::new("ServiceExited", Class::Internal);
/// A service or one of its tasks panicked.
const SERVICE_PANICKED: ErrorType = &ErrorKind::new("ServicePanicked", Class::Internal);
/// Not every service was ready within the startup timeout.
const STARTUP_TIMED_OUT: ErrorType = &ErrorKind::new("StartupTimedOut", Class::Timeout);
/// The services cannot run: there are none, or two share a name.
const INVALID_SERVICES: ErrorType = &ErrorKind::new("InvalidServices", Class::Internal);

/// Something that happened during a run.
#[derive(Debug)]
pub(super) enum Input {
    /// Service `i` called `ready()`.
    Ready(usize),
    /// The run future of service `i` ended; tasks it spawned may still run.
    Ended(usize, Exit),
    /// The named task of service `i` failed or panicked.
    ChildFailed(usize, Cow<'static, str>, BError),
    /// The run future of service `i` and every task it spawned have ended.
    Drained(usize),
    /// The host asks the run to stop.
    Stop(StopReason),
    /// A service asks for a restart.
    Restart(Cow<'static, str>),
    /// A timer the machine started has fired.
    Timer(Timer),
}

/// How the run future of a service ended.
#[derive(Debug)]
pub(super) enum Exit {
    Returned,
    Failed(BError),
    Panicked(String),
}

/// The timers of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Timer {
    /// Every service must be ready when it fires.
    Startup,
    /// Services still running when it fires are abandoned.
    Deadline,
}

/// What the driver does for the machine, in order.
#[derive(Debug)]
pub(super) enum Effect {
    /// The phase changed, for this reason.
    Phase(Phase, Cow<'static, str>),
    /// Ask service `i` and its tasks to stop now, whatever the others do.
    Cancel(usize),
    /// Ask every frontline service to stop.
    StopFrontline,
    /// Ask every background service to stop.
    StopBackground,
    /// Start a timer that fires after this long. A deadline only ever moves earlier: the driver
    /// keeps the earlier of two.
    StartTimer(Timer, Duration),
    /// The run is over; the machine handles no input after this.
    Finish(Outcome),
}

/// Why a run is stopping. The first reason stays, except that a failure turns a stop or a
/// restart into a fault, and the host's stop wins over a restart.
#[derive(Debug)]
enum Reason {
    /// A service failed before it was ready, or the services were not ready in time.
    Startup(BError),
    /// A service failed after it was ready.
    Fault(BError),
    /// The host asked the run to stop.
    Stop(StopReason),
    /// A service asked for a restart.
    Restart(Cow<'static, str>),
}

/// What the machine knows of one service.
#[derive(Debug)]
struct Entry {
    name: Cow<'static, str>,
    kind: ServiceKind,
    ready: bool,
    /// Its run future or one of its tasks still runs.
    alive: bool,
    /// It has failed; later failures of the same service change nothing.
    failed: bool,
}

/// The state of one run.
#[derive(Debug)]
pub(super) struct State {
    services: Vec<Entry>,
    phase: Phase,
    reason: Option<Reason>,
    /// The host's stop requests so far: the second one aborts the run.
    stops: usize,
    background_stopped: bool,
    startup_timeout: Duration,
    stop_timeout: Duration,
}

impl State {
    /// A run of these services, none of them ready yet.
    pub(super) fn new(
        services: Vec<(Cow<'static, str>, ServiceKind)>,
        startup_timeout: Duration,
        stop_timeout: Duration,
    ) -> Self {
        let services = (services.into_iter())
            .map(|(name, kind)| Entry {
                name,
                kind,
                ready: false,
                alive: true,
                failed: false,
            })
            .collect();
        State {
            services,
            phase: Phase::Starting,
            reason: None,
            stops: 0,
            background_stopped: false,
            startup_timeout,
            stop_timeout,
        }
    }

    /// Starts the run: the startup timer, or the end at once when the services cannot run, in
    /// which case the driver starts none of them.
    pub(super) fn start(&mut self) -> Vec<Effect> {
        let mut out = vec![Effect::Phase(Phase::Starting, "the run started".into())];
        match self.invalid() {
            None => out.push(Effect::StartTimer(Timer::Startup, self.startup_timeout)),
            Some(error) => {
                log_error!(Level::ERROR, &error, "startup failed");
                self.services
                    .iter_mut()
                    .for_each(|service| service.alive = false);
                self.reason = Some(Reason::Startup(error));
                self.finish(false, &mut out);
            }
        }
        out
    }

    /// Handles one input and says what to do.
    pub(super) fn step(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.phase == Phase::Stopped {
            return out;
        }
        match input {
            Input::Ready(i) if self.phase == Phase::Starting => {
                self.services[i].ready = true;
                self.enter_running_if_ready(&mut out);
            }
            Input::Ready(_) => {}
            Input::Ended(i, Exit::Returned) => self.returned(i, &mut out),
            Input::Ended(i, Exit::Failed(error)) => self.fail(i, None, error, &mut out),
            Input::Ended(i, Exit::Panicked(message)) => {
                self.fail(i, None, panicked(&message), &mut out);
            }
            Input::ChildFailed(i, task, error) => self.fail(i, Some(task), error, &mut out),
            Input::Drained(i) => self.drained(i, &mut out),
            Input::Stop(stop) => self.stop(stop, &mut out),
            Input::Restart(why) if self.phase != Phase::Stopping => {
                let shown = format!("restart requested: {why}").into();
                self.begin_stopping(Reason::Restart(why), shown, &mut out);
                self.advance(&mut out);
            }
            Input::Restart(_) => {}
            Input::Timer(Timer::Startup) => self.startup_timed_out(&mut out),
            Input::Timer(Timer::Deadline) if self.phase == Phase::Stopping => {
                self.finish(true, &mut out);
            }
            Input::Timer(Timer::Deadline) => {
                // Cannot occur: the deadline starts when the run starts to stop.
                let phase = self.phase.as_str();
                tracing::error!(
                    phase,
                    "a deadline fired before the run was stopping; ignored"
                );
            }
        }
        out
    }

    /// The run future of service `i` returned `Ok`: a failure while the run starts or runs,
    /// unless the service is a ready background one.
    fn returned(&mut self, i: usize, out: &mut Vec<Effect>) {
        let service = &self.services[i];
        let early = match (service.ready, service.kind) {
            (false, _) => Some("returned before it was ready"),
            (true, ServiceKind::Frontline) => Some("returned before it was asked to stop"),
            (true, ServiceKind::Background) => None,
        };
        match (self.phase, early) {
            (Phase::Starting | Phase::Running, Some(why)) => {
                self.fail(i, None, Error::explain(SERVICE_EXITED, why), out);
            }
            (Phase::Starting, None) => self.enter_running_if_ready(out),
            _ => {}
        }
    }

    /// Service `i`, or one of its tasks, failed. Only its first failure counts.
    fn fail(
        &mut self,
        i: usize,
        task: Option<Cow<'static, str>>,
        error: BError,
        out: &mut Vec<Effect>,
    ) {
        let service = &mut self.services[i];
        if std::mem::replace(&mut service.failed, true) {
            return;
        }
        let (name, ready) = (service.name.clone(), service.ready);
        // The first failure is why the run stops, and so is a failure while it stops for a
        // stop or restart request; a failure after another failure is a secondary one.
        let primary = self.phase != Phase::Stopping
            || matches!(self.reason, Some(Reason::Stop(_) | Reason::Restart(_)));
        let task_name = task.as_deref();
        if primary {
            log_error!(
                Level::ERROR, &error,
                service.name = %name, task.name = task_name, "service failed"
            );
        } else {
            log_error!(
                Level::WARN, &error,
                service.name = %name, task.name = task_name,
                "service failed while the run was stopping"
            );
        }
        let place = match &task {
            Some(task) => format!("service {name}, task {task}"),
            None => format!("service {name}"),
        };
        // The outcome says where the error came from.
        let error = error.more_context(place.clone());
        match self.phase {
            // Decided by the service's own readiness, never by how far the others got, so that
            // the outcome does not depend on scheduling.
            Phase::Starting | Phase::Running => {
                let reason = if ready {
                    Reason::Fault(error)
                } else {
                    Reason::Startup(error)
                };
                self.begin_stopping(reason, format!("{place} failed").into(), out);
            }
            _ if primary => self.reason = Some(Reason::Fault(error)),
            _ => {}
        }
        out.push(Effect::Cancel(i));
        self.advance(out);
    }

    fn drained(&mut self, i: usize, out: &mut Vec<Effect>) {
        self.services[i].alive = false;
        match self.phase {
            Phase::Stopping => self.advance(out),
            // Every service finished on its own.
            _ if self.services.iter().all(|service| !service.alive) => self.finish(false, out),
            _ => {}
        }
    }

    /// A stop request from the host: the first stops the run, the second aborts it.
    fn stop(&mut self, stop: StopReason, out: &mut Vec<Effect>) {
        self.stops += 1;
        if self.stops > 1 {
            self.phase = Phase::Stopped;
            out.push(Effect::Phase(Phase::Stopped, "aborted".into()));
            out.push(Effect::Finish(Outcome::Aborted(stop)));
            return;
        }
        if self.phase != Phase::Stopping {
            let why = stop.to_string().into();
            self.begin_stopping(Reason::Stop(stop), why, out);
            self.advance(out);
            return;
        }
        // Already stopping for a failure or a restart: the stop's budget may bring the deadline
        // forward, and the operator's stop wins over a restart.
        if let Some(budget) = stop.budget() {
            let after = budget.min(self.stop_timeout);
            out.push(Effect::StartTimer(Timer::Deadline, after));
        }
        if matches!(self.reason, Some(Reason::Restart(_))) {
            self.reason = Some(Reason::Stop(stop));
        }
    }

    fn startup_timed_out(&mut self, out: &mut Vec<Effect>) {
        if self.phase != Phase::Starting {
            return;
        }
        let waiting: Vec<&str> = (self.services.iter())
            .filter(|service| !service.ready)
            .map(|service| service.name.as_ref())
            .collect();
        let timeout = format_duration(self.startup_timeout);
        let why = format!("not ready within {timeout}: {}", waiting.join(", "));
        let error = Error::explain(STARTUP_TIMED_OUT, why.clone());
        log_error!(Level::ERROR, &error, "startup failed");
        self.begin_stopping(Reason::Startup(error), why.into(), out);
        self.advance(out);
    }

    fn enter_running_if_ready(&mut self, out: &mut Vec<Effect>) {
        if self.phase == Phase::Starting && self.services.iter().all(|service| service.ready) {
            self.phase = Phase::Running;
            out.push(Effect::Phase(
                Phase::Running,
                "every service is ready".into(),
            ));
        }
    }

    /// The run's first reason to stop, shown as `why`: from now on, services are asked to stop
    /// by one deadline, the frontline services first.
    fn begin_stopping(&mut self, reason: Reason, why: Cow<'static, str>, out: &mut Vec<Effect>) {
        let budget = match &reason {
            Reason::Stop(stop) => stop.budget(),
            _ => None,
        };
        self.reason = Some(reason);
        self.phase = Phase::Stopping;
        out.push(Effect::Phase(Phase::Stopping, why));
        let after = budget.map_or(self.stop_timeout, |budget| budget.min(self.stop_timeout));
        out.push(Effect::StartTimer(Timer::Deadline, after));
        out.push(Effect::StopFrontline);
    }

    /// While stopping: background services stop once no frontline service runs, and the run is
    /// over once nothing runs.
    fn advance(&mut self, out: &mut Vec<Effect>) {
        if self.phase != Phase::Stopping {
            return;
        }
        let alive = |kind| (self.services.iter()).any(|s| s.alive && s.kind == kind);
        if !self.background_stopped && !alive(ServiceKind::Frontline) {
            self.background_stopped = true;
            out.push(Effect::StopBackground);
        }
        if self.services.iter().all(|service| !service.alive) {
            self.finish(false, out);
        }
    }

    /// Ends the run with the outcome its reason gives, `timed_out` when services are still
    /// running at the deadline.
    fn finish(&mut self, timed_out: bool, out: &mut Vec<Effect>) {
        let outcome = match self.reason.take() {
            None => Outcome::Stopped,
            Some(Reason::Stop(_)) if timed_out => Outcome::StopTimedOut,
            Some(Reason::Stop(_)) => Outcome::Stopped,
            Some(Reason::Restart(why)) => Outcome::RestartRequested(why),
            Some(Reason::Startup(error)) => Outcome::StartupFailed(error),
            Some(Reason::Fault(error)) => Outcome::Fault(error),
        };
        self.phase = Phase::Stopped;
        out.push(Effect::Phase(Phase::Stopped, outcome.name().into()));
        out.push(Effect::Finish(outcome));
    }

    /// Why the services cannot run, if they cannot.
    fn invalid(&self) -> Option<BError> {
        if self.services.is_empty() {
            return Some(Error::explain(INVALID_SERVICES, "there are no services"));
        }
        let mut names = BTreeSet::new();
        let twice = (self.services.iter()).find(|service| !names.insert(&service.name))?;
        let why = format!("two services are named {}", twice.name);
        Some(Error::explain(INVALID_SERVICES, why))
    }
}

/// The error of a service or task that panicked with this message.
pub(super) fn panicked(message: &str) -> BError {
    Error::explain(SERVICE_PANICKED, format!("panicked: {message}"))
}

#[cfg(test)]
mod tests;
