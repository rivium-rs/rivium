//! Bounded persistent-task reconciliation. A desired key owns one incarnation at a time.

use rivium::{Error, Result, error::kinds, scope::TaskScope};
use std::{
    collections::BTreeMap,
    fmt::{self, Debug},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    task::{JoinError, JoinHandle},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

const ABORT_CONFIRMATION: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct Policy {
    pub capacity: usize,
    pub max_failures: u32,
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub stop_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            capacity: 1024,
            max_failures: 5,
            backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(300),
            stop_timeout: Duration::from_secs(3),
        }
    }
}

impl Policy {
    pub fn delay(&self, failures: u32) -> Duration {
        self.backoff
            .saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
            .min(self.max_backoff)
    }

    fn validate(&self) -> Result<()> {
        let now = Instant::now();
        if self.capacity == 0
            || self.max_failures == 0
            || self.stop_timeout.is_zero()
            || self.backoff > self.max_backoff
            || now.checked_add(self.stop_timeout).is_none()
            || now.checked_add(self.max_backoff).is_none()
        {
            return Err(Error::invalid(
                "task policy requires finite deadlines, positive capacity and failure budget",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct View<S> {
    pub spec: S,
    pub alive: bool,
    pub stopping: bool,
    pub failures: u32,
    pub retry_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan<K> {
    pub stop: Vec<K>,
    pub start: Vec<K>,
    pub limited: Vec<K>,
}

struct Observation<'a, S> {
    spec: &'a S,
    alive: bool,
    stopping: bool,
    failures: u32,
    retry_at: Instant,
}

fn observe<S>(view: &View<S>, alive: bool) -> Observation<'_, S> {
    Observation {
        spec: &view.spec,
        alive,
        stopping: view.stopping,
        failures: view.failures,
        retry_at: view.retry_at,
    }
}

fn scan_current<'a, 'b, K: Ord + 'a, S: Eq + 'a + 'b>(
    desired: impl Fn(&K) -> Option<&'b S>,
    current: impl Iterator<Item = (&'a K, Observation<'a, S>)>,
    mut stop: impl FnMut(&K),
) -> usize {
    let mut retained = 0;
    for (key, state) in current {
        if let Some(spec) = desired(key) {
            retained += 1;
            if spec != state.spec || state.stopping {
                stop(key);
            }
        } else {
            stop(key);
            if state.alive {
                retained += 1;
            }
        }
    }
    retained
}

fn decide_starts<'a, 'b, K: Clone + 'b, S: 'a + 'b>(
    desired: impl Iterator<Item = (&'b K, &'b S)>,
    observe: impl Fn(&K) -> Option<Observation<'a, S>>,
    requires_restart: impl Fn(&Observation<'a, S>, &S) -> bool,
    mut retained: usize,
    now: Instant,
    policy: &Policy,
) -> (Vec<K>, Vec<K>) {
    let mut start = Vec::new();
    let mut limited = Vec::new();
    for (key, spec) in desired {
        match observe(key) {
            Some(state) if requires_restart(&state, spec) => start.push(key.clone()),
            Some(state)
                if !state.alive
                    && state.failures < policy.max_failures
                    && now >= state.retry_at =>
            {
                start.push(key.clone())
            }
            Some(_) => {}
            None if retained < policy.capacity => {
                retained += 1;
                start.push(key.clone());
            }
            None => limited.push(key.clone()),
        }
    }
    (start, limited)
}

/// Pure planning; the caller supplies an explicit monotonic observation time.
pub fn plan<K: Ord + Clone, S: Eq>(
    desired: &BTreeMap<K, S>,
    current: &BTreeMap<K, View<S>>,
    now: Instant,
    policy: &Policy,
) -> Plan<K> {
    let mut stop = Vec::new();
    let retained = scan_current(
        |key| desired.get(key),
        current
            .iter()
            .map(|(key, view)| (key, observe(view, view.alive))),
        |key| stop.push(key.clone()),
    );
    let (start, limited) = decide_starts(
        desired.iter(),
        |key| current.get(key).map(|view| observe(view, view.alive)),
        |state, spec| state.stopping || state.spec != spec,
        retained,
        now,
        policy,
    );
    Plan {
        stop,
        start,
        limited,
    }
}

fn find_spec<'a, K: Ord, S>(desired: &'a [(K, S)], key: &K) -> Option<&'a S> {
    desired
        .binary_search_by(|(candidate, _)| candidate.cmp(key))
        .ok()
        .map(|index| &desired[index].1)
}

#[derive(Clone)]
pub struct Context {
    pub cancel: CancellationToken,
    progress: Arc<Progress>,
}

struct Progress {
    alive: AtomicBool,
    observed: AtomicBool,
}

struct RunningProgress(Arc<Progress>);

impl Drop for RunningProgress {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
    }
}

