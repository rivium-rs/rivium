#[cfg(feature = "process")]
use crate::Action;
use crate::{
    App, CleanupFailure, CleanupReport, CleanupStage, Code, Context, HostReport, Options,
    PendingResources, config, log, reload,
};
use rivium::{
    Control, Error, Outcome, Phase, Readiness, Result, StopReason, Supervisor, error::Class,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tracing::instrument::WithSubscriber;

pub(crate) static RESTARTS: AtomicU64 = AtomicU64::new(0);
pub(crate) static FAULTS: AtomicU64 = AtomicU64::new(0);
static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    Exit,
    InProcess,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Lifecycle {
    #[serde(with = "duration")]
    pub startup_timeout: Duration,
    #[serde(with = "duration")]
    pub stop_timeout: Duration,
    pub restart: Restart,
    pub instance: String,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(30),
            stop_timeout: Duration::from_secs(3),
            restart: Restart::Exit,
            instance: String::new(),
        }
    }
}

impl Lifecycle {
    fn validate(&self) -> Result<()> {
        if self.startup_timeout.is_zero()
            || self.stop_timeout.is_zero()
            || Instant::now().checked_add(self.startup_timeout).is_none()
            || Instant::now().checked_add(self.stop_timeout).is_none()
        {
            return Err(Error::invalid(
                "lifecycle timeouts must be positive and finite",
            ));
        }
        Ok(())
    }
}

mod duration {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::time::Duration;

    pub(super) fn serialize<S: Serializer>(
        value: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{}ms", value.as_millis()))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Duration, D::Error> {
        let value = String::deserialize(deserializer)?;
        let (number, unit) = ["ms", "s", "m", "h"]
            .into_iter()
            .find_map(|suffix| value.strip_suffix(suffix).map(|number| (number, suffix)))
            .ok_or_else(|| D::Error::custom("expected a duration such as 250ms, 3s, 5m or 1h"))?;
        let factor = match unit {
            "ms" => 1,
            "s" => 1000,
            "m" => 60_000,
            _ => 3_600_000,
        };
        let millis = number
            .parse::<u64>()
            .ok()
            .and_then(|value| value.checked_mul(factor))
            .ok_or_else(|| D::Error::custom("invalid duration or duration overflow"))?;
        Ok(Duration::from_millis(millis))
    }
}

pub(crate) struct Loaded<C> {
    pub config: C,
    pub log: log::Settings,
    pub lifecycle: Lifecycle,
    pub all: toml::Value,
}

fn diagnostic(error: config::Report) -> Error {
    Error::new(error.kind(), error.to_string()).with_source(error)
}

fn serialize(value: &impl Serialize) -> Result<toml::Value> {
    toml::Value::try_from(value).map_err(|e| Error::invalid(e.to_string()).with_source(e))
}

fn application(config: &impl Serialize) -> Result<toml::Table> {
    let toml::Value::Table(table) = serialize(config)? else {
        return Err(Error::invalid("application configuration must be a table"));
    };
    if table.contains_key("log") || table.contains_key("lifecycle") {
        return Err(Error::invalid("reserved log/lifecycle section"));
    }
    Ok(table)
}

pub(crate) fn defaults<A: App>() -> Result<toml::Value> {
    let mut table = application(&A::Config::default())?;
    table.insert("log".into(), serialize(&log::Settings::for_app(A::NAME))?);
    table.insert("lifecycle".into(), serialize(&Lifecycle::default())?);
    Ok(toml::Value::Table(table))
}

pub(crate) fn load<A: App>(
    options: &Options,
    text: Option<&str>,
    require_file: bool,
) -> Result<Loaded<A::Config>> {
    if text.is_none()
        && (require_file || options.paths.explicit_config())
        && !options.paths.config_file().is_file()
    {
        return Err(Error::invalid("configuration file is missing"));
    }

    let loader = config::Loader::new(&defaults::<A>()?)
        .map_err(diagnostic)?
        .file(options.paths.config_file())
        .environment(&options.prefix, options.environment.clone())
        .overrides(options.sets.clone());
    let mut document = if text.is_some() {
        loader.load_text(text)
    } else {
        loader.load()
    }
    .map_err(diagnostic)?;
    let log: log::Settings = document.section("log").map_err(diagnostic)?;
    log.validate(A::NAME)?;
    let lifecycle: Lifecycle = document.section("lifecycle").map_err(diagnostic)?;
    lifecycle.validate()?;
    let all = document.value.clone();
    let application = document.value.as_table_mut().expect("loader creates table");
    application.remove("log");
    application.remove("lifecycle");
    let config = document.decode::<A::Config>().map_err(diagnostic)?;
    A::validate(&config)?;
    // Compare normalized values so equivalent durations and defaults are no-ops.
    let mut all = all;
    replace_application(&mut all, &config)?;
    let table = all.as_table_mut().unwrap();
    table.insert("log".into(), serialize(&log)?);
    table.insert("lifecycle".into(), serialize(&lifecycle)?);
    Ok(Loaded {
        config,
        log,
        lifecycle,
        all,
    })
}

