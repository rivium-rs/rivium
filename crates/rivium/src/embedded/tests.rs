//! The rules of the embedded host's state, driven directly: every status against every call,
//! the first round's results, restarts in the process and giving up, which takes ten failures
//! and about four minutes of backoff for real. `tests/embedded.rs` runs a real host.

use std::time::Duration;

use tokio::sync::mpsc;

use super::{State, Status};
use crate::Code;
use crate::host::{End, Event, Failure};
use crate::lifecycle::StopReason;
use crate::lifecycle::health::Phase;

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
        (state.first, state.status, state.last_error),
        (Some(Code::Ok), Status::Running, None)
    );

    // A stop first: Cancelled at once, whatever the round does next.
    let (mut state, _stops) = fresh(Status::Starting);
    assert_eq!(state.stop(Duration::ZERO), Ok(()));
    assert_eq!(state.first, Some(Code::Cancelled));
    state.event(Event::Phase(Phase::Running));
    assert_eq!(state.status, Status::Stopping);
    let failed = "startup failed: service http: no device";
    state.ended(1, end(Code::StartupFailed, Some(failed)), true);
    assert_eq!(state.first, Some(Code::Cancelled));
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
        state.ended(1, end(code, Some(message)), true);
        assert_eq!((state.first, state.status), (Some(code), Status::Idle));
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
    state.ended(1, end(Code::Fault, Some(gave_up)), true);
    assert_eq!(state.status, Status::Failed);
    assert_eq!(
        state.last_error.as_deref(),
        Some(format!("Fault: {gave_up}").as_str())
    );
    assert_eq!(state.may_start(), Ok(()));
    assert_eq!(state.stop(Duration::ZERO), Err(Code::NotRunning));

    // Every service finished on its own: idle, not failed.
    let (mut state, _stops) = running();
    state.ended(1, end(Code::Ok, None), true);
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
        state.ended(1, end, flushed);
        assert_eq!(state.ended, Some((1, returned)));
        assert_eq!(state.status, Status::Idle);
        assert_eq!(state.last_error.as_deref(), last_error);
    }
}