impl Context {
    pub fn mark_progress(&self) {
        if self.progress.alive.load(Ordering::Acquire) {
            self.progress.observed.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone, Debug)]
pub enum Exit {
    Completed,
    Failed(Error),
    Panicked(Arc<JoinError>),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Open,
    Closed,
}

#[derive(Clone, Debug)]
pub struct Report<K> {
    pub lifecycle: Lifecycle,
    pub started: Vec<K>,
    pub removed: Vec<K>,
    pub restarted: Vec<K>,
    pub exhausted: Vec<K>,
    pub limited: Vec<K>,
    pub unconfirmed: Vec<K>,
    pub exits: Vec<(K, Exit)>,
}

impl<K> Default for Report<K> {
    fn default() -> Self {
        Self {
            lifecycle: Lifecycle::Open,
            started: vec![],
            removed: vec![],
            restarted: vec![],
            exhausted: vec![],
            limited: vec![],
            unconfirmed: vec![],
            exits: vec![],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesiredError {
    DuplicateKey,
    TooMany { requested: usize, capacity: usize },
}

impl fmt::Display for DesiredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateKey => f.write_str("desired keys must be unique"),
            Self::TooMany {
                requested,
                capacity,
            } => write!(f, "desired set of {requested} exceeds capacity {capacity}"),
        }
    }
}

impl std::error::Error for DesiredError {}

#[derive(Debug)]
pub enum ReconcileError {
    InvalidDesired(DesiredError),
    Failed(Error),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDesired(e) => fmt::Display::fmt(e, f),
            Self::Failed(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for ReconcileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::InvalidDesired(e) => e,
            Self::Failed(e) => e,
        })
    }
}

impl From<ReconcileError> for Error {
    fn from(value: ReconcileError) -> Self {
        match value {
            ReconcileError::Failed(error) => error,
            ReconcileError::InvalidDesired(error) => {
                Error::invalid(error.to_string()).with_source(error)
            }
        }
    }
}

struct Entry<S> {
    state: View<S>,
    task: Option<JoinHandle<Result<()>>>,
    cancel: CancellationToken,
    progress: Arc<Progress>,
    exhaustion_reported: bool,
}

/// Owns handles and all tracked keys, including backoff and exhausted records.
pub struct Manager<K: Ord, S> {
    entries: BTreeMap<K, Entry<S>>,
    pending_exits: Vec<(K, Exit)>,
    policy: Policy,
    scope: TaskScope,
    owns_scope: bool,
    closed: bool,
}

impl<K: Ord + Clone + Debug, S: Eq + Clone> Manager<K, S> {
    pub fn new(policy: Policy) -> Result<Self> {
        let mut manager = Self::with_scope(policy, TaskScope::new())?;
        manager.owns_scope = true;
        Ok(manager)
    }

    pub fn with_scope(policy: Policy, scope: TaskScope) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            entries: BTreeMap::new(),
            pending_exits: Vec::new(),
            policy,
            scope,
            owns_scope: false,
            closed: false,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn scan_current(&self, desired: &[(K, S)], stop: impl FnMut(&K)) -> usize {
        scan_current(
            |key| find_spec(desired, key),
            self.entries
                .iter()
                .map(|(key, entry)| (key, observe(&entry.state, entry.task.is_some()))),
            stop,
        )
    }

    fn plan_starts(&self, desired: &[(K, S)], now: Instant) -> (Vec<K>, Vec<K>) {
        let retained = self
            .entries
            .iter()
            .filter(|(key, entry)| find_spec(desired, key).is_some() || entry.task.is_some())
            .count();
        decide_starts(
            desired.iter().map(|(key, spec)| (key, spec)),
            |key| {
                self.entries
                    .get(key)
                    .map(|entry| observe(&entry.state, entry.task.is_some()))
            },
            // Every spec mismatch was marked stopping before awaiting confirmation.
            |state, _| state.stopping,
            retained,
            now,
            &self.policy,
        )
    }

