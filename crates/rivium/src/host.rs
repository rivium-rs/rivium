//! The startup loop the hosts share: build a round of services, run it, then end or build the
//! next round. A round ends the loop unless the host restarts in the process: then a restart
//! request rebuilds at once, and a failure rebuilds after a backoff, until ten failures in a
//! row. The host's stop requests end the loop after the round they arrive in, or at once when
//! they arrive between rounds. A panic while a round is built or run fails the round.

use std::ffi::OsString;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::Code;
use crate::app::Check;
use crate::config::de::format_duration;
use crate::config::load::{FileLayer, Inputs, load};
use crate::config::{Problem, Report};
use crate::lifecycle::health::Phase;
use crate::lifecycle::tasks::CatchUnwind;
use crate::lifecycle::{Outcome, StopReason, Supervisor, stop_channel};
use crate::log::{LogSettings, payload_text};
use crate::stats::{FAULTS, RESTARTS};

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

/// Runs rounds until one ends the loop. `build` builds a round, `true` for the first one;
/// `in_process` says whether the first round's configuration restarts in the process, and
/// later rounds say it for themselves; `stops` brings the host's stop requests.
pub(crate) async fn run<B>(
    mut build: B,
    mut in_process: bool,
    stops: &mut mpsc::UnboundedReceiver<StopReason>,
) -> End
where
    B: FnMut(bool) -> Result<Round, Failure>,
{
    let (mut first, mut failures) = (true, 0);
    loop {
        let built = Instant::now();
        let round = panic::catch_unwind(AssertUnwindSafe(|| build(first)))
            .unwrap_or_else(|payload| Err(panicked(&*payload)));
        first = false;
        let ended = match round {
            Err(failure) => Ended::Failed(failure),
            Ok(round) => {
                in_process = round.in_process;
                if let Some(end) = stopped_between_rounds(stops) {
                    return end;
                }
                run_round(round.supervisor, stops).await
            }
        };
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

/// Runs one round, passing the host's stop requests on to it.
async fn run_round(
    supervisor: Supervisor,
    stops: &mut mpsc::UnboundedReceiver<StopReason>,
) -> Ended {
    let mut phase = supervisor.readiness().phase;
    let (stop, receiver) = stop_channel();
    let mut run = pin!(CatchUnwind(Box::pin(supervisor.supervise(receiver))));
    let (mut stopped, mut running_since) = (false, None);
    let ended = loop {
        tokio::select! {
            ended = &mut run => break ended,
            Some(reason) = stops.recv() => {
                stopped = true;
                stop.stop(reason);
            }
            Ok(()) = phase.changed() => {
                if *phase.borrow_and_update() == Phase::Running {
                    running_since = Some(Instant::now());
                }
            }
        }
    };
    let (outcome, abandoned) = match ended {
        Ok(ended) => ended,
        Err(message) => {
            return Ended::Failed(Failure::new(
                Code::StartupFailed,
                format!("panic: {message}"),
            ));
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
    match outcome {
        _ if stopped => Ended::Stopped(End { code, message }),
        Outcome::Stopped => Ended::Stopped(End { code, message }),
        Outcome::RestartRequested(why) => Ended::Restart(why.into_owned()),
        _ if steady => Ended::Steady(Failure::new(code, message.unwrap_or_default())),
        _ => Ended::Failed(Failure::new(code, message.unwrap_or_default())),
    }
}

/// The code of a round's outcome.
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
        format!("panic: {}", payload_text(payload)),
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
    Box::new(move |candidate| {
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

#[cfg(test)]
mod tests;
