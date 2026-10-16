//! Bounded service supervision with separate outcome and drain reports.
use crate::{
    Error, Result,
    error::kinds,
    scope::{Drain, Reservation, TaskScope},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, mpsc, watch},
    task::{Id, JoinSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, instrument::WithSubscriber};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Maximum accepted service roots and children per supervisor.
pub const TASK_CAPACITY: usize = 4096;
/// Maximum queued task registrations.
pub const COMMAND_CAPACITY: usize = 4096;
/// Whole-batch confirmation after forced abort.
pub const ABORT_CONFIRMATION: Duration = Duration::from_millis(100);
/// Boxed asynchronous service result.
pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;

/// Service shutdown tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Reject external work and drain before background dependencies stop.
    Frontline,
    /// Stay available until frontline execution resources have drained.
    Background,
}
/// Public lifecycle phase; stopped does not imply resource release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Initializing services.
    Starting,
    /// Every root has declared readiness.
    Running,
    /// Admission is closing and work is draining.
    Stopping,
    /// Supervision has returned.
    Stopped,
}
/// A fixed health check's current state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Health {
    /// No healthy observation has been recorded.
    Unknown,
    /// The dependency can fulfill its role.
    Healthy,
    /// Private diagnostic explanation for an unhealthy dependency.
    Unhealthy(String),
}
/// Request that initiated shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// Ordinary stop.
    Requested,
    /// Operating-system signal number.
    Signal(i32),
    /// Stop using at most this remaining service grace.
    Deadline(Duration),
}
/// Why the supervised run ended, independent of resource release.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Normal stop and service work drained.
    Stopped,
    /// Requested generation replacement.
    Restart(String),
    /// Validation or initialization failed.
    StartupFailed(Error),
    /// A running service failed.
    Failed(Error),
    /// A second stop forced abortion; the value is the signal, or zero.
    Aborted(i32),
    /// The shared grace expired, even if subsequent abortion released resources.
    TimedOut {
        /// Names of work pending at the grace deadline.
        tasks: Vec<String>,
        /// Subset backed by blocking closures.
        blocking: Vec<String>,
    },
}
/// Return-time lifecycle evidence.
#[derive(Clone, Debug)]
pub struct RunReport {
    /// Why supervision ended.
    pub outcome: Outcome,
    /// Whether all registered execution bodies have released captures.
    pub drain: Drain,
}
/// Synchronous child-admission failure. Conversion to business errors is explicit.
///
/// ```compile_fail
/// fn cannot_hide_closed_admission() -> rivium::Result<()> {
///     Err(rivium::SubmitError::Closed)?;
///     Ok(())
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitError {
    /// This service scope is permanently closed.
    Closed,
    /// Queue or managed execution budget is full.
    Overloaded,
    /// A child name is empty.
    InvalidName,
    /// This service already owns an unfinished child with this name.
    DuplicateName,
}
impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SubmitError {}

/// A service root owned by a supervisor.
pub trait Service: Send + 'static {
    /// Nonempty, run-unique root name.
    fn name(&self) -> &str;
    /// Dependency shutdown tier.
    fn kind(&self) -> Kind;
    /// Construct and run the service inside the supervisor's panic boundary.
    fn run(self: Box<Self>, context: Context) -> ServiceFuture;
}
struct ClosureService<F> {
    name: String,
    kind: Kind,
    factory: F,
}
impl<F, Fut> Service for ClosureService<F>
where
    F: FnOnce(Context) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn run(self: Box<Self>, context: Context) -> ServiceFuture {
        Box::pin((self.factory)(context))
    }
}
/// Turn a one-shot service factory into a supervised root.
pub fn service<F, Fut>(name: impl Into<String>, kind: Kind, factory: F) -> Box<dyn Service>
where
    F: FnOnce(Context) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Box::new(ClosureService {
        name: name.into(),
        kind,
        factory,
    })
}

