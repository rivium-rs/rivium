//! The startup loop the hosts share: build a round of services, run it, then end or build the
//! next round. A round ends the loop unless the host restarts in the process: then a restart
//! request rebuilds at once, and a failure rebuilds after a backoff, until ten failures in a
//! row. The host's stop requests end the loop after the round they arrive in, or at once when
//! they arrive between rounds. A panic while a round is built or run fails the round. Also
//! what both hosts load and install before the first round, and how they build each round.

use std::ffi::OsString;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::Code;
use crate::app::{App, AppContext, Check, Identity};
use crate::config::de::format_duration;
use crate::config::load::{FileLayer, Inputs, Loaded, load};
use crate::config::{Paths, Problem, Report};
use crate::lifecycle::health::Phase;
use crate::lifecycle::tasks::CatchUnwind;
use crate::lifecycle::{Outcome, Restart, StopReason, Supervisor, stop_channel};
use crate::log::{self, InstallError, LogInputs, LogSettings, payload_text};
use crate::process::cli::Options;
use crate::stats::{FAULTS, RESTARTS};

/// How long the runtime may take to shut down once the services have ended.
pub(crate) const RUNTIME_SHUTDOWN: Duration = Duration::from_millis(400);
/// How long a log export in progress may take to stop as the host tears down.
pub(crate) const EXPORT_CANCEL: Duration = Duration::from_millis(100);
/// How long the last log lines may take to reach the log files.
pub(crate) const FLUSH: Duration = Duration::from_millis(500);
/// What a host takes after the stop deadline: the runtime, a log export being cancelled
/// (100ms), and the flush.
pub(crate) const AFTER_DEADLINE: Duration = Duration::from_secs(1);
/// The waits before rebuilding after the first, second… failure in a row; the last repeats.
const BACKOFF: [Duration; 7] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(60),
];
/// Failures in a row after which the loop gives up.
const GIVE_UP: u32 = 10;
/// A restart requested sooner than this after the last build counts as a failure.
const STORM: Duration = Duration::from_secs(10);
/// A round that stays running this long clears the count of failures.
const STEADY: Duration = Duration::from_secs(600);

/// A round of services, built and ready to run.
pub(crate) struct Round {
    pub(crate) supervisor: Supervisor,
    /// Whether this round's configuration restarts in the process.
    pub(crate) in_process: bool,
}

/// Why a round could not be built or ended badly: its code, and what to tell the operator.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
        Failure {
            code,
            message: message.into(),
        }
    }
}

/// How the loop ended: the code, and what went wrong if anything did.
#[derive(Debug)]
pub(crate) struct End {
    pub(crate) code: Code,
    pub(crate) message: Option<String>,
}

/// What the loop tells its host as it goes; the embedded host keeps its status with them.
#[derive(Debug)]
pub(crate) enum Event<'a> {
    /// The round entered this phase.
    Phase(Phase),
    /// A round failed; the loop builds another after a backoff.
    Failed(&'a Failure),
    /// A round after the first was built and runs.
    Rebuilt,
}