    /// Rejected input leaves existing work intact; accepted rounds may finish after close.
    pub async fn reconcile<F, Fut>(
        &mut self,
        mut desired: Vec<(K, S)>,
        mut factory: F,
    ) -> std::result::Result<Report<K>, ReconcileError>
    where
        K: Send + 'static,
        S: Send + 'static,
        F: FnMut(K, S, Context) -> Fut,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        if desired.len() > self.policy.capacity {
            return Err(ReconcileError::InvalidDesired(DesiredError::TooMany {
                requested: desired.len(),
                capacity: self.policy.capacity,
            }));
        }
        if self.closed {
            return Err(ReconcileError::Failed(Error::new(
                kinds::UNAVAILABLE,
                "task manager has shut down",
            )));
        }
        desired.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if desired
            .windows(2)
            .any(|pair| pair[0].0.cmp(&pair[1].0).is_eq())
        {
            return Err(ReconcileError::InvalidDesired(DesiredError::DuplicateKey));
        }

        let mut report = Report::default();
        self.reap_finished().await;
        if self.entries.len() == desired.len()
            && self
                .entries
                .iter()
                .zip(&desired)
                .all(|((key, entry), (wanted_key, spec))| {
                    key == wanted_key
                        && entry.state.spec == *spec
                        && entry.task.is_some()
                        && !entry.state.stopping
                })
        {
            // Unchanged work still needs admission and delivery of retained exits.
            if self.scope.reserve(0).is_none() {
                report.lifecycle = Lifecycle::Closed;
            }
            report.exits = std::mem::take(&mut self.pending_exits);
            return Ok(report);
        }
        let wanted = desired;
        let mut stop = Vec::new();
        self.scan_current(&wanted, |key| stop.push(key.clone()));

        // Reserve candidates before any stop, then plan against confirmed releases.
        // Healthy unchanged keys need no new reservation.
        let candidates: Vec<_> = {
            let mut stopping = stop.iter().peekable();
            wanted
                .iter()
                .map(|(key, _)| key)
                .filter(|key| {
                    // Both lists are ordered; reuse the stop decision without comparing specs.
                    while stopping
                        .peek()
                        .is_some_and(|stopped| (*stopped).cmp(*key).is_lt())
                    {
                        stopping.next();
                    }
                    stopping
                        .peek()
                        .is_some_and(|stopped| (*stopped).cmp(*key).is_eq())
                        || self.entries.get(*key).is_none_or(|entry| {
                            entry.task.is_none() && entry.state.failures < self.policy.max_failures
                        })
                })
                .collect()
        };
        let reservations = self.scope.reserve(candidates.len());
        let Some(reservations) = reservations else {
            report.lifecycle = Lifecycle::Closed;
            report.exits = std::mem::take(&mut self.pending_exits);
            return Ok(report);
        };
        let mut reservations = candidates.into_iter().zip(reservations);

        self.stop_keys(&stop, &mut report).await;
        // No suspension follows: until here, cancellation must retain every exit.
        report.exits = std::mem::take(&mut self.pending_exits);
        for key in &stop {
            if find_spec(&wanted, key).is_none()
                && self.entries.get(key).is_some_and(|e| e.task.is_none())
            {
                self.entries.remove(key);
                report.removed.push(key.clone());
            }
        }

