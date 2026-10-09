//! The rules of the embedded host's state, driven directly: every status against every call,
//! the first round's results, restarts in the process and giving up, which takes ten failures
//! and about four minutes of backoff for real; and callers that get the lock back only after
//! the next start began, with scripted threads in place of `main`. `tests/embedded.rs` runs a
//! real host.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{Host, Shared, State, Status};
use crate::Code;
use crate::host::{End, Event, Failure};
use crate::lifecycle::StopReason;
use crate::lifecycle::health::Phase;
use crate::process::cli::Options;

/// A state in `status` for start 1, with its stop requests.
fn fresh(status: Status) -> (State, mpsc::UnboundedReceiver<StopReason>) {
    let (stop, stops) = mpsc::unbounded_channel();
    let state = State {
        status,
        run: 1,
        stops: Some(stop),
        ..State::default()
    };
    (state, stops)
}

/// The budgets of the stop requests sent so far.
fn sent(stops: &mut mpsc::UnboundedReceiver<StopReason>) -> Vec<Duration> {
    std::iter::from_fn(|| stops.try_recv().ok())
        .map(|stop| stop.budget().expect("a host stop"))
        .collect()
}

fn end(code: Code, message: Option<&str>) -> End {
    End {
        code,
        message: message.map(String::from),
    }
}

/// What the start's caller returns, once known.
fn first(state: &State) -> Option<Code> {
    state.results.first.get().copied()
}

fn running() -> (State, mpsc::UnboundedReceiver<StopReason>) {
    let (mut state, stops) = fresh(Status::Starting);
    state.event(Event::Phase(Phase::Running));
    (state, stops)
}

#[test]
fn every_call_in_every_status() {
    use Status::{Failed, Idle, Restarting, Running, Starting, Stopping};
    let budget = Duration::from_millis(1_500);
    let cases = [
        // status, start, stop, whether the stop asks the services to stop, ffi
        (Idle, Ok(()), Err(Code::NotRunning), false, 0),
        (Starting, Err(Code::AlreadyRunning), Ok(()), true, 1),
        (Running, Err(Code::AlreadyRunning), Ok(()), true, 2),
        (Stopping, Err(Code::Busy), Ok(()), false, 3),
        (Restarting, Err(Code::AlreadyRunning), Ok(()), true, 4),
        (Failed, Ok(()), Err(Code::NotRunning), false, 5),
    ];
    for (status, start, stop, asks, ffi) in cases {
        let (mut state, mut stops) = fresh(status);
        assert_eq!(state.may_start(), start, "{status:?}");
        assert_eq!(state.stop(budget), stop, "{status:?}");
        let expected = if asks { vec![budget] } else { Vec::new() };
        assert_eq!(sent(&mut stops), expected, "{status:?}");
        assert_eq!(status.ffi(), ffi);
        if stop.is_ok() {
            assert_eq!(state.status, Stopping, "{status:?}");
            // A second stop waits for the same one: a second request would abort it.
            assert_eq!(state.stop(budget), Ok(()));
            assert_eq!(sent(&mut stops), Vec::<Duration>::new(), "{status:?}");
        }
    }
}

#[test]
fn the_first_round_decides_what_start_returns() {
    // Running: Ok, and the last error is cleared.
    let (mut state, _stops) = fresh(Status::Starting);
    state.last_error = Some("Config: an earlier start".into());
    state.event(Event::Phase(Phase::Running));
    assert_eq!(
        (first(&state), state.status, state.last_error),
        (Some(Code::Ok), Status::Running, None)
    );

    // A stop first: Cancelled at once, whatever the round does next.
    let (mut state, _stops) = fresh(Status::Starting);
    assert_eq!(state.stop(Duration::ZERO), Ok(()));
    assert_eq!(first(&state), Some(Code::Cancelled));
    state.event(Event::Phase(Phase::Running));
    assert_eq!(state.status, Status::Stopping);
    let failed = "startup failed: service http: no device";
    state.ended(end(Code::StartupFailed, Some(failed)), true);
    assert_eq!(first(&state), Some(Code::Cancelled));
    assert_eq!(state.status, Status::Idle);
    assert_eq!(
        state.last_error.unwrap(),
        format!("StartupFailed: {failed}")
    );

    // The round ends before it runs, or nothing starts: its code.
    for (code, message) in [
        (
            Code::StartupFailed,
            "startup failed: panic: composition root",
        ),
        (
            Code::Config,
            "invalid configuration: a (host --set a): no such key",
        ),
        (Code::CantCreate, "cannot write the log files: denied"),
    ] {
        let (mut state, _stops) = fresh(Status::Starting);
        state.event(Event::Phase(Phase::Stopping));
        assert_eq!(state.status, Status::Starting, "the start reports it");
        state.ended(end(code, Some(message)), true);
        assert_eq!((first(&state), state.status), (Some(code), Status::Idle));
        assert_eq!(
            state.last_error.unwrap(),
            format!("{}: {message}", code.name())
        );
    }

    // No result within the guard: the services are stopped at once.
    let (mut state, mut stops) = fresh(Status::Starting);
    assert_eq!(state.overdue(), Code::StartupFailed);
    assert_eq!(state.status, Status::Stopping);
    assert_eq!(sent(&mut stops), [Duration::ZERO]);
    assert!(
        state
            .last_error
            .unwrap()
            .starts_with("StartupFailed: no result within")
    );
}