pub(crate) fn replace_application(all: &mut toml::Value, config: &impl Serialize) -> Result<()> {
    let app = application(config)?;
    let table = all
        .as_table_mut()
        .ok_or_else(|| Error::invalid("configuration must be a table"))?;
    table.retain(|key, _| key == "log" || key == "lifecycle");
    table.extend(app);
    Ok(())
}

pub(crate) fn differences(left: &toml::Value, right: &toml::Value) -> Vec<String> {
    fn equivalent(left: &toml::Value, right: &toml::Value) -> bool {
        match (left, right) {
            (toml::Value::Float(left), toml::Value::Float(right)) => {
                left == right || (left.is_nan() && right.is_nan())
            }
            (toml::Value::Array(left), toml::Value::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| equivalent(left, right))
            }
            (toml::Value::Table(left), toml::Value::Table(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.get(key).is_some_and(|right| equivalent(value, right))
                    })
            }
            _ => left == right,
        }
    }

    fn walk(
        left: Option<&toml::Value>,
        right: Option<&toml::Value>,
        path: &str,
        out: &mut Vec<String>,
    ) {
        if matches!((left,right),(Some(left),Some(right)) if equivalent(left,right))
            || (left.is_none() && right.is_none())
        {
            return;
        }
        match (
            left.and_then(toml::Value::as_table),
            right.and_then(toml::Value::as_table),
        ) {
            (Some(left), Some(right)) => {
                for key in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(left.get(key), right.get(key), &child, out);
                }
            }
            _ => out.push(path.into()),
        }
    }

    let mut result = Vec::new();
    walk(Some(left), Some(right), "", &mut result);
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
#[cfg_attr(not(feature = "embedded"), allow(unreachable_pub))]
pub enum Status {
    Idle = 0,
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Restarting = 4,
    Failed = 5,
}

#[derive(Clone, Default)]
struct Resources {
    readiness: Option<Readiness>,
    control: Option<Control>,
    reload: Option<reload::Requester>,
    logger: Option<Arc<log::Logger>>,
    stops: [Option<(StopReason, Instant)>; 2],
}

pub(crate) struct Generation {
    resources: Mutex<Resources>,
    gate: Arc<Mutex<bool>>,
    closed: AtomicBool,
    failures: Mutex<Vec<CleanupFailure>>,
    pub completed: Notify,
}

impl Generation {
    fn new() -> Self {
        Self {
            resources: Mutex::new(Resources::default()),
            gate: Arc::new(Mutex::new(true)),
            closed: AtomicBool::new(false),
            failures: Mutex::new(Vec::new()),
            completed: Notify::new(),
        }
    }

    fn resources(&self) -> Resources {
        crate::lock(&self.resources).clone()
    }

    fn stop(&self, reason: StopReason) {
        let control = {
            let mut resources = crate::lock(&self.resources);
            if resources.control.is_none()
                && let Some(slot) = resources.stops.iter_mut().find(|slot| slot.is_none())
            {
                *slot = Some((reason, Instant::now()));
            }
            resources.control.clone()
        };
        if let Some(control) = control {
            control.stop(reason);
        }
    }

    fn install_control(&self, control: Control) {
        let mut delivered = 0;
        loop {
            let pending = {
                let mut resources = crate::lock(&self.resources);
                if delivered >= 2 || resources.stops[0].is_none() {
                    resources.stops = [None, None];
                    resources.control = Some(control);
                    return;
                }
                std::mem::take(&mut resources.stops)
            };
            for (reason, received) in pending.into_iter().flatten() {
                control.stop(match reason {
                    StopReason::Deadline(within) => {
                        StopReason::Deadline(within.saturating_sub(received.elapsed()))
                    }
                    reason => reason,
                });
                delivered += 1;
            }
        }
    }