        // Stops can cross retry deadlines or leave unconfirmed work occupying capacity.
        let now = Instant::now();
        let (start, limited) = self.plan_starts(&wanted, now);
        report.limited = limited;
        let scope_cancel = self.scope.cancellation_token();
        for key in start {
            // Both sequences follow desired-key order; skipped candidates need no start.
            let reservation = reservations
                .by_ref()
                .find_map(|(candidate, reservation)| (candidate == &key).then_some(reservation))
                .expect("admitted candidate must precede mutation");
            if scope_cancel.is_cancelled() || self.closed {
                continue;
            }
            let previous = self.entries.get(&key);
            if previous.is_some_and(|entry| entry.task.is_some()) {
                continue;
            }
            let replacement = previous.is_some();
            if !replacement && self.entries.len() >= self.policy.capacity {
                report.limited.push(key);
                continue;
            }
            let spec = find_spec(&wanted, &key)
                .expect("plan only starts desired keys")
                .clone();
            let failures = previous
                .filter(|e| e.state.spec == spec && !e.state.stopping)
                .map_or(0, |e| e.state.failures);
            let progress = Arc::new(Progress {
                alive: AtomicBool::new(true),
                observed: AtomicBool::new(false),
            });
            let cancel = scope_cancel.child_token();
            let context = Context {
                cancel: cancel.clone(),
                progress: progress.clone(),
            };
            // Factories run only after admission and previous resource release.
            let future = factory(key.clone(), spec.clone(), context);
            let guard = RunningProgress(progress.clone());
            let future = async move {
                let _guard = guard;
                future.await
            };
            #[cfg(feature = "log")]
            let future = {
                use tracing::instrument::WithSubscriber;
                future.with_subscriber(tracing::dispatcher::get_default(Clone::clone))
            };
            let task = tokio::spawn(reservation.track(future));
            self.entries.insert(
                key.clone(),
                Entry {
                    state: View {
                        spec,
                        alive: true,
                        stopping: false,
                        failures,
                        retry_at: now,
                    },
                    task: Some(task),
                    cancel,
                    progress,
                    exhaustion_reported: false,
                },
            );
            if replacement {
                report.restarted.push(key);
            } else {
                report.started.push(key);
            }
        }

        for (key, entry) in &mut self.entries {
            if entry.task.is_none()
                && entry.state.failures >= self.policy.max_failures
                && !entry.exhaustion_reported
            {
                report.exhausted.push(key.clone());
                entry.exhaustion_reported = true;
            }
        }
        Ok(report)
    }

    async fn reap_finished(&mut self) {
        let finished: Vec<_> = self
            .entries
            .iter_mut()
            .filter_map(|(key, entry)| {
                if entry.progress.observed.swap(false, Ordering::AcqRel) {
                    entry.state.failures = 0;
                }
                entry
                    .task
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished)
                    .then(|| key.clone())
            })
            .collect();
        for key in finished {
            let entry = self.entries.get_mut(&key).expect("owned entry");
            let result = entry.task.as_mut().expect("finished handle retained").await;
            entry.task.take();
            Self::record_exit(&self.policy, entry, &key, result, &mut self.pending_exits);
        }
    }

    fn record_exit(
        policy: &Policy,
        entry: &mut Entry<S>,
        key: &K,
        result: std::result::Result<Result<()>, JoinError>,
        exits: &mut Vec<(K, Exit)>,
    ) {
        if entry.progress.observed.swap(false, Ordering::AcqRel) {
            entry.state.failures = 0;
        }
        let exit = match result {
            Ok(Ok(())) => Exit::Completed,
            Ok(Err(error)) => Exit::Failed(error),
            Err(error) if error.is_cancelled() => Exit::Cancelled,
            Err(error) => Exit::Panicked(Arc::new(error)),
        };
        if !entry.state.stopping && matches!(exit, Exit::Failed(_) | Exit::Panicked(_)) {
            entry.state.failures = entry.state.failures.saturating_add(1);
            entry.state.retry_at = Instant::now() + policy.delay(entry.state.failures);
        } else if matches!(exit, Exit::Completed) {
            entry.state.failures = 0;
            entry.state.retry_at = Instant::now();
        }
        entry.state.alive = false;
        exits.push((key.clone(), exit));
    }

    async fn stop_keys(&mut self, keys: &[K], report: &mut Report<K>) {
        for key in keys {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.state.stopping = true;
                entry.cancel.cancel();
            }
        }
        let grace = Instant::now() + self.policy.stop_timeout;
        self.wait_batch(keys, grace).await;
        for key in keys {
            if let Some(handle) = self.entries.get(key).and_then(|e| e.task.as_ref()) {
                handle.abort();
            }
        }
        self.wait_batch(keys, Instant::now() + ABORT_CONFIRMATION)
            .await;
        report.unconfirmed.extend(
            keys.iter()
                .filter(|key| self.entries.get(*key).is_some_and(|e| e.task.is_some()))
                .cloned(),
        );
    }

    async fn wait_batch(&mut self, keys: &[K], deadline: Instant) {
        for key in keys {
            let Some(entry) = self.entries.get_mut(key) else {
                continue;
            };
            let Some(handle) = entry.task.as_mut() else {
                continue;
            };
            if let Ok(result) = tokio::time::timeout_at(deadline, handle).await {
                entry.task.take();
                Self::record_exit(&self.policy, entry, key, result, &mut self.pending_exits);
            }
        }
    }

    /// Closing this manager never closes a borrowed service scope shared with siblings.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.closed = true;
        if self.owns_scope {
            self.scope.cancel();
        }
        let keys: Vec<_> = self.entries.keys().cloned().collect();
        let mut report = Report::default();
        self.stop_keys(&keys, &mut report).await;
        self.pending_exits.clear();
        self.entries.retain(|_, e| e.task.is_some());
        if report.unconfirmed.is_empty() {
            Ok(())
        } else {
            Err(unconfirmed(&report.unconfirmed))
        }
    }
}