struct Status {
    phase: Phase,
    health: BTreeMap<String, Health>,
    first_stop: Option<StopReason>,
    second_stop: Option<StopReason>,
    restart: Option<String>,
    deadline: Option<Instant>,
}
struct Shared {
    status: Mutex<Status>,
    changed: watch::Sender<u64>,
    wake: Arc<Notify>,
    roots: TaskScope,
    scopes: Mutex<Vec<TaskScope>>,
    grace: Duration,
}
impl Shared {
    fn signal(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
        self.wake.notify_waiters();
    }
    fn phase(&self) -> Phase {
        lock(&self.status).phase
    }
    fn set_phase(&self, phase: Phase) {
        let changed = {
            let mut state = lock(&self.status);
            let changed = state.phase != phase;
            state.phase = phase;
            changed
        };
        if changed {
            tracing::info!(?phase, "phase changed");
            self.signal();
        }
    }
    fn close_scopes(&self, cancel: bool) {
        let scopes = lock(&self.scopes).clone();
        for scope in scopes {
            if cancel {
                scope.cancel();
            } else {
                scope.close();
            }
        }
    }
    fn active(&self) -> usize {
        self.roots.active_tasks()
            + lock(&self.scopes)
                .iter()
                .map(TaskScope::active_tasks)
                .sum::<usize>()
    }
    fn drain_state(&self) -> Drain {
        // Observe permanent closure before counts: otherwise an intervening final admission
        // could make a previously observed zero look drained after the scope closes.
        let closed = self.roots.is_closed() && lock(&self.scopes).iter().all(TaskScope::is_closed);
        let active = self.active();
        if closed && active == 0 {
            Drain::Complete
        } else {
            Drain::Incomplete {
                active_tasks: active,
            }
        }
    }
}
/// Cloneable observation handle bound to one run.
#[derive(Clone)]
pub struct Readiness {
    shared: Arc<Shared>,
    changes: watch::Receiver<u64>,
}
impl Readiness {
    /// Current public phase.
    pub fn phase(&self) -> Phase {
        self.shared.phase()
    }
    /// Ready only while running, healthy, and without a queued stop/restart.
    pub fn is_ready(&self) -> bool {
        let state = lock(&self.shared.status);
        state.phase == Phase::Running
            && state.first_stop.is_none()
            && state.restart.is_none()
            && state.health.values().all(|v| matches!(v, Health::Healthy))
    }
    /// Wait for a phase or health change. None means supervision is finished.
    pub async fn changed(&mut self) -> Option<Phase> {
        if self.phase() == Phase::Stopped {
            return None;
        }
        self.changes.changed().await.ok().map(|()| self.phase())
    }
    /// Register fixed health names during startup.
    pub fn health(&self) -> HealthRegistry {
        HealthRegistry {
            shared: self.shared.clone(),
        }
    }
    /// Accepted roots, children, and independently reserved scope executions.
    pub fn active_tasks(&self) -> usize {
        self.shared.active()
    }
    /// Await actual execution-resource release without sending stop.
    pub async fn drain(&self, within: Duration) -> Drain {
        let wait = async {
            loop {
                let notified = self.shared.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.shared.drain_state().is_complete() {
                    return;
                }
                notified.await;
            }
        };
        if !within.is_zero() && !self.shared.drain_state().is_complete() {
            let _ = tokio::time::timeout(within, wait).await;
        }
        self.shared.drain_state()
    }
}
/// Run-owned health registry bounded to 4096 fixed names.
#[derive(Clone)]
pub struct HealthRegistry {
    shared: Arc<Shared>,
}
impl HealthRegistry {
    /// Add a unique 1..=128 byte name while the run is starting.
    pub fn register(&self, name: impl Into<String>) -> Result<HealthHandle> {
        let name = name.into();
        if name.is_empty() || name.len() > 128 {
            return Err(Error::invalid("health name must contain 1..=128 bytes"));
        }
        {
            let mut state = lock(&self.shared.status);
            if state.phase != Phase::Starting || state.health.contains_key(&name) {
                return Err(Error::new(
                    kinds::CONFLICT,
                    "health registration is closed or duplicate",
                ));
            }
            if state.health.len() >= TASK_CAPACITY {
                return Err(Error::new(
                    kinds::TOO_MANY_REQUESTS,
                    "health registry is full",
                ));
            }
            state.health.insert(name.clone(), Health::Unknown);
        }
        self.shared.signal();
        Ok(HealthHandle {
            shared: self.shared.clone(),
            name,
        })
    }
    /// Snapshot the fixed checks in deterministic name order.
    pub fn snapshot(&self) -> BTreeMap<String, Health> {
        lock(&self.shared.status).health.clone()
    }
}
/// Update handle for one registered health check.
#[derive(Clone)]
pub struct HealthHandle {
    shared: Arc<Shared>,
    name: String,
}
impl HealthHandle {
    /// Replace the check's state and wake readiness observers.
    pub fn set(&self, health: Health) {
        lock(&self.shared.status)
            .health
            .insert(self.name.clone(), health);
        self.shared.signal();
    }
    /// Read the check's last state.
    pub fn get(&self) -> Health {
        lock(&self.shared.status).health[&self.name].clone()
    }
}
/// Bounded stop/restart mailbox for one run.
#[derive(Clone)]
pub struct Control {
    shared: Arc<Shared>,
}
impl Control {
    /// First request starts shutdown; a second forces abort. Later requests coalesce.
    pub fn stop(&self, reason: StopReason) {
        {
            let mut state = lock(&self.shared.status);
            if state.phase == Phase::Stopped {
                return;
            }
            if state.first_stop.is_none() {
                state.first_stop = Some(reason);
            } else if state.second_stop.is_none() {
                state.second_stop = Some(reason);
            }
            let within = match reason {
                StopReason::Deadline(within) => within.min(self.shared.grace),
                _ => self.shared.grace,
            };
            if let Some(deadline) = Instant::now().checked_add(within) {
                state.deadline = Some(
                    state
                        .deadline
                        .map_or(deadline, |existing| existing.min(deadline)),
                );
            }
        }
        self.shared.signal();
    }
    /// Request one generation replacement, unless stop takes precedence.
    pub fn restart(&self, reason: impl Into<String>) {
        let mut reason = reason.into();
        if reason.len() > 1024 {
            let mut end = 1024;
            while !reason.is_char_boundary(end) {
                end -= 1;
            }
            reason.truncate(end);
        }
        {
            let mut state = lock(&self.shared.status);
            if state.phase == Phase::Stopped
                || state.restart.is_some()
                || state.first_stop.is_some()
            {
                return;
            }
            state.restart = Some(reason);
            if state.deadline.is_none() {
                state.deadline = Instant::now().checked_add(self.shared.grace);
            }
        }
        self.shared.signal();
    }
}
/// Service-local cooperative stop and global shutdown deadline.
#[derive(Clone)]
pub struct Stop {
    token: CancellationToken,
    shared: Arc<Shared>,
}
impl Stop {
    /// Await cancellation of this service's shutdown tier.
    pub async fn stopped(&self) {
        self.token.cancelled().await;
    }
    /// Whether this service has been asked to stop.
    pub fn is_stopping(&self) -> bool {
        self.token.is_cancelled()
    }
    /// The one shared shutdown deadline, absent before a stop request/failure.
    pub fn deadline(&self) -> Option<Instant> {
        lock(&self.shared.status).deadline
    }
    /// Clone the cooperative cancellation token.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }
}

