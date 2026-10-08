//! The embedded host: runs an [`App`] inside another program, such as an Android app through
//! rivium-jni, which starts and stops it with calls instead of signals.
//!
//! [`Host::start`] runs the services on a thread of their own, `<name>-main`, and returns once
//! the first round of services is running or has failed; [`Host::stop`] stops them and waits
//! for the stop. The arguments of `start` are the options of the process host's `run`:
//! `--root DIR`, which is required, `--config FILE` and `--set KEY=VALUE`; the environment is
//! not read. Logs go to files below the root, and on Android to Android's log, never to
//! standard output. A round that fails or asks for a restart is rebuilt in the process, after a
//! backoff for failures, until ten failures in a row leave the host [`Status::Failed`].
//!
//! ```no_run
//! # struct Agent;
//! # impl rivium::App for Agent {
//! #     const NAME: &'static str = "agent";
//! #     const VERSION: &'static str = "0.1.0";
//! #     type Config = ();
//! #     fn services(_: &(), _: &rivium::AppContext) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
//! #         Ok(Vec::new())
//! #     }
//! # }
//! use std::time::Duration;
//!
//! use rivium::Code;
//! use rivium::embedded::Host;
//!
//! let host = Host::new::<Agent>();
//! let args = ["--root", "/data/agent", "--set", "log.filter=debug"].map(String::from);
//! assert_eq!(host.start(&args), Code::Ok);
//! assert_eq!(host.stop(Duration::from_secs(2)), Code::Ok);
//! ```

use std::ffi::OsString;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use crate::host::{self, AFTER_DEADLINE, End, Event, FLUSH, Failure, RUNTIME_SHUTDOWN, Rounds};
use crate::lifecycle::StopReason;
use crate::lifecycle::health::Phase;
use crate::log::{self, payload_text};
use crate::process::cli::{self, Options};
use crate::{App, Code};

/// What the guard on the first round adds to its startup and stop timeouts.
const GUARD: Duration = Duration::from_secs(2);

/// What an embedded host is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Status {
    /// Not started, or stopped.
    #[default]
    Idle = 0,
    /// The services are starting: after `start`, or after a restart in the process.
    Starting = 1,
    /// Every service is running.
    Running = 2,
    /// The services are stopping, after `stop` or when the first round had no result in time.
    Stopping = 3,
    /// Restarting in the process: the services stop after a failure or a restart request, a
    /// failure waits out a backoff, then the configuration is loaded again.
    Restarting = 4,
    /// Ten failures in a row: the host gave up restarting. `start` starts again.
    Failed = 5,
}

impl Status {
    /// The number of the status for FFI: idle 0, starting 1, running 2, stopping 3,
    /// restarting 4, failed 5.
    #[must_use]
    pub const fn ffi(self) -> i32 {
        self as i32
    }
}

/// Runs an [`App`] on behalf of the program it is embedded in: [`start`](Self::start) and
/// [`stop`](Self::stop) as often as the program likes, from any thread. One host per process:
/// logging is installed once, below the root of the first start.
pub struct Host {
    name: &'static str,
    main: Main,
    shared: Arc<Shared>,
}

/// The thread of one start: `main::<A>`.
type Main = fn(Arc<Shared>, u64, Options, mpsc::UnboundedReceiver<StopReason>);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// One lock serializes every change; callers wait outside it.
#[derive(Debug, Default)]
struct State {
    status: Status,
    last_error: Option<String>,
    /// The number of the last start.
    run: u64,
    /// The stop requests to the last start's rounds, until its thread ends.
    stops: Option<mpsc::UnboundedSender<StopReason>>,
    /// The thread of the last start, until it is joined.
    thread: Option<JoinHandle<()>>,
    /// What the first round of the last start came to, once that is known.
    first: Option<Code>,
    /// When the waiting start stops waiting for the first round.
    guard: Option<Instant>,
    /// The host asked the last start's services to stop.
    stopping: bool,
    /// The start that ended last, and what `stop` returns for it.
    ended: Option<(u64, Code)>,
}