impl<K: Ord, S> Drop for Manager<K, S> {
    fn drop(&mut self) {
        if self.owns_scope {
            self.scope.cancel();
        }
        for entry in self.entries.values() {
            entry.cancel.cancel();
            if let Some(handle) = &entry.task {
                handle.abort();
            }
        }
    }
}

fn unconfirmed<K: Debug>(keys: &[K]) -> Error {
    Error::new(
        kinds::TIMEOUT,
        format!("task release unconfirmed: {keys:?}"),
    )
}

#[derive(Clone, Debug)]
pub struct DriveOptions {
    pub list_timeout: Duration,
}

impl Default for DriveOptions {
    fn default() -> Self {
        Self {
            list_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug)]
pub enum DriveRound<K> {
    Reconciled(Report<K>),
    SourceFailed(Error),
    SourceTimedOut { timeout: Duration },
    DesiredRejected(DesiredError),
}

pub trait DesiredSource {
    type Key: Ord + Clone + Debug;
    type Spec: Eq + Clone;

    fn list_desired(&mut self)
    -> impl Future<Output = Result<Vec<(Self::Key, Self::Spec)>>> + Send;

    fn tick_interval(&mut self) -> Duration;
}

pub async fn drive<K, S, F, Fut, L, A, AFut>(
    manager: &mut Manager<K, S>,
    mut source: L,
    mut factory: F,
    mut wake: tokio::sync::watch::Receiver<u64>,
    cancel: CancellationToken,
    options: DriveOptions,
    mut after_round: A,
) -> Result<()>
where
    K: Ord + Clone + Debug + Send + 'static,
    S: Eq + Clone + Send + 'static,
    F: FnMut(K, S, Context) -> Fut,
    Fut: Future<Output = Result<()>> + Send + 'static,
    L: DesiredSource<Key = K, Spec = S>,
    A: FnMut(DriveRound<K>) -> AFut,
    AFut: Future<Output = Result<()>>,
{
    let scoped_stop = manager.scope.cancellation_token();
    let operation: Result<()> = async {
        if options.list_timeout.is_zero() {
            return Err(Error::invalid(
                "desired-state list timeout must be positive"
            ));
        }
        loop {
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = scoped_stop.cancelled() => break,
                result = tokio::time::timeout(options.list_timeout, source.list_desired()) => result,
            };
            match result {
                Ok(Ok(desired)) => match manager.reconcile(desired, &mut factory).await {
                    Ok(report) => {
                        let closed = report.lifecycle == Lifecycle::Closed;
                        let remaining = report.unconfirmed.clone();
                        after_round(DriveRound::Reconciled(report)).await?;
                        if !remaining.is_empty() {
                            return Err(unconfirmed(&remaining));
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(ReconcileError::InvalidDesired(error)) =>
                        after_round(DriveRound::DesiredRejected(error)).await?,
                    Err(ReconcileError::Failed(error)) => return Err(error),
                },
                Ok(Err(error)) => after_round(DriveRound::SourceFailed(error)).await?,
                Err(_) => after_round(DriveRound::SourceTimedOut {
                    timeout: options.list_timeout
                })
                .await?,
            }
            let interval = source.tick_interval();
            let Some(deadline) = Instant::now()
                .checked_add(interval)
                .filter(|_| !interval.is_zero())
            else {
                return Err(Error::invalid(
                    "desired-state tick interval must be finite and positive"
                ));
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = scoped_stop.cancelled() => break,
                _ = tokio::time::sleep_until(deadline) => {},
                changed = wake.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    wake.borrow_and_update();
                },
            }
        }
        Ok(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    match (operation, cleanup) {
        (Ok(()), result) => result,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => {
            let message = format!("{error}; manager shutdown failed: {cleanup}");
            Err(error.context(message))
        }
    }
}