#[test]
fn rounds_restart_in_the_process_until_ten_failures_in_a_row() {
    // A fault: restarting while the round stops, then the failure, then a new round.
    let (mut state, _stops) = running();
    state.event(Event::Phase(Phase::Stopping));
    assert_eq!(state.status, Status::Restarting);
    let fault = Failure::new(
        Code::Fault,
        "stopped after a fault: service http: device gone",
    );
    state.event(Event::Failed(&fault));
    assert_eq!(state.status, Status::Restarting);
    let shown = "Fault: stopped after a fault: service http: device gone";
    assert_eq!(state.last_error.as_deref(), Some(shown));
    state.event(Event::Rebuilt);
    assert_eq!(state.status, Status::Starting);
    // A rebuilt round that fails before it is ready restarts too.
    state.event(Event::Phase(Phase::Stopping));
    assert_eq!(state.status, Status::Restarting);
    state.event(Event::Rebuilt);
    state.event(Event::Phase(Phase::Running));
    // The last failure stays, for diagnosis.
    assert_eq!(state.status, Status::Running);
    assert_eq!(state.last_error.as_deref(), Some(shown));

    // A round that cannot run at all fails without stopping.
    let (mut state, _stops) = running();
    state.event(Event::Phase(Phase::Stopping));
    state.event(Event::Rebuilt);
    state.event(Event::Failed(&Failure::new(
        Code::StartupFailed,
        "no services",
    )));
    assert_eq!(state.status, Status::Restarting);

    // Ten failures in a row: the loop gives up.
    let gave_up = "gave up after 10 failures in a row; the last: stopped after a fault";
    state.ended(end(Code::Fault, Some(gave_up)), true);
    assert_eq!(state.status, Status::Failed);
    assert_eq!(
        state.last_error.as_deref(),
        Some(format!("Fault: {gave_up}").as_str())
    );
    assert_eq!(state.may_start(), Ok(()));
    assert_eq!(state.stop(Duration::ZERO), Err(Code::NotRunning));

    // Every service finished on its own: idle, not failed.
    let (mut state, _stops) = running();
    state.ended(end(Code::Ok, None), true);
    assert_eq!((state.status, state.last_error), (Status::Idle, None));
}

#[test]
fn what_stop_returns() {
    let cases = [
        (end(Code::Ok, None), true, Code::Ok, None),
        // A service failed while stopping: the stop still succeeded.
        (
            end(
                Code::Fault,
                Some("stopped after a fault: service http: flush"),
            ),
            true,
            Code::Ok,
            Some("Fault: stopped after a fault: service http: flush"),
        ),
        (
            end(Code::StopTimedOut, Some("stop timed out\nabandoned: http")),
            true,
            Code::StopTimedOut,
            Some("StopTimedOut: stop timed out\nabandoned: http"),
        ),
        // The last logs did not reach the files in time.
        (end(Code::Ok, None), false, Code::StopTimedOut, None),
    ];
    for (end, flushed, returned, last_error) in cases {
        let (mut state, _stops) = running();
        assert_eq!(state.stop(Duration::from_secs(1)), Ok(()));
        state.ended(end, flushed);
        assert_eq!(state.results.ended.get(), Some(&returned));
        assert_eq!(state.status, Status::Idle);
        assert_eq!(state.last_error.as_deref(), last_error);
    }
}

// Callers of one start that get the lock back only after the next start began. A scripted
// thread stands in for `main`, and the test changes the state "quietly", without waking the
// callers that wait: the scheduler may leave a woken caller without the lock that long.

type Stops = mpsc::UnboundedReceiver<StopReason>;
type Step = Box<dyn FnOnce(&Shared, u64, &mut Stops) + Send>;
type Steps = ((usize, u64), Receiver<Step>);

/// The steps of the scripted threads, by host and start.
static SCRIPTS: Mutex<Vec<Steps>> = Mutex::new(Vec::new());

/// How long a caller may take to return before it counts as stuck.
const STUCK: Duration = Duration::from_secs(10);

/// The thread of a start, in place of `main`: it takes the steps of its script in order.
fn scripted(shared: Arc<Shared>, run: u64, _: Options, mut stops: Stops) {
    let key = (Arc::as_ptr(&shared) as usize, run);
    let steps = {
        let mut scripts = SCRIPTS.lock().unwrap();
        let at = scripts.iter().position(|(k, _)| *k == key);
        scripts.remove(at.expect("a script for the start")).1
    };
    for step in steps {
        step(&shared, run, &mut stops);
    }
}

/// The steps of start `run`; its thread ends once the script is dropped.
struct Script(Sender<Step>);

impl Script {
    fn new(host: &Host, run: u64) -> Script {
        let (steps, taken) = channel();
        let key = (Arc::as_ptr(&host.shared) as usize, run);
        SCRIPTS.lock().unwrap().push((key, taken));
        Script(steps)
    }