/// Runs rounds until one ends the loop. `build` builds a round, `true` for the first one;
/// `in_process` says whether the first round's configuration restarts in the process, and
/// later rounds say it for themselves. Without `retry_first`, as for the embedded host, whose
/// start reports how the first round went, a first round that never runs ends the loop.
/// `stops` brings the host's stop requests; `events` hears how the rounds go.
pub(crate) async fn run<B, E>(
    mut build: B,
    mut in_process: bool,
    retry_first: bool,
    stops: &mut mpsc::UnboundedReceiver<StopReason>,
    mut events: E,
) -> End
where
    B: FnMut(bool) -> Result<Round, Failure>,
    E: FnMut(Event<'_>),
{
    let (mut first, mut failures) = (true, 0);
    loop {
        let built = Instant::now();
        let round = panic::catch_unwind(AssertUnwindSafe(|| build(first)))
            .unwrap_or_else(|payload| Err(panicked(&*payload)));
        let (ended, ran) = match round {
            Err(failure) => (Ended::Failed(failure), false),
            Ok(round) => {
                in_process = round.in_process;
                if let Some(end) = stopped_between_rounds(stops) {
                    return end;
                }
                if !first {
                    events(Event::Rebuilt);
                }
                run_round(round.supervisor, stops, &mut events).await
            }
        };
        if first && !ran && !retry_first {
            let message = match ended {
                Ended::Stopped(end) => return end,
                Ended::Failed(failure) | Ended::Steady(failure) => failure.message,
                Ended::Restart(why) => format!("startup failed: restart requested: {why}"),
            };
            return Failure::new(Code::StartupFailed, message).into();
        }
        first = false;
        let failure = match ended {
            Ended::Stopped(end) => return end,
            Ended::Failed(failure) if !in_process => return failure.into(),
            Ended::Failed(failure) => failure,
            Ended::Steady(failure) => {
                failures = 0;
                failure
            }
            Ended::Restart(why) if !in_process => {
                let message = format!("restart requested: {why}");
                return Failure::new(Code::Restart, message).into();
            }
            Ended::Restart(_) if built.elapsed() >= STORM => {
                tracing::info!("restarting as requested");
                RESTARTS.fetch_add(1, Relaxed);
                match stopped_between_rounds(stops) {
                    Some(end) => return end,
                    None => continue,
                }
            }
            Ended::Restart(why) => {
                let soon = format_duration(STORM);
                let message = format!("restart requested within {soon} of the last start: {why}");
                Failure::new(Code::Restart, message)
            }
        };
        failures += 1;
        if failures >= GIVE_UP {
            tracing::error!(failures, "gave up restarting");
            let message = format!(
                "gave up after {failures} failures in a row; the last: {}",
                failure.message
            );
            return Failure::new(failure.code, message).into();
        }
        events(Event::Failed(&failure));
        let delay = BACKOFF[(failures as usize - 1).min(BACKOFF.len() - 1)];
        let (shown, reason) = (format_duration(delay), &failure.message);
        tracing::warn!(failures, delay = %shown, reason, "restarting after a failure");
        RESTARTS.fetch_add(1, Relaxed);
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            Some(_) = stops.recv() => return stopped_at_once(stops),
        }
        if let Some(end) = stopped_between_rounds(stops) {
            return end;
        }
    }
}

/// How a round ended, for the loop.
enum Ended {
    /// The loop ends: the host stopped the round, or every service finished.
    Stopped(End),
    /// A failure.
    Failed(Failure),
    /// A failure after the round had been running for long enough to clear the count.
    Steady(Failure),
    /// A restart request.
    Restart(String),
}

/// Runs one round, passing the host's stop requests on to it and its phases to `events`; says
/// how it ended and whether it was running.
async fn run_round<E: FnMut(Event<'_>)>(
    mut supervisor: Supervisor,
    stops: &mut mpsc::UnboundedReceiver<StopReason>,
    events: &mut E,
) -> (Ended, bool) {
    let mut phases = supervisor.phases();
    let (stop, receiver) = stop_channel();
    let mut run = pin!(CatchUnwind(Box::pin(supervisor.supervise(receiver))));
    let (mut stopped, mut running_since) = (false, None);
    let mut entered = |phase, events: &mut E| {
        if phase == Phase::Running {
            running_since = Some(Instant::now());
        }
        events(Event::Phase(phase));
    };
    let ended = loop {
        tokio::select! {
            ended = &mut run => break ended,
            Some(reason) = stops.recv() => {
                stopped = true;
                stop.stop(reason);
            }
            Some(phase) = phases.recv() => entered(phase, events),
        }
    };
    // The phases of the run's last moments may still be queued.
    while let Ok(phase) = phases.try_recv() {
        entered(phase, events);
    }
    let ran = running_since.is_some();
    let (outcome, abandoned) = match ended {
        Ok(ended) => ended,
        Err(message) => {
            let failure = Failure::new(Code::StartupFailed, format!("panic: {message}"));
            return (Ended::Failed(failure), ran);
        }
    };
    let code = code_of(&outcome);
    let mut message = match &outcome {
        Outcome::Stopped => None,
        Outcome::StopTimedOut => Some("stop timed out".to_string()),
        Outcome::StartupFailed(error) => Some(format!("startup failed: {error:#}")),
        Outcome::Fault(error) => Some(format!("stopped after a fault: {error:#}")),
        Outcome::RestartRequested(why) => Some(format!("restart requested: {why}")),
        Outcome::Aborted(stop) => Some(format!(
            "the stop was cut short by a second stop request ({stop})"
        )),
    };
    if !abandoned.is_empty() {
        let line = format!("abandoned: {}", abandoned.join(", "));
        message = Some(message.map_or(line.clone(), |message| format!("{message}\n{line}")));
    }
    if matches!(outcome, Outcome::Fault(_)) {
        FAULTS.fetch_add(1, Relaxed);
    }
    let steady = running_since.is_some_and(|since| since.elapsed() >= STEADY);
    let ended = match outcome {
        _ if stopped => Ended::Stopped(End { code, message }),
        Outcome::Stopped => Ended::Stopped(End { code, message }),
        Outcome::RestartRequested(why) => Ended::Restart(why.into_owned()),
        _ if steady => Ended::Steady(Failure::new(code, message.unwrap_or_default())),
        _ => Ended::Failed(Failure::new(code, message.unwrap_or_default())),
    };
    (ended, ran)
}