    #[cfg(feature = "embedded")]
    pub(crate) fn reload(&self) -> Option<reload::Requester> {
        self.resources().reload
    }

    pub(crate) fn cleanup(&self) -> CleanupReport {
        let closed = self.closed.load(Ordering::Acquire);
        let resources = self.resources();
        // A zero owner count synchronizes with its terminal result; sampling it
        // first is conservative when completion races this snapshot.
        let pending = PendingResources {
            service_tasks: resources
                .readiness
                .as_ref()
                .map_or(0, Readiness::active_tasks),
            reload_preparing: resources
                .reload
                .as_ref()
                .map_or(0, reload::Requester::pending),
            log_workers: resources
                .logger
                .as_ref()
                .map_or(0, |logger| logger.pending_workers()),
        };
        if let Some(error) = resources.reload.and_then(|reload| reload.failure()) {
            self.failure(CleanupStage::Reload, error);
        }
        if (closed || pending.log_workers == 0)
            && let Some(logger) = resources.logger
            && let Err(error) = logger.close(Duration::ZERO)
        {
            self.failure(CleanupStage::Logger, error);
        }
        CleanupReport {
            closed,
            pending,
            failures: crate::lock(&self.failures).clone(),
        }
    }

    fn failure(&self, stage: CleanupStage, error: Error) {
        if error.class() == Class::Timeout {
            return;
        }

        let mut failures = crate::lock(&self.failures);
        if !failures.iter().any(|failure| failure.stage == stage) && failures.len() < 4 {
            failures.push(CleanupFailure { stage, error });
        }
    }
}

pub(crate) struct State {
    pub status: Status,
    pub first: Option<Code>,
    pub finished: bool,
    pub requested: bool,
    pub deadline: Option<Instant>,
    pub report: Option<HostReport>,
    pub generation: Arc<Generation>,
}

pub(crate) struct Run {
    pub state: Mutex<State>,
    pub changed: Condvar,
    pub notify: Notify,
}

impl Run {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                status: Status::Starting,
                first: None,
                finished: false,
                requested: false,
                deadline: None,
                report: None,
                generation: Arc::new(Generation::new()),
            }),
            changed: Condvar::new(),
            notify: Notify::new(),
        }
    }

    pub(crate) fn wake(&self) {
        self.changed.notify_all();
        self.notify.notify_waiters();
    }

    pub(crate) fn generation(&self) -> Arc<Generation> {
        crate::lock(&self.state).generation.clone()
    }

    fn replace_generation(&self) -> Option<Arc<Generation>> {
        let generation = Arc::new(Generation::new());
        let old = {
            let mut state = crate::lock(&self.state);
            if state.requested {
                return None;
            }
            state.status = Status::Starting;
            std::mem::replace(&mut state.generation, generation.clone())
        };
        drop(old);
        self.wake();
        Some(generation)
    }

    pub(crate) fn cleanup(&self) -> CleanupReport {
        self.generation().cleanup()
    }

    pub(crate) fn report(&self) -> Option<HostReport> {
        let (report, finished) = {
            let state = crate::lock(&self.state);
            (state.report.clone(), state.finished)
        };
        report.map(|mut report| {
            if finished {
                report.cleanup = self.cleanup();
            }
            report
        })
    }

    pub(crate) fn stop(&self, reason: StopReason, budget: Option<Duration>) {
        let generation = {
            let mut state = crate::lock(&self.state);
            if state.finished {
                return;
            }
            state.requested = true;
            state.status = Status::Stopping;
            if let Some(budget) = budget {
                let deadline = Instant::now()
                    .checked_add(budget)
                    .unwrap_or_else(Instant::now);
                state.deadline = Some(state.deadline.map_or(deadline, |old| old.min(deadline)));
            }
            state.generation.clone()
        };
        *crate::lock(&generation.gate) = false;
        let resources = generation.resources();
        if let Some(reload) = resources.reload {
            reload.close();
        }
        generation.stop(reason);

        self.wake();
    }

    pub(crate) fn finish(
        &self,
        mut code: Code,
        primary: Option<Error>,
        report: Option<rivium::RunReport>,
    ) {
        let generation = self.generation();
        generation.closed.store(true, Ordering::Release);
        let mut cleanup = self.cleanup();
        cleanup.closed = true;
        if !matches!(
            code,
            Code::Aborted(_)
                | Code::Config
                | Code::StartupFailed
                | Code::Panicked
                | Code::Fault
                | Code::OsError
                | Code::CantCreate
        ) {
            if !cleanup.is_drained() {
                code = Code::StopTimedOut;
            } else if !cleanup.failures.is_empty() {
                code = Code::Fault;
            }
        } else if !cleanup.is_drained() && primary.is_none() && !matches!(code, Code::Aborted(_)) {
            code = Code::StopTimedOut;
        }

        let previous = {
            let mut state = crate::lock(&self.state);
            state.finished = true;
            state.status = if matches!(code, Code::Ok | Code::Cancelled) {
                Status::Idle
            } else {
                Status::Failed
            };
            if state.first.is_none() {
                state.first = Some(if state.requested && code == Code::Ok {
                    Code::Cancelled
                } else {
                    code
                });
            }
            state.report.replace(HostReport {
                code,
                primary,
                run: report,
                cleanup,
            })
        };
        drop(previous);
        self.wake();
        generation.completed.notify_waiters();
    }
}