impl Host {
    /// A host for `A`, idle until it is started.
    #[must_use]
    pub fn new<A: App>() -> Host {
        Host {
            name: A::NAME,
            main: main::<A>,
            shared: Arc::default(),
        }
    }

    /// Starts the services with `args` and waits for the first round:
    ///
    /// - `Ok`: every service is running;
    /// - `Usage`, `Config`, `CantCreate`, `OsError`: the arguments, the configuration, the log
    ///   directory or the thread or runtime let nothing start;
    /// - `StartupFailed`: the services did not start, or had no result within the startup and
    ///   stop timeouts plus 2s, after which they are stopped;
    /// - `Cancelled`: [`stop`](Self::stop) came first;
    /// - at once, `AlreadyRunning` while starting, running or restarting, and `Busy` while
    ///   stopping.
    ///
    /// It blocks the calling thread, so it must not be called on Android's main thread.
    pub fn start(&self, args: &[String]) -> Code {
        log::install_panic_hook(self.name);
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let mut state = self.shared.state();
        if let Err(code) = state.may_start() {
            return code;
        }
        let options = match cli::options(&args, false) {
            Ok(options) => options,
            Err(problem) => return state.failed(Code::Usage, &problem),
        };
        // The last start's thread has ended its run already: joining it takes no time.
        if let Some(thread) = state.thread.take() {
            let _ = thread.join();
        }
        let (stop, stops) = mpsc::unbounded_channel();
        let run = state.run + 1;
        *state = State {
            status: Status::Starting,
            last_error: state.last_error.take(),
            run,
            stops: Some(stop),
            ended: state.ended.take(),
            ..State::default()
        };
        let (shared, main) = (Arc::clone(&self.shared), self.main);
        let thread = thread::Builder::new()
            .name(format!("{}-main", self.name))
            .spawn(move || main(shared, run, options, stops));
        match thread {
            Ok(thread) => state.thread = Some(thread),
            Err(error) => {
                (state.status, state.stops) = (Status::Idle, None);
                return state.failed(Code::OsError, &format!("cannot start a thread: {error}"));
            }
        }
        loop {
            if let Some(code) = state.first {
                self.shared.reap(state, run);
                return code;
            }
            let left = state
                .guard
                .map(|at| at.saturating_duration_since(Instant::now()));
            if left.is_some_and(|left| left.is_zero()) {
                return state.overdue();
            }
            state = self.shared.wait(state, left);
        }
    }

    /// Stops the services, giving them `timeout` less the host's own second at most, and waits
    /// up to `timeout`: `Ok` when they stopped and their last logs reached the files in time,
    /// even if one failed while stopping (see [`last_error`](Self::last_error)); `StopTimedOut`
    /// when a service was abandoned at its deadline or the stop took longer; `NotRunning` when
    /// idle or failed. A stop while stopping waits for the same stop. On Android's main thread,
    /// keep `timeout` within 2 seconds.
    pub fn stop(&self, timeout: Duration) -> Code {
        let until = Instant::now().checked_add(timeout);
        let mut state = self.shared.state();
        if let Err(code) = state.stop(timeout.saturating_sub(AFTER_DEADLINE)) {
            return code;
        }
        self.shared.changed.notify_all();
        let run = state.run;
        loop {
            if let Some((ended, code)) = state.ended
                && ended == run
            {
                self.shared.reap(state, run);
                return code;
            }
            let left = until.map(|until| until.saturating_duration_since(Instant::now()));
            if left.is_some_and(|left| left.is_zero()) {
                return Code::StopTimedOut;
            }
            state = self.shared.wait(state, left);
        }
    }

    /// What the host is doing.
    #[must_use]
    pub fn status(&self) -> Status {
        self.shared.state().status
    }