#[derive(Default)]
struct Admissions {
    active: usize,
    children: BTreeSet<(usize, String)>,
}
struct Admission {
    state: Mutex<Admissions>,
}
struct Permit {
    owner: Arc<Admission>,
    key: Option<(usize, String)>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = lock(&self.owner.state);
        state.active -= 1;
        if let Some(key) = &self.key {
            state.children.remove(key);
        }
    }
}
enum Body {
    Async(ServiceFuture),
    Blocking(Box<dyn FnOnce() -> Result<()> + Send>),
}
struct Command {
    body: Body,
    permit: Permit,
    reservation: Reservation,
    service: usize,
    name: String,
}

/// Owned access to readiness, shutdown, and bounded child admission.
#[derive(Clone)]
pub struct Context {
    index: usize,
    name: Arc<str>,
    kind: Kind,
    ready: Arc<AtomicBool>,
    scope: TaskScope,
    shared: Arc<Shared>,
    admission: Arc<Admission>,
    commands: mpsc::Sender<Command>,
}
impl Context {
    /// Root name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Root shutdown tier.
    pub fn kind(&self) -> Kind {
        self.kind
    }
    /// Declare successful initialization; repeated declarations are harmless.
    pub fn ready(&self) {
        self.ready.store(true, Ordering::Release);
        self.shared.signal();
    }
    /// Run control mailbox.
    pub fn control(&self) -> Control {
        Control {
            shared: self.shared.clone(),
        }
    }
    /// Run readiness observer.
    pub fn readiness(&self) -> Readiness {
        Readiness {
            shared: self.shared.clone(),
            changes: self.shared.changed.subscribe(),
        }
    }
    /// Cancellation handle for this service tier.
    pub fn stop(&self) -> Stop {
        Stop {
            token: self.scope.cancellation_token(),
            shared: self.shared.clone(),
        }
    }
    /// Wait until this service tier is stopping.
    pub async fn stopped(&self) {
        self.scope.cancellation_token().cancelled().await;
    }
    /// Whether this service tier is stopping.
    pub fn is_stopping(&self) -> bool {
        self.scope.cancellation_token().is_cancelled()
    }
    /// The shared absolute shutdown deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.stop().deadline()
    }
    /// Scope for independently owned work; it excludes this service root.
    pub fn task_scope(&self) -> TaskScope {
        self.scope.clone()
    }
    /// Accept an asynchronous child, or synchronously release it on rejection.
    pub fn spawn(
        &self,
        name: impl Into<String>,
        future: impl Future<Output = Result<()>> + Send + 'static,
    ) -> std::result::Result<(), SubmitError> {
        self.submit(name.into(), Body::Async(Box::pin(future)))
    }
    /// Accept a blocking child. A started closure cannot be forcibly killed.
    pub fn spawn_blocking(
        &self,
        name: impl Into<String>,
        work: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> std::result::Result<(), SubmitError> {
        self.submit(name.into(), Body::Blocking(Box::new(work)))
    }
    fn submit(&self, name: String, body: Body) -> std::result::Result<(), SubmitError> {
        if name.is_empty() {
            return Err(SubmitError::InvalidName);
        }
        if self.scope.is_closed() {
            return Err(SubmitError::Closed);
        }
        let queue = self.commands.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => SubmitError::Overloaded,
            mpsc::error::TrySendError::Closed(_) => SubmitError::Closed,
        })?;
        let (permit, reservation) = {
            let mut state = lock(&self.admission.state);
            if state.children.contains(&(self.index, name.clone())) {
                return Err(SubmitError::DuplicateName);
            }
            if state.active >= TASK_CAPACITY {
                return Err(SubmitError::Overloaded);
            }
            let reservation = self
                .scope
                .reserve(1)
                .and_then(|mut entries| entries.pop())
                .ok_or(SubmitError::Closed)?;
            state.active += 1;
            state.children.insert((self.index, name.clone()));
            (
                Permit {
                    owner: self.admission.clone(),
                    key: Some((self.index, name.clone())),
                },
                reservation,
            )
        };
        queue.send(Command {
            body,
            permit,
            reservation,
            service: self.index,
            name,
        });
        Ok(())
    }
}
struct PreparedService {
    service: Box<dyn Service>,
    name: String,
    kind: Kind,
}
struct Running {
    name: String,
    kind: Kind,
    ready: Arc<AtomicBool>,
    scope: TaskScope,
    root_live: bool,
}
struct TaskInfo {
    service: usize,
    name: String,
    root: bool,
    blocking: bool,
}