/// The code of a round's outcome. The embedded host sends one stop request per start, so a
/// second one, which aborts a run, comes from signals only.
pub(crate) fn code_of(outcome: &Outcome) -> Code {
    match outcome {
        Outcome::Stopped => Code::Ok,
        Outcome::StopTimedOut => Code::StopTimedOut,
        Outcome::StartupFailed(_) => Code::StartupFailed,
        Outcome::Fault(_) => Code::Fault,
        Outcome::RestartRequested(_) => Code::Restart,
        Outcome::Aborted(StopReason::Signal { number, .. }) => Code::Aborted(*number),
        Outcome::Aborted(_) => Code::StopTimedOut,
    }
}

/// A stop request that arrived between rounds ends the loop at once.
fn stopped_between_rounds(stops: &mut mpsc::UnboundedReceiver<StopReason>) -> Option<End> {
    stops.try_recv().ok().map(|_| stopped_at_once(stops))
}

/// The loop ends at once for a stop request between rounds: with `Ok`, unless a second one has
/// arrived too.
fn stopped_at_once(stops: &mut mpsc::UnboundedReceiver<StopReason>) -> End {
    match stops.try_recv() {
        Ok(second) => {
            let message = format!("the stop was cut short by a second stop request ({second})");
            let aborted = Outcome::Aborted(second);
            End {
                code: code_of(&aborted),
                message: Some(message),
            }
        }
        Err(_) => End {
            code: Code::Ok,
            message: None,
        },
    }
}

fn panicked(payload: &(dyn std::any::Any + Send)) -> Failure {
    Failure::new(
        Code::StartupFailed,
        format!("startup failed: panic: {}", payload_text(payload)),
    )
}

impl From<Failure> for End {
    fn from(failure: Failure) -> Self {
        End {
            code: failure.code,
            message: Some(failure.message),
        }
    }
}

/// What a round's configuration was loaded with besides the file: the environment (none for
/// the embedded host) and the `--set` overrides.
#[derive(Clone, Debug)]
pub(crate) struct Overrides {
    pub(crate) env: Option<Vec<(OsString, OsString)>>,
    pub(crate) sets: Vec<(String, String)>,
    /// Whether the `--set` overrides come from the embedded host rather than the command line.
    pub(crate) host: bool,
}

/// The check of a candidate configuration file at `file`, loaded with the round's overrides.
/// When the host restarts in the process, `installed` holds the log settings, which cannot
/// change but for their filters.
pub(crate) fn check<C: Serialize + DeserializeOwned + Default>(
    name: &'static str,
    file: PathBuf,
    overrides: Overrides,
    installed: Option<LogSettings>,
) -> Check {
    Arc::new(move |candidate| {
        let inputs = Inputs {
            name,
            file: FileLayer::Text {
                path: file.clone(),
                text: candidate.to_string(),
            },
            env: overrides.env.as_deref(),
            sets: &overrides.sets,
            host: overrides.host,
        };
        let loaded = load::<C>(&inputs)?;
        let changed = (installed.as_ref())
            .map(|installed| loaded.reserved.log.unchangeable(installed))
            .unwrap_or_default();
        let problems: Vec<Problem> = (changed.into_iter())
            .map(|key| {
                let reason = "cannot change while the process runs: restart the process instead";
                Problem::new(Some(key.to_string()), loaded.source_of(key), reason)
            })
            .collect();
        match problems.is_empty() {
            true => Ok(()),
            false => Err(Report::new(problems)),
        }
    })
}