#[cfg(feature = "process")]
pub(crate) fn inspect<A: App>(options: &Options) -> Option<Code> {
    if options.action == Action::Run {
        return None;
    }

    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        match options.action {
            Action::Version => println!("{} {}", A::NAME, A::VERSION),
            Action::Help => println!(
                "{} [run|check-config|default-config] [--root DIR] [--config FILE] [--set KEY=VALUE]",
                A::NAME
            ),
            Action::DefaultConfig => println!(
                "{}",
                toml::to_string_pretty(&defaults::<A>()?)
                    .map_err(|error| Error::invalid(error.to_string()))?
            ),
            Action::CheckConfig => {
                load::<A>(options, None, false)?;
                println!("configuration valid");
            }
            Action::Run => {}
        }
        Ok(())
    }));
    Some(match result {
        Ok(Ok(())) => Code::Ok,
        Ok(Err(_)) => Code::Config,
        Err(_) => Code::Panicked,
    })
}

#[derive(Default)]
struct RetryState {
    ever_ready: bool,
    failures: u32,
}

impl RetryState {
    fn observed_running(&mut self, elapsed: Duration) {
        self.ever_ready = true;
        if elapsed >= Duration::from_secs(600) {
            self.failures = 0;
        }
    }

    fn failed(&mut self) -> Option<Duration> {
        if !self.ever_ready {
            return None;
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 10 {
            return None;
        }
        Some(
            Duration::from_millis(100_u64.saturating_mul(1_u64 << (self.failures - 1)))
                .min(Duration::from_secs(30)),
        )
    }
}

fn park_retry(
    runtime: &tokio::runtime::Runtime,
    run: &Run,
    generation: &Generation,
    report: HostReport,
    delay: Duration,
) -> bool {
    let previous = {
        let mut state = crate::lock(&run.state);
        if state.requested {
            return false;
        }
        state.status = Status::Restarting;
        state.report.replace(report)
    };
    drop(previous);
    RESTARTS.fetch_add(1, Ordering::Relaxed);
    generation.closed.store(true, Ordering::Release);
    generation.completed.notify_waiters();
    run.wake();
    runtime.block_on(async {
        let notified = run.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if crate::lock(&run.state).requested {
            return;
        }
        tokio::select! {
            biased;
            _ = notified => {},
            _ = tokio::time::sleep(delay) => {}
        }
    });
    true
}

fn cleanup_deadline(run: &Run) -> Instant {
    let own = Instant::now() + crate::CLEANUP;
    crate::lock(&run.state)
        .deadline
        .map_or(own, |deadline| deadline.min(own))
}

fn finish_resources(
    runtime: &tokio::runtime::Runtime,
    generation: &Generation,
    end: Instant,
) -> rivium::scope::Drain {
    *crate::lock(&generation.gate) = false;
    let resources = generation.resources();
    if let Some(reload) = &resources.reload {
        reload.close();
    }
    if let Some(logger) = &resources.logger
        && let Err(error) = logger.close(Duration::ZERO)
    {
        generation.failure(CleanupStage::Logger, error);
    }

    let drain = resources
        .readiness
        .map_or(rivium::scope::Drain::Complete, |readiness| {
            runtime.block_on(readiness.drain(end.saturating_duration_since(Instant::now())))
        });
    if let Some(reload) = resources.reload {
        reload.wait(end);
    }
    if let Some(logger) = resources.logger
        && let Err(error) = logger.close(end.saturating_duration_since(Instant::now()))
    {
        generation.failure(CleanupStage::Logger, error);
    }
    drain
}

struct Prepared<C> {
    lifecycle: Lifecycle,
    logger: Arc<log::Logger>,
    reload: reload::Requester,
    config: config::Handle<C>,
}

fn prepare<A: App>(
    options: &Options,
    generation: &Generation,
    embedded: bool,
    retry_enabled: &mut bool,
) -> std::result::Result<Prepared<A::Config>, (Code, Error)> {
    let loaded = catch_unwind(AssertUnwindSafe(|| load::<A>(options, None, false)))
        .map_err(|_| {
            (
                Code::Panicked,
                Error::internal("configuration hook panicked"),
            )
        })?
        .map_err(|error| (Code::Config, error))?;
    *retry_enabled = embedded || loaded.lifecycle.restart == Restart::InProcess;
    let layers = catch_unwind(AssertUnwindSafe(A::log_layers))
        .map_err(|_| (Code::Panicked, Error::internal("log layer hook panicked")))?;
    let logger = Arc::new(
        catch_unwind(AssertUnwindSafe(|| {
            log::Logger::new(A::NAME, options.paths.root(), loaded.log, !embedded, layers)
        }))
        .map_err(|_| (Code::Fault, Error::internal("logger construction panicked")))?
        .map_err(|error| (Code::CantCreate, error))?,
    );
    crate::lock(&generation.resources).logger = Some(logger.clone());
    let (reload, config) = reload::Requester::start::<A>(
        options.clone(),
        loaded.config,
        loaded.all,
        logger.dispatch(),
        generation.gate.clone(),
    )
    .map_err(|error| (Code::OsError, error))?;
    crate::lock(&generation.resources).reload = Some(reload.clone());
    Ok(Prepared {
        lifecycle: loaded.lifecycle,
        logger,
        reload,
        config,
    })
}

fn retry_initialization(
    runtime: &tokio::runtime::Runtime,
    run: &Run,
    generation: &Generation,
    retry: &mut RetryState,
    enabled: bool,
    failure: (Code, Error),
) -> std::result::Result<(), Code> {
    finish_resources(runtime, generation, cleanup_deadline(run));
    let mut cleanup = generation.cleanup();
    cleanup.closed = true;
    if !enabled || !cleanup.is_success() {
        return Err(failure.0);
    }

    let Some(delay) = retry.failed() else {
        return Err(failure.0);
    };
    FAULTS.fetch_add(1, Ordering::Relaxed);
    if park_retry(
        runtime,
        run,
        generation,
        HostReport {
            code: failure.0,
            primary: Some(failure.1),
            run: None,
            cleanup,
        },
        delay,
    ) {
        Ok(())
    } else {
        Err(Code::Ok)
    }
}

pub(crate) fn run<A: App>(options: Options, run: Arc<Run>, embedded: bool) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            run.finish(Code::OsError, Some(error.into()), None);
            return;
        }
    };
    #[cfg(feature = "process")]
    let signals = if !embedded {
        let setup = {
            let _entered = runtime.enter();
            crate::process::signals(run.clone())
        };
        match setup {
            Ok(signals) => Some(runtime.spawn(signals)),
            Err(error) => {
                runtime.shutdown_timeout(crate::CLEANUP);
                run.finish(Code::OsError, Some(error.into()), None);
                return;
            }
        }
    } else {
        None
    };
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let mut first_generation = true;
        let mut retry = RetryState::default();
        let mut retry_enabled = embedded;
        let final_code;
        let mut primary = None;
        let mut final_report = None;
        let mut cleanup_end = Instant::now() + crate::CLEANUP;
        loop {
            if crate::lock(&run.state).requested {
                if let Some(report) = run.report() {
                    primary = report.primary;
                    final_report = report.run;
                }
                final_code = if retry.ever_ready {
                    Code::Ok
                } else {
                    Code::Cancelled
                };
                break;
            }

            let generation = if first_generation {
                first_generation = false;
                run.generation()
            } else {
                let Some(generation) = run.replace_generation() else {
                    continue;
                };
                generation
            };
            let Prepared {
                lifecycle,
                logger,
                reload,
                config,
            } = match prepare::<A>(&options, &generation, embedded, &mut retry_enabled) {
                Ok(prepared) => prepared,
                Err((code, error)) => {
                    match retry_initialization(
                        &runtime,
                        &run,
                        &generation,
                        &mut retry,
                        retry_enabled,
                        (code, error.clone()),
                    ) {
                        Ok(()) => continue,
                        Err(code) => final_code = code,
                    }
                    primary = Some(error);
                    break;
                }
            };
            let dispatch = logger.dispatch();
            let run_id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
            let mut supervisor = Supervisor::new(lifecycle.startup_timeout, lifecycle.stop_timeout);
            let readiness = supervisor.readiness();
            let control = supervisor.control();
            let check_options = options.clone();
            let context = Context {
                config: config.clone(),
                reload: reload.clone(),
                paths: options.paths.clone(),
                readiness: readiness.clone(),
                control: crate::HostControl {
                    inner: control.clone(),
                    reload: reload.clone(),
                },
                health: readiness.health(),
                check: Arc::new(move |text| {
                    catch_unwind(AssertUnwindSafe(|| {
                        load::<A>(&check_options, Some(text), false).map(|_| ())
                    }))
                    .unwrap_or_else(|_| Err(Error::internal("configuration validation panicked")))
                }),
                #[cfg(feature = "log-export")]
                exporter: logger.exporter(),
            };
            crate::lock(&generation.resources).readiness = Some(readiness.clone());
            generation.install_control(control.clone());
            tracing::dispatcher::with_default(&dispatch, || {
                tracing::info!(
                    app = A::NAME,
                    run_id,
                    phase = "starting",
                    "services starting"
                );
            });
            let services = catch_unwind(AssertUnwindSafe(|| {
                tracing::dispatcher::with_default(&dispatch, || {
                    A::services(&config.current(), &context)
                })
            }));
            let service_error = match services {
                Ok(Ok(services)) => {
                    for service in services {
                        supervisor = supervisor.with(service);
                    }
                    None
                }
                Ok(Err(error)) => Some(error),
                Err(_) => Some(Error::internal("service construction panicked")),
            };
            let mut ready_at = None;
            let mut running_for = None;
            let report = if let Some(error) = service_error {
                drop(supervisor);
                rivium::RunReport {
                    outcome: Outcome::StartupFailed(error),
                    drain: rivium::scope::Drain::Complete,
                }
            } else {
                runtime.block_on(async {
                    let mut observed = readiness.clone();
                    let future = supervisor.run().with_subscriber(dispatch.clone());
                    tokio::pin!(future);
                    loop {
                        if observed.phase() == Phase::Running && ready_at.is_none() {
                            ready_at = Some(Instant::now());
                            {
                                let mut state = crate::lock(&run.state);
                                if !state.requested {
                                    state.status = Status::Running;
                                }
                                if state.first.is_none() {
                                    state.first = Some(Code::Ok);
                                }
                            }
                            run.wake();
                            tracing::dispatcher::with_default(&dispatch, || {
                                tracing::info!(
                                    app = A::NAME,
                                    run_id,
                                    phase = "running",
                                    "services running"
                                );
                            });
                        }
                        if matches!(observed.phase(), Phase::Stopping | Phase::Stopped) {
                            if running_for.is_none() {
                                running_for = ready_at.map(|start: Instant| start.elapsed());
                            }
                            reload.close();
                        }
                        tokio::select! {
                            biased;
                            report = &mut future => break report,
                            _ = observed.changed() => {},
                        }
                    }
                })
            };
            if let Some(ready_at) = ready_at {
                retry.observed_running(running_for.unwrap_or_else(|| ready_at.elapsed()));
            }
            tracing::dispatcher::with_default(&dispatch, || match &report.outcome {
                Outcome::Failed(error) | Outcome::StartupFailed(error) => {
                    tracing::error!(
                        app = A::NAME,
                        run_id,
                        class = ?error.class(),
                        drain = ?report.drain,
                        "service failed"
                    );
                }
                _ => {
                    tracing::info!(
                        app = A::NAME,
                        run_id,
                        outcome = ?report.outcome,
                        drain = ?report.drain,
                        "services stopped"
                    )
                }
            });
            cleanup_end = cleanup_deadline(&run);
            finish_resources(&runtime, &generation, cleanup_end);
            let (mut code, cause) = match &report.outcome {
                Outcome::Stopped => (Code::Ok, None),
                Outcome::Restart(_) => (Code::Restart, None),
                Outcome::StartupFailed(error) => (Code::StartupFailed, Some(error.clone())),
                Outcome::Failed(error) => (Code::Fault, Some(error.clone())),
                Outcome::TimedOut { .. } => (Code::StopTimedOut, None),
                Outcome::Aborted(signal) => (Code::Aborted(*signal), None),
            };
            let mut pending = run.cleanup();
            pending.closed = true;
            let can_retry = pending.pending == PendingResources::default()
                && pending.failures.is_empty()
                && retry_enabled;
            let delay = if can_retry {
                match &report.outcome {
                    Outcome::Restart(_) => Some(Duration::from_millis(100)),
                    Outcome::Failed(_) | Outcome::StartupFailed(_) => retry.failed(),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(delay) = delay {
                if matches!(
                    report.outcome,
                    Outcome::Failed(_) | Outcome::StartupFailed(_)
                ) {
                    FAULTS.fetch_add(1, Ordering::Relaxed);
                }
                if park_retry(
                    &runtime,
                    &run,
                    &generation,
                    HostReport {
                        code,
                        primary: cause.clone(),
                        run: Some(report.clone()),
                        cleanup: pending,
                    },
                    delay,
                ) {
                    continue;
                }
                code = Code::Ok;
            }
            final_code = code;
            primary = cause;
            final_report = Some(report);
            break;
        }
        (final_code, primary, final_report, cleanup_end)
    }));
    let (final_code, primary, final_report, cleanup_end) = match execution {
        Ok(result) => result,
        Err(_) => {
            let error = Error::internal("host driver panicked");
            let cleanup_end = cleanup_deadline(&run);
            let report = Some({
                let generation = run.generation();
                if let Some(control) = generation.resources().control {
                    control.stop(StopReason::Deadline(Duration::ZERO));
                }

                let drain = finish_resources(&runtime, &generation, cleanup_end);
                generation.failure(CleanupStage::Runtime, error.clone());
                rivium::RunReport {
                    outcome: Outcome::Failed(error.clone()),
                    drain,
                }
            });
            (Code::Fault, Some(error), report, cleanup_end)
        }
    };
    #[cfg(feature = "process")]
    if let Some(signals) = signals {
        signals.abort();
    }
    runtime.shutdown_timeout(cleanup_end.saturating_duration_since(Instant::now()));
    if matches!(
        final_code,
        Code::Fault | Code::StartupFailed | Code::Panicked
    ) {
        FAULTS.fetch_add(1, Ordering::Relaxed);
    }
    run.finish(final_code, primary, final_report);
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[tokio::test]
    async fn stop_wins_generation_admission_and_is_delivered_once_after_admission() {
        for admitted in [false, true] {
            let run = Run::new();
            let original = run.generation();
            assert!(!crate::lock(&run.state).requested);
            if admitted {
                assert!(run.replace_generation().is_some());
            }
            run.stop(StopReason::Requested, None);
            assert!(run.replace_generation().is_none());
            let generation = run.generation();
            assert_eq!(Arc::ptr_eq(&original, &generation), !admitted);
            assert!(!*crate::lock(&generation.gate));
            let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_secs(1))
                .with(rivium_test::idle("worker", rivium::Kind::Frontline));
            generation.install_control(supervisor.control());
            assert!(matches!(supervisor.run().await.outcome, Outcome::Stopped));
        }
    }

    #[test]
    fn first_failure_waits_100ms_and_tenth_failure_stops() {
        let mut state = RetryState::default();
        state.observed_running(Duration::ZERO);
        for millis in [100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600] {
            assert_eq!(state.failed(), Some(Duration::from_millis(millis)));
        }
        assert_eq!(state.failed(), None);
        assert_eq!(state.failed(), None);
    }

    #[test]
    fn only_600_seconds_of_running_resets_the_failure_budget() {
        let mut state = RetryState::default();
        state.observed_running(Duration::ZERO);
        assert_eq!(state.failed(), Some(Duration::from_millis(100)));
        state.observed_running(Duration::from_secs(599));
        assert_eq!(state.failed(), Some(Duration::from_millis(200)));
        state.observed_running(Duration::from_secs(600));
        assert_eq!(state.failed(), Some(Duration::from_millis(100)));
    }

    #[test]
    fn a_generation_that_never_reached_running_is_not_retried() {
        let mut state = RetryState::default();
        assert_eq!(state.failed(), None);
        assert_eq!(state.failures, 0);
    }
}