/// Owns root services and all children accepted through their contexts.
pub struct Supervisor {
    startup: Duration,
    services: Vec<Box<dyn Service>>,
    shared: Arc<Shared>,
    admission: Arc<Admission>,
    sender: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
    tasks: JoinSet<Result<()>>,
    names: HashMap<Id, TaskInfo>,
    running: Vec<Running>,
}
impl Default for Supervisor {
    fn default() -> Self {
        Self::new(Duration::from_secs(30), Duration::from_secs(3))
    }
}
impl Supervisor {
    /// Construct a run with positive startup and service grace budgets.
    pub fn new(startup: Duration, grace: Duration) -> Self {
        let wake = Arc::new(Notify::new());
        let (changed, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                phase: Phase::Starting,
                health: BTreeMap::new(),
                first_stop: None,
                second_stop: None,
                restart: None,
                deadline: None,
            }),
            changed,
            wake: wake.clone(),
            roots: TaskScope::with_notify(wake),
            scopes: Mutex::new(Vec::new()),
            grace,
        });
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        Self {
            startup,
            services: Vec::new(),
            shared,
            admission: Arc::new(Admission {
                state: Mutex::new(Admissions::default()),
            }),
            sender,
            receiver,
            tasks: JoinSet::new(),
            names: HashMap::new(),
            running: Vec::new(),
        }
    }
    /// Add a root; the complete set is validated before any factory is called.
    pub fn with(mut self, service: Box<dyn Service>) -> Self {
        self.services.push(service);
        self
    }
    /// Observe this run before starting it.
    pub fn readiness(&self) -> Readiness {
        Readiness {
            shared: self.shared.clone(),
            changes: self.shared.changed.subscribe(),
        }
    }
    /// Control this run before or during execution.
    pub fn control(&self) -> Control {
        Control {
            shared: self.shared.clone(),
        }
    }
    fn prepare(&mut self) -> Result<Vec<PreparedService>> {
        let now = Instant::now();
        if self.startup.is_zero()
            || self.shared.grace.is_zero()
            || now.checked_add(self.startup).is_none()
            || now.checked_add(self.shared.grace).is_none()
        {
            return Err(Error::invalid(
                "startup and grace must be positive, representable deadlines",
            ));
        }
        if self.services.is_empty() || self.services.len() > TASK_CAPACITY {
            return Err(Error::invalid("supervisor requires 1..=4096 services"));
        }
        let mut names = BTreeSet::new();
        let mut prepared = Vec::with_capacity(self.services.len());
        for service in std::mem::take(&mut self.services) {
            let (name, kind) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (service.name().to_owned(), service.kind())
            }))
            .map_err(|_| Error::internal("service metadata panicked"))?;
            if name.is_empty() || !names.insert(name.clone()) {
                return Err(Error::invalid("service names must be nonempty and unique"));
            }
            prepared.push(PreparedService {
                service,
                name,
                kind,
            });
        }
        Ok(prepared)
    }
    fn launch(&mut self, services: Vec<PreparedService>) {
        lock(&self.admission.state).active = services.len();
        for (
            index,
            PreparedService {
                service,
                name,
                kind,
            },
        ) in services.into_iter().enumerate()
        {
            let ready = Arc::new(AtomicBool::new(false));
            let scope = TaskScope::with_notify(self.shared.wake.clone());
            lock(&self.shared.scopes).push(scope.clone());
            let context = Context {
                index,
                name: name.clone().into(),
                kind,
                ready: ready.clone(),
                scope: scope.clone(),
                shared: self.shared.clone(),
                admission: self.admission.clone(),
                commands: self.sender.clone(),
            };
            let permit = Permit {
                owner: self.admission.clone(),
                key: None,
            };
            let reservation = self.shared.roots.reserve(1).unwrap().pop().unwrap();
            let span = tracing::info_span!("service", service = %name);
            let handle = self.tasks.spawn(
                reservation.track(
                    async move {
                        let _permit = permit;
                        service.run(context).await
                    }
                    .instrument(span)
                    .with_current_subscriber(),
                ),
            );
            self.names.insert(
                handle.id(),
                TaskInfo {
                    service: index,
                    name: name.clone(),
                    root: true,
                    blocking: false,
                },
            );
            self.running.push(Running {
                name,
                kind,
                ready,
                scope,
                root_live: true,
            });
        }
    }
    fn accept(&mut self, command: Command) {
        if self.running[command.service]
            .scope
            .cancellation_token()
            .is_cancelled()
        {
            return;
        }
        let Command {
            body,
            permit,
            reservation,
            service,
            name,
        } = command;
        let blocking = matches!(&body, Body::Blocking(_));
        let span = tracing::info_span!(
            "service_task",
            service = %self.running[service].name,
            task_key = %name
        );
        let handle = match body {
            Body::Async(future) => self.tasks.spawn(
                reservation.track(
                    async move {
                        let _permit = permit;
                        future.await
                    }
                    .instrument(span)
                    .with_current_subscriber(),
                ),
            ),
            Body::Blocking(work) => {
                let dispatch = tracing::dispatcher::get_default(Clone::clone);
                self.tasks.spawn_blocking(move || {
                    reservation.run(move || {
                        let _permit = permit;
                        tracing::dispatcher::with_default(&dispatch, || {
                            let _entered = span.enter();
                            work()
                        })
                    })
                })
            }
        };
        self.names.insert(
            handle.id(),
            TaskInfo {
                service,
                name: format!("{}/{name}", self.running[service].name),
                root: false,
                blocking,
            },
        );
    }
    fn begin_stop(&self, kind: Option<Kind>) {
        {
            let mut state = lock(&self.shared.status);
            if state.deadline.is_none() {
                let now = Instant::now();
                state.deadline = Some(now.checked_add(self.shared.grace).unwrap_or(now));
            }
        }
        self.shared.set_phase(Phase::Stopping);
        self.shared.roots.close();
        for service in &self.running {
            if kind.is_none() || Some(service.kind) == kind {
                service.scope.cancel();
            }
        }
        self.shared.signal();
    }
    fn front_drained(&self) -> bool {
        self.running
            .iter()
            .filter(|r| r.kind == Kind::Frontline)
            .all(|r| !r.root_live && r.scope.active_tasks() == 0)
    }
    fn timeout_outcome(&self) -> Outcome {
        let mut tasks: BTreeSet<_> = self.names.values().map(|task| task.name.clone()).collect();
        let blocking: BTreeSet<_> = self
            .names
            .values()
            .filter(|task| task.blocking)
            .map(|task| task.name.clone())
            .collect();
        for (index, service) in self.running.iter().enumerate() {
            let managed = self
                .names
                .values()
                .filter(|task| task.service == index && !task.root)
                .count();
            if service.scope.active_tasks() > managed {
                tasks.insert(format!("{}/<scope>", service.name));
            }
        }
        Outcome::TimedOut {
            tasks: tasks.into_iter().collect(),
            blocking: blocking.into_iter().collect(),
        }
    }
    fn finish(&mut self, outcome: Outcome) -> RunReport {
        self.receiver.close();
        while self.receiver.try_recv().is_ok() {}
        self.shared.roots.close();
        self.shared.close_scopes(false);
        self.shared.set_phase(Phase::Stopped);
        let report = RunReport {
            outcome,
            drain: self.shared.drain_state(),
        };
        tracing::info!(
            outcome = ?report.outcome,
            drain = ?report.drain,
            active_tasks = self.shared.active(),
            "run finished"
        );
        report
    }
    /// Drive one generation through startup, execution, and bounded shutdown.
    pub async fn run(mut self) -> RunReport {
        let services = match self.prepare() {
            Ok(services) => services,
            Err(error) => return self.finish(Outcome::StartupFailed(error)),
        };
        tracing::info!(phase = ?Phase::Starting, "phase changed");
        let Some(startup_deadline) = Instant::now().checked_add(self.startup) else {
            return self.finish(Outcome::StartupFailed(Error::invalid(
                "startup deadline is not representable",
            )));
        };
        self.launch(services);
        let mut outcome: Option<Outcome> = None;
        let mut stopping_back = false;
        let mut abort_deadline = None;
        loop {
            let wake = self.shared.wake.clone();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if outcome.is_none()
                && self.shared.phase() == Phase::Starting
                && self.running.iter().all(|r| r.ready.load(Ordering::Acquire))
            {
                self.shared.set_phase(Phase::Running);
            }
            let (first, second, restart, deadline) = {
                let state = lock(&self.shared.status);
                (
                    state.first_stop,
                    state.second_stop,
                    state.restart.clone(),
                    state.deadline,
                )
            };
            if let Some(second) = second {
                if abort_deadline.is_none() {
                    outcome = Some(Outcome::Aborted(match second {
                        StopReason::Signal(signal) => signal,
                        _ => 0,
                    }));
                    self.begin_stop(None);
                    self.receiver.close();
                    while self.receiver.try_recv().is_ok() {}
                    self.tasks.abort_all();
                    abort_deadline = Some(Instant::now() + ABORT_CONFIRMATION);
                }
            } else if first.is_some() && matches!(outcome, None | Some(Outcome::Restart(_))) {
                outcome = Some(Outcome::Stopped);
                self.begin_stop(Some(Kind::Frontline));
            } else if outcome.is_none()
                && let Some(reason) = restart
            {
                outcome = Some(Outcome::Restart(reason));
                self.begin_stop(Some(Kind::Frontline));
            }
            if outcome.is_none()
                && self.receiver.is_empty()
                && self.running.iter().all(|r| !r.root_live)
            {
                outcome = Some(Outcome::Stopped);
                self.begin_stop(Some(Kind::Frontline));
            }
            if outcome.is_some() && !stopping_back && self.front_drained() {
                stopping_back = true;
                self.begin_stop(Some(Kind::Background));
            }
            if self.tasks.is_empty()
                && self.shared.drain_state().is_complete()
                && let Some(outcome) = outcome.take()
            {
                return self.finish(outcome);
            }
            if let Some(abort) = abort_deadline {
                if Instant::now() >= abort {
                    return self.finish(outcome.unwrap());
                }
            } else if outcome.is_some() && deadline.is_some_and(|end| Instant::now() >= end) {
                let timeout = self.timeout_outcome();
                if !matches!(
                    outcome,
                    Some(Outcome::StartupFailed(_) | Outcome::Failed(_))
                ) {
                    outcome = Some(timeout);
                }
                self.begin_stop(None);
                self.receiver.close();
                while self.receiver.try_recv().is_ok() {}
                self.tasks.abort_all();
                abort_deadline = Some(Instant::now() + ABORT_CONFIRMATION);
                continue;
            }
            let timer = abort_deadline.or_else(|| {
                if outcome.is_some() {
                    lock(&self.shared.status).deadline
                } else if self.shared.phase() == Phase::Starting {
                    Some(startup_deadline)
                } else {
                    None
                }
            });
            tokio::select! {
                biased;
                _ = notified => {},
                completed = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    if let Some(completed) = completed {
                        let (id, result) = match completed {
                            Ok((id, result)) => (id, result),
                            Err(error) => (
                                error.id(),
                                Err(Error::internal(if error.is_panic() {
                                    "service task panicked"
                                } else {
                                    "service task was aborted"
                                }))
                            )
                        };
                        if let Some(info) = self.names.remove(&id) {
                            let service = &mut self.running[info.service];
                            if info.root {
                                service.root_live = false;
                                service.scope.close();
                            }
                            if abort_deadline.is_none() {
                                let failure = match result {
                                    Err(error) => Some(error.context(format!(
                                        "service task {}",
                                        info.name
                                    ))),
                                    Ok(()) if info.root
                                        && outcome.is_none()
                                        && !service.ready.load(Ordering::Acquire) =>
                                        Some(Error::internal(format!(
                                            "{} ended before declaring ready",
                                            info.name
                                        ))),
                                    Ok(()) if info.root
                                        && service.kind == Kind::Frontline
                                        && outcome.is_none() =>
                                        Some(Error::internal(format!(
                                            "frontline {} ended unexpectedly",
                                            info.name
                                        ))),
                                    _ => None,
                                };
                                if let Some(error) = failure
                                    && !matches!(
                                        outcome,
                                        Some(Outcome::StartupFailed(_) | Outcome::Failed(_))
                                    )
                                {
                                    outcome = Some(if self.shared.phase() == Phase::Starting
                                        && !service.ready.load(Ordering::Acquire)
                                    {
                                        Outcome::StartupFailed(error)
                                    } else {
                                        Outcome::Failed(error)
                                    });
                                    self.begin_stop(Some(Kind::Frontline));
                                }
                            }
                        }
                    }
                },
                command = self.receiver.recv(),
                    if !self.receiver.is_closed() || !self.receiver.is_empty() =>
                {
                    if let Some(command) = command {
                        self.accept(command);
                    }
                },
                _ = async {
                    match timer {
                        Some(end) => tokio::time::sleep_until(end).await,
                        None => std::future::pending().await
                    }
                } => {
                    if outcome.is_none() && self.shared.phase() == Phase::Starting {
                        outcome = Some(Outcome::StartupFailed(Error::new(
                            kinds::TIMEOUT,
                            "service startup deadline exceeded"
                        )));
                        self.begin_stop(Some(Kind::Frontline));
                    }
                }
            }
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shared.roots.close();
        self.shared.close_scopes(true);
        self.tasks.abort_all();
        if std::thread::panicking() {
            // A failing subscriber must not be invoked again during unwinding.
            lock(&self.shared.status).phase = Phase::Stopped;
            self.shared.signal();
        } else {
            self.shared.set_phase(Phase::Stopped);
        }
    }
}
/// Construct a background service that ticks immediately, then skips missed intervals.
/// A running tick must cooperate with cancellation itself; stop is checked between ticks.
pub fn periodic<F, Fut>(name: impl Into<String>, every: Duration, mut tick: F) -> Box<dyn Service>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    service(name, Kind::Background, move |context| async move {
        if every.is_zero() || Instant::now().checked_add(every).is_none() {
            return Err(Error::invalid(
                "periodic interval must be positive and representable",
            ));
        }
        let mut timer = tokio::time::interval(every);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        context.ready();
        loop {
            tokio::select! {
                biased;
                _ = context.stopped() => return Ok(()),
                _ = timer.tick() => tick().await?
            }
        }
    })
}