    fn then(&self, step: impl FnOnce(&Shared, u64, &mut Stops) + Send + 'static) {
        self.0.send(Box::new(step)).unwrap();
    }

    /// The first round runs.
    fn runs(&self) {
        self.then(|shared, run, _| {
            shared.update(run, |state| state.event(Event::Phase(Phase::Running)));
        });
    }

    /// The thread ends with `code`.
    fn ends(self, code: Code) {
        self.then(move |shared, run, _| {
            shared.update(run, |state| state.ended(end(code, None), true));
        });
    }

    /// The thread ends quietly with `code`, after the stop request when `stopped`; returns once
    /// it has.
    fn ends_quietly(self, code: Code, stopped: bool) {
        let (done, ended) = channel();
        self.then(move |shared, _, stops| {
            if stopped {
                stops.blocking_recv().expect("a stop request");
            }
            quietly(shared, |state| state.ended(end(code, None), true));
            done.send(()).unwrap();
        });
        ended.recv().unwrap();
    }
}

/// Changes the state without waking the callers that wait: they get the lock back only after
/// what the test does next.
fn quietly(shared: &Shared, change: impl FnOnce(&mut State)) {
    change(&mut shared.state());
}

/// Calls `call` on another thread; the receiver gets what it returns.
fn aside(host: &Arc<Host>, call: impl FnOnce(&Host) -> Code + Send + 'static) -> Receiver<Code> {
    let (host, (returns, returned)) = (Arc::clone(host), channel());
    thread::spawn(move || returns.send(call(&host)));
    returned
}

/// A host with scripted threads, and its start 1 waiting on another thread.
fn starting() -> (Arc<Host>, Script, Receiver<Code>) {
    let host = Arc::new(Host {
        name: "scripted",
        main: scripted,
        shared: Arc::default(),
    });
    let one = Script::new(&host, 1);
    let start = aside(&host, |host| host.start(&[]));
    (host, one, start)
}

/// A host with scripted threads, and its start 1 running.
fn started() -> (Arc<Host>, Script) {
    let (host, one, start) = starting();
    one.runs();
    assert_eq!(start.recv_timeout(STUCK), Ok(Code::Ok));
    (host, one)
}

/// Waits until `callers` callers wait for what the last start comes to.
fn waiting(host: &Host, callers: usize) {
    while Arc::strong_count(&host.shared.state().results) < 1 + callers {
        thread::yield_now();
    }
}

#[test]
fn a_start_returns_what_its_own_first_round_came_to() {
    // Start 1 fails; its caller gets the lock back once start 2 runs, and does not wait for
    // start 2's thread.
    let (host, one, start) = starting();
    one.ends_quietly(Code::Config, false);
    let two = Script::new(&host, 2);
    two.runs();
    assert_eq!(host.start(&[]), Code::Ok);
    assert_eq!(start.recv_timeout(STUCK), Ok(Code::Config));
    two.ends(Code::Ok);

    // ... or once start 2 failed too, with another code.
    let (host, one, start) = starting();
    one.ends_quietly(Code::Config, false);
    Script::new(&host, 2).ends(Code::StartupFailed);
    assert_eq!(host.start(&[]), Code::StartupFailed);
    assert_eq!(start.recv_timeout(STUCK), Ok(Code::Config));

    // A stop cancels start 1, whose thread then ends; start 2 runs before start 1's caller gets
    // the lock back.
    let (host, one, start) = starting();
    one.then(|shared, _, _| {
        quietly(shared, |state| {
            assert_eq!(state.stop(Duration::ZERO), Ok(()))
        })
    });
    one.ends_quietly(Code::StartupFailed, true);
    let two = Script::new(&host, 2);
    two.runs();
    assert_eq!(host.start(&[]), Code::Ok);
    assert_eq!(start.recv_timeout(STUCK), Ok(Code::Cancelled));
    two.ends(Code::Ok);
}

#[test]
fn a_stop_returns_what_its_own_start_came_to() {
    // Start 1 stops; the stop's caller gets the lock back once start 2 runs, and does not wait
    // for start 2's thread.
    let (host, one) = started();
    let stop = aside(&host, |host| host.stop(Duration::from_secs(60)));
    one.ends_quietly(Code::Ok, true);
    let two = Script::new(&host, 2);
    two.runs();
    assert_eq!(host.start(&[]), Code::Ok);
    assert_eq!(stop.recv_timeout(STUCK), Ok(Code::Ok));
    two.ends(Code::Ok);

    // ... or once start 2 failed: what start 2 came to does not replace what start 1 came to,
    // for any of the stops that wait.
    let (host, one) = started();
    let stops = [(); 2].map(|()| aside(&host, |host| host.stop(Duration::from_secs(60))));
    waiting(&host, 2);
    one.ends_quietly(Code::Ok, true);
    Script::new(&host, 2).ends(Code::Config);
    assert_eq!(host.start(&[]), Code::Config);
    for stop in stops {
        assert_eq!(stop.recv_timeout(STUCK), Ok(Code::Ok));
    }
}