    /// The last failure, as `"<code name>: <message>"`, until a start gets the services
    /// running: a start that failed, a failure that made the host restart, a service that
    /// failed while stopping.
    #[must_use]
    pub fn last_error(&self) -> Option<String> {
        self.shared.state().last_error.clone()
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host")
            .field("name", &self.name)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // Every change completes under the lock, so a poisoned state is still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the state of start `run`, unless a later start has begun, and wakes the waiting
    /// callers.
    fn update(&self, run: u64, change: impl FnOnce(&mut State)) {
        let mut state = self.state();
        if state.run == run {
            change(&mut state);
            self.changed.notify_all();
        }
    }

    /// Waits for a change, at most `timeout` when there is one.
    fn wait<'a>(
        &self,
        state: MutexGuard<'a, State>,
        timeout: Option<Duration>,
    ) -> MutexGuard<'a, State> {
        match timeout {
            None => self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
            Some(timeout) => {
                let waited = self.changed.wait_timeout(state, timeout);
                waited.unwrap_or_else(PoisonError::into_inner).0
            }
        }
    }

    /// Joins the thread of start `run` once it has ended, after letting go of the lock.
    fn reap(&self, mut state: MutexGuard<'_, State>, run: u64) {
        let thread = match state.ended {
            Some((ended, _)) if ended == run => state.thread.take(),
            _ => None,
        };
        drop(state);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl State {
    /// Whether a start may begin: not while another start runs, restarts or stops.
    fn may_start(&self) -> Result<(), Code> {
        match self.status {
            Status::Starting | Status::Running | Status::Restarting => Err(Code::AlreadyRunning),
            Status::Stopping => Err(Code::Busy),
            Status::Idle | Status::Failed => Ok(()),
        }
    }

    /// Asks the services to stop, leaving them `budget`; a start still waiting for its first
    /// round is cancelled. While stopping, nothing more is asked: a second stop request would
    /// abort the stop.
    fn stop(&mut self, budget: Duration) -> Result<(), Code> {
        match self.status {
            Status::Idle | Status::Failed => return Err(Code::NotRunning),
            Status::Stopping => {}
            Status::Starting | Status::Running | Status::Restarting => {
                (self.status, self.stopping) = (Status::Stopping, true);
                self.first.get_or_insert(Code::Cancelled);
                if let Some(stops) = &self.stops {
                    let _ = stops.send(StopReason::Host { budget });
                }
            }
        }
        Ok(())
    }

    /// The first round had no result within the guard: its services are stopped at once.
    fn overdue(&mut self) -> Code {
        let _ = self.stop(Duration::ZERO);
        self.first = Some(Code::StartupFailed);
        let why = "no result within the startup and stop timeouts plus 2s; stopping the services";
        self.failed(Code::StartupFailed, why)
    }

    /// Records a failure as the last error and returns its code.
    fn failed(&mut self, code: Code, message: &str) -> Code {
        self.last_error = Some(format!("{}: {message}", code.name()));
        code
    }

    /// The loop of the last start tells how its rounds go.
    fn event(&mut self, event: Event<'_>) {
        let (own, starting_or_running) = (
            !self.stopping,
            matches!(self.status, Status::Starting | Status::Running),
        );
        match event {
            Event::Phase(Phase::Running) if own && self.status == Status::Starting => {
                self.status = Status::Running;
                if self.first.is_none() {
                    (self.first, self.last_error) = (Some(Code::Ok), None);
                }
            }
            // A round stops on its own, for a failure or a restart request. Before the first
            // round runs, the waiting start reports that instead.
            Event::Phase(Phase::Stopping) if own && starting_or_running && self.first.is_some() => {
                self.status = Status::Restarting;
            }
            Event::Failed(failure) => {
                self.failed(failure.code, &failure.message);
                if own && starting_or_running {
                    self.status = Status::Restarting;
                }
            }
            Event::Rebuilt if own && self.status == Status::Restarting => {
                self.status = Status::Starting;
            }
            _ => {}
        }
    }

    /// The thread of start `run` ends with `end`; `flushed` says whether the last logs reached
    /// the files in time.
    fn ended(&mut self, run: u64, end: End, flushed: bool) {
        let failed = end.code != Code::Ok;
        let gave_up = failed && !self.stopping && self.first == Some(Code::Ok);
        if let Some(message) = end.message.filter(|_| failed) {
            self.failed(end.code, &message);
        }
        self.first.get_or_insert(end.code);
        self.status = if gave_up {
            Status::Failed
        } else {
            Status::Idle
        };
        self.stops = None;
        let timed_out = end.code == Code::StopTimedOut || !flushed;
        self.ended = Some((
            run,
            if timed_out {
                Code::StopTimedOut
            } else {
                Code::Ok
            },
        ));
    }
}

/// The thread of start `run`: loads the configuration, installs logging, builds the runtime and
/// runs rounds of services until one ends the loop, then tears down and says how it ended.
fn main<A: App>(
    shared: Arc<Shared>,
    run: u64,
    options: Options,
    mut stops: mpsc::UnboundedReceiver<StopReason>,
) {
    let served = panic::catch_unwind(AssertUnwindSafe(|| {
        serve::<A>(&shared, run, options, &mut stops)
    }));
    let end: End = served.unwrap_or_else(|payload| {
        let why = format!("startup failed: panic: {}", payload_text(&*payload));
        Failure::new(Code::StartupFailed, why).into()
    });
    let code = end.code.name();
    match &end.message {
        None => tracing::info!(code, "stopped"),
        Some(why) => tracing::warn!(code, reason = %why, "stopped"),
    }
    let flushed = log::flush(FLUSH).is_ok();
    shared.update(run, |state| state.ended(run, end, flushed));
}

fn serve<A: App>(
    shared: &Shared,
    run: u64,
    options: Options,
    stops: &mut mpsc::UnboundedReceiver<StopReason>,
) -> End {
    let (runtime, mut rounds) = match prepare::<A>(shared, run, options) {
        Ok(prepared) => prepared,
        Err(failure) => return failure.into(),
    };
    let events = |event: Event<'_>| shared.update(run, |state| state.event(event));
    let end = runtime.block_on(host::run(|_| rounds.build(), true, false, stops, events));
    shared.update(run, |state| {
        // Every service finished on its own: the rest is a stop.
        if state.status == Status::Running {
            state.status = Status::Stopping;
        }
    });
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    end
}

/// Loads the configuration, installs logging and builds the runtime of a start.
fn prepare<A: App>(
    shared: &Shared,
    run: u64,
    options: Options,
) -> Result<(Runtime, Rounds<'static, A>), Failure> {
    let (paths, explicit, loaded) = host::configure::<A>(&options, None)?;
    let lifecycle = &loaded.reserved.lifecycle;
    let guard = Instant::now() + lifecycle.startup_timeout + lifecycle.stop_timeout + GUARD;
    shared.update(run, |state| state.guard = Some(guard));
    host::install_logging::<A>(&paths, &loaded, true)?;
    tracing::info!(
        service.name = A::NAME,
        service.version = A::VERSION,
        service.instance = %lifecycle.instance,
        root = %paths.root().display(),
        config.file = loaded.file.as_ref().map(|file| file.display().to_string()),
        overrides = loaded.overrides,
        "starting"
    );
    if let Some(missing) = &loaded.missing_file {
        let file = missing.display();
        tracing::warn!(config.file = %file, "no configuration file; the defaults apply");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name(format!("{}-worker", A::NAME))
        .build()
        .map_err(|error| {
            Failure::new(Code::OsError, format!("cannot build the runtime: {error}"))
        })?;
    let rounds = Rounds {
        paths,
        explicit,
        env: None,
        sets: options.sets,
        first: Some(loaded),
    };
    Ok((runtime, rounds))
}

#[cfg(test)]
mod tests;