/// Finds the root and the configuration file in `options`, and loads the configuration as the
/// host does: with the environment, which the embedded host does not read (`None`), and the
/// `--set` overrides.
pub(crate) fn configure<A: App>(
    options: &Options,
    env: Option<&[(OsString, OsString)]>,
) -> Result<(Paths, bool, Loaded<A::Config>), Failure> {
    let (root, config) = (options.root.clone(), options.config.clone());
    let (paths, explicit) = Paths::locate(A::NAME, root, config, env, A::CONFIG_FILE)
        .map_err(|error| Failure::new(Code::Usage, format!("{error:#}")))?;
    let loaded = load_config::<A>(&paths, explicit, env, &options.sets)?;
    Ok((paths, explicit, loaded))
}

fn load_config<A: App>(
    paths: &Paths,
    explicit: bool,
    env: Option<&[(OsString, OsString)]>,
    sets: &[(String, String)],
) -> Result<Loaded<A::Config>, Failure> {
    let path = paths.config_file().to_path_buf();
    let inputs = Inputs {
        name: A::NAME,
        file: FileLayer::Path { path, explicit },
        env,
        sets,
        host: env.is_none(),
    };
    load::<A::Config>(&inputs).map_err(|problems| Failure::new(Code::Config, problems.to_string()))
}

/// Installs logging, or reloads its filters once it is installed: with standard output for the
/// process host, with Android's log for the embedded host. The service's own layers are built
/// for the first installation only.
pub(crate) fn install_logging<A: App>(
    paths: &Paths,
    loaded: &Loaded<A::Config>,
    embedded: bool,
) -> Result<(), Failure> {
    let settings = &loaded.reserved.log;
    let layers = match log::installed() {
        true => Vec::new(),
        false => A::log_layers(&loaded.config),
    };
    let inputs = LogInputs {
        name: A::NAME.to_string(),
        settings: settings.clone(),
        dir: paths.resolve(&settings.file.dir),
        console: !embedded,
        logcat: embedded,
        layers,
    };
    log::install(inputs).map_err(|error| {
        let code = match error {
            InstallError::Changed(_) => Code::Config,
            InstallError::Io(_) => Code::CantCreate,
        };
        Failure::new(code, error.to_string())
    })
}

/// What a host builds each round from.
pub(crate) struct Rounds<'a, A: App> {
    pub(crate) paths: Paths,
    pub(crate) explicit: bool,
    /// The environment; `None` for the embedded host, which reads none and always restarts in
    /// the process.
    pub(crate) env: Option<&'a [(OsString, OsString)]>,
    pub(crate) sets: Vec<(String, String)>,
    /// The configuration of the first round, loaded before logging was installed.
    pub(crate) first: Option<Loaded<A::Config>>,
}

impl<A: App> Rounds<'_, A> {
    /// The next round: its configuration, loaded again after the first round, and its
    /// services.
    pub(crate) fn build(&mut self) -> Result<Round, Failure> {
        let loaded = match self.first.take() {
            Some(loaded) => loaded,
            None => self.reload()?,
        };
        let lifecycle = &loaded.reserved.lifecycle;
        let embedded = self.env.is_none();
        let in_process = embedded || lifecycle.restart == Restart::InProcess;
        let supervisor = Supervisor::new(lifecycle.startup_timeout, lifecycle.stop_timeout);
        let identity = Identity {
            name: A::NAME,
            version: A::VERSION,
            instance: lifecycle.instance.clone(),
        };
        let file = self.paths.config_file().to_path_buf();
        let overrides = Overrides {
            env: self.env.map(<[_]>::to_vec),
            sets: self.sets.clone(),
            host: embedded,
        };
        let installed = in_process.then(|| loaded.reserved.log.clone());
        let check = check::<A::Config>(A::NAME, file, overrides, installed);
        let ctx = AppContext::new(self.paths.clone(), identity, &supervisor, check);
        let services = A::services(&loaded.config, &ctx).map_err(|error| {
            let why = format!("startup failed: the services cannot be built: {error:#}");
            Failure::new(Code::StartupFailed, why)
        })?;
        let supervisor = services.into_iter().fold(supervisor, Supervisor::with);
        Ok(Round {
            supervisor,
            in_process,
        })
    }

    /// Loads the configuration again and reloads the log filters.
    fn reload(&self) -> Result<Loaded<A::Config>, Failure> {
        let loaded = load_config::<A>(&self.paths, self.explicit, self.env, &self.sets)?;
        install_logging::<A>(&self.paths, &loaded, self.env.is_none())?;
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests;
