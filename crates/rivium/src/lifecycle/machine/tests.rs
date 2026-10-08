//! The rule table of the lifecycle design, one test per cell or group of cells: each state is
//! reached through the inputs the driver would give, and the effects are compared as words.

use std::borrow::Cow;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rivium_error::{BError, Error, kinds};

use super::{Effect, Exit, Input, State, Timer};
use crate::lifecycle::health::Phase;
use crate::lifecycle::service::ServiceKind::{self, Background, Frontline};
use crate::lifecycle::supervisor::{Outcome, StopReason};

const HTTP: usize = 0;
const LOG: usize = 1;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// `http` (frontline) and `log` (background), started.
fn starting() -> State {
    with(&[("http", Frontline), ("log", Background)])
}

fn with(services: &[(&'static str, ServiceKind)]) -> State {
    let services = (services.iter())
        .map(|(name, kind)| (Cow::Borrowed(*name), *kind))
        .collect();
    let mut state = State::new(services, secs(30), secs(3));
    state.start();
    state
}

fn running() -> State {
    let mut state = starting();
    state.step(Input::Ready(HTTP));
    state.step(Input::Ready(LOG));
    state
}

fn sigterm() -> StopReason {
    StopReason::Signal {
        name: "SIGTERM",
        number: 15,
    }
}

fn sigint() -> StopReason {
    StopReason::Signal {
        name: "SIGINT",
        number: 2,
    }
}

fn host(budget: Duration) -> StopReason {
    StopReason::Host { budget }
}

/// A running run stopping for each reason there is.
fn stopping_for(reason: &str) -> State {
    let mut state = running();
    match reason {
        "stop" => state.step(Input::Stop(sigterm())),
        "restart" => state.step(Input::Restart("configuration changed".into())),
        "fault" => state.step(failed(LOG)),
        _ => unreachable!("{reason}"),
    };
    state
}

fn error(context: &'static str) -> BError {
    Error::explain(kinds::UNAVAILABLE, context)
}

fn failed(i: usize) -> Input {
    Input::Ended(i, Exit::Failed(error("broken")))
}

/// Each way service `i` can fail, as a fresh input.
fn failures(i: usize) -> [Input; 3] {
    [
        failed(i),
        Input::Ended(i, Exit::Panicked("boom".into())),
        Input::ChildFailed(i, "flush".into(), error("disk full")),
    ]
}

/// The effects as short words, so the expectations stay readable.
fn words(effects: &[Effect]) -> Vec<String> {
    let name = |i: usize| ["http", "log", "db"][i];
    (effects.iter())
        .map(|effect| match effect {
            Effect::Phase(phase, _) => phase.to_string(),
            Effect::Cancel(i) => format!("cancel {}", name(*i)),
            Effect::StopFrontline => "stop frontline".into(),
            Effect::StopBackground => "stop background".into(),
            Effect::StartTimer(timer, after) => {
                let timer = format!("{timer:?}").to_lowercase();
                format!(
                    "timer {timer} {}",
                    crate::config::de::format_duration(*after)
                )
            }
            Effect::Finish(outcome) => format!("finish {}", outcome.name()),
        })
        .collect()
}

fn step(state: &mut State, input: Input) -> Vec<String> {
    words(&state.step(input))
}

const NONE: [&str; 0] = [];

/// Every service drains; returns the outcome.
fn end(state: &mut State) -> Outcome {
    let mut effects = Vec::new();
    for i in [HTTP, LOG] {
        effects.extend(state.step(Input::Drained(i)));
    }
    match effects.pop() {
        Some(Effect::Finish(outcome)) => outcome,
        other => panic!("the run did not finish: {other:?}"),
    }
}

fn outcome_of(effects: Vec<Effect>) -> Outcome {
    match effects.into_iter().last() {
        Some(Effect::Finish(outcome)) => outcome,
        other => panic!("the run did not finish: {other:?}"),
    }
}

#[test]
fn a_run_starts_with_the_startup_timer() {
    let services = vec![("http".into(), Frontline)];
    let mut state = State::new(services, secs(30), secs(3));
    assert_eq!(words(&state.start()), ["starting", "timer startup 30s"]);
}

#[test]
fn no_services_or_two_with_one_name_cannot_run() {
    for (services, why) in [
        (vec![], "there are no services"),
        (
            vec![("http".into(), Frontline), ("http".into(), Background)],
            "two services are named http",
        ),
    ] {
        let mut state = State::new(services, secs(30), secs(3));
        let effects = state.start();
        assert_eq!(
            words(&effects),
            ["starting", "stopped", "finish startup-failed"]
        );
        let Outcome::StartupFailed(error) = outcome_of(effects) else {
            panic!("not a startup failure");
        };
        assert_eq!(
            (error.etype().name(), error.to_string()),
            ("InvalidServices", why.to_string())
        );
    }
}

// Ready(i)

#[test]
fn starting_runs_once_every_service_is_ready() {
    let mut state = starting();
    assert_eq!(step(&mut state, Input::Ready(HTTP)), NONE);
    assert_eq!(step(&mut state, Input::Ready(HTTP)), NONE, "ready twice");
    assert_eq!(step(&mut state, Input::Ready(LOG)), ["running"]);
}

#[test]
fn ready_while_running_or_stopping_changes_nothing() {
    let mut state = running();
    assert_eq!(step(&mut state, Input::Ready(LOG)), NONE);
    let mut state = starting();
    state.step(Input::Ready(HTTP));
    state.step(Input::Stop(sigterm()));
    assert_eq!(step(&mut state, Input::Ready(LOG)), NONE);
    // `log` did not become ready: returning now while stopping is no failure.
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
}

// A failure of service i

#[test]
fn a_failure_before_the_service_is_ready_fails_the_startup() {
    let returned = || Input::Ended(HTTP, Exit::Returned);
    for input in failures(HTTP).into_iter().chain([returned()]) {
        let mut state = starting();
        state.step(Input::Ready(LOG));
        assert_eq!(
            step(&mut state, input),
            [
                "stopping",
                "timer deadline 3s",
                "stop frontline",
                "cancel http"
            ]
        );
        assert_eq!(step(&mut state, Input::Drained(HTTP)), ["stop background"]);
        assert!(matches!(end(&mut state), Outcome::StartupFailed(_)));
    }
}

#[test]
fn a_failure_of_a_ready_service_while_others_start_is_a_fault() {
    for input in failures(LOG) {
        let mut state = starting();
        state.step(Input::Ready(LOG));
        assert_eq!(
            step(&mut state, input),
            [
                "stopping",
                "timer deadline 3s",
                "stop frontline",
                "cancel log"
            ]
        );
        assert!(matches!(end(&mut state), Outcome::Fault(_)));
    }
}

#[test]
fn a_failure_while_running_is_a_fault() {
    // A frontline service that returns before it is asked to stop fails, even with `Ok`.
    let returned = || Input::Ended(HTTP, Exit::Returned);
    for input in failures(HTTP).into_iter().chain([returned()]) {
        let mut state = running();
        assert_eq!(
            step(&mut state, input),
            [
                "stopping",
                "timer deadline 3s",
                "stop frontline",
                "cancel http"
            ]
        );
        assert!(matches!(end(&mut state), Outcome::Fault(_)));
    }
}

#[test]
fn a_failure_while_stopping_for_a_stop_or_restart_makes_it_a_fault() {
    for reason in ["stop", "restart"] {
        for input in failures(LOG) {
            let mut state = stopping_for(reason);
            assert_eq!(step(&mut state, input), ["cancel log"], "{reason}");
            assert!(matches!(end(&mut state), Outcome::Fault(_)), "{reason}");
        }
    }
}

#[test]
fn a_failure_while_stopping_for_a_failure_keeps_the_first() {
    let mut state = starting();
    state.step(Input::Ready(LOG));
    state.step(Input::Ended(HTTP, Exit::Failed(error("bind"))));
    assert_eq!(step(&mut state, failed(LOG)), ["cancel log"]);
    let Outcome::StartupFailed(first) = end(&mut state) else {
        panic!("the first failure was a startup failure");
    };
    assert_eq!(format!("{first:#}"), "service http: bind");

    let mut state = stopping_for("fault");
    assert_eq!(step(&mut state, failed(HTTP)), ["cancel http"]);
    let Outcome::Fault(first) = end(&mut state) else {
        panic!("the first failure was a fault");
    };
    assert_eq!(format!("{first:#}"), "service log: broken");
}

#[test]
fn only_the_first_failure_of_a_service_counts() {
    let mut state = running();
    state.step(Input::ChildFailed(LOG, "flush".into(), error("disk full")));
    assert_eq!(step(&mut state, failed(LOG)), NONE);
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
    let Outcome::Fault(error) = end(&mut state) else {
        panic!("not a fault");
    };
    assert_eq!(format!("{error:#}"), "service log, task flush: disk full");
}

#[test]
fn the_outcome_says_where_the_error_came_from() {
    let cases = [
        (failed(HTTP), "service http: broken", "Unavailable"),
        (
            Input::Ended(HTTP, Exit::Panicked("boom".into())),
            "service http: panicked: boom",
            "ServicePanicked",
        ),
        (
            Input::Ended(HTTP, Exit::Returned),
            "service http: returned before it was asked to stop",
            "ServiceExited",
        ),
        (
            Input::ChildFailed(HTTP, "conn".into(), error("reset")),
            "service http, task conn: reset",
            "Unavailable",
        ),
    ];
    for (input, shown, kind) in cases {
        let mut state = running();
        state.step(input);
        let Outcome::Fault(error) = end(&mut state) else {
            panic!("not a fault: {shown}");
        };
        assert_eq!(
            (format!("{error:#}"), error.etype().name()),
            (shown.into(), kind)
        );
    }
}

// Ended(i, Returned), not a failure

#[test]
fn a_ready_background_service_may_finish() {
    let mut state = starting();
    state.step(Input::Ready(LOG));
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
    assert_eq!(step(&mut state, Input::Drained(LOG)), NONE);
    assert_eq!(step(&mut state, Input::Ready(HTTP)), ["running"]);

    let mut state = running();
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
    assert_eq!(step(&mut state, Input::Drained(LOG)), NONE);

    // While stopping, returning is no failure, whatever the service.
    let mut state = stopping_for("stop");
    assert_eq!(step(&mut state, Input::Ended(HTTP, Exit::Returned)), NONE);
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
    assert!(matches!(end(&mut state), Outcome::Stopped));
}

// Drained(i)

#[test]
fn a_run_whose_services_all_finish_is_stopped() {
    let mut state = with(&[("a", Background), ("b", Background)]);
    state.step(Input::Ready(0));
    state.step(Input::Ready(1));
    state.step(Input::Ended(0, Exit::Returned));
    assert_eq!(step(&mut state, Input::Drained(0)), NONE);
    state.step(Input::Ended(1, Exit::Returned));
    assert_eq!(
        step(&mut state, Input::Drained(1)),
        ["stopped", "finish stopped"]
    );
}

#[test]
fn stopping_stops_background_services_after_the_frontline_and_ends_when_nothing_runs() {
    let mut state = running();
    assert_eq!(
        step(&mut state, Input::Stop(sigterm())),
        ["stopping", "timer deadline 3s", "stop frontline"]
    );
    assert_eq!(step(&mut state, Input::Ended(HTTP, Exit::Returned)), NONE);
    // Tasks of `http` still run until it is drained.
    assert_eq!(step(&mut state, Input::Drained(HTTP)), ["stop background"]);
    assert_eq!(step(&mut state, Input::Ended(LOG, Exit::Returned)), NONE);
    assert_eq!(
        step(&mut state, Input::Drained(LOG)),
        ["stopped", "finish stopped"]
    );
}

#[test]
fn background_services_stop_at_once_when_no_frontline_service_runs() {
    let mut state = with(&[("log", Background), ("db", Background)]);
    state.step(Input::Ready(0));
    state.step(Input::Ready(1));
    assert_eq!(
        step(&mut state, Input::Stop(sigterm())),
        [
            "stopping",
            "timer deadline 3s",
            "stop frontline",
            "stop background"
        ]
    );
}

// The host's stop requests

#[test]
fn the_first_stop_stops_the_run_by_its_budget() {
    for (stop, deadline) in [
        (sigterm(), "timer deadline 3s"),
        (host(secs(1)), "timer deadline 1s"),
        (host(secs(10)), "timer deadline 3s"),
        (host(Duration::ZERO), "timer deadline 0s"),
    ] {
        for mut state in [starting(), running()] {
            assert_eq!(
                step(&mut state, Input::Stop(stop.clone())),
                ["stopping", deadline, "stop frontline"]
            );
        }
    }
}

#[test]
fn a_stop_while_stopping_keeps_a_failure_and_wins_over_a_restart() {
    let mut state = stopping_for("restart");
    assert_eq!(step(&mut state, Input::Stop(sigterm())), NONE);
    assert!(matches!(end(&mut state), Outcome::Stopped));

    let mut state = stopping_for("fault");
    assert_eq!(
        step(&mut state, Input::Stop(host(secs(1)))),
        ["timer deadline 1s"]
    );
    assert!(matches!(end(&mut state), Outcome::Fault(_)));

    let mut state = starting();
    state.step(failed(HTTP));
    state.step(Input::Stop(sigterm()));
    assert!(matches!(end(&mut state), Outcome::StartupFailed(_)));
}

#[test]
fn the_second_stop_aborts_the_run() {
    for mut state in [starting(), running(), stopping_for("fault")] {
        state.step(Input::Stop(sigterm()));
        let effects = state.step(Input::Stop(sigint()));
        assert_eq!(words(&effects), ["stopped", "finish aborted"]);
        let Outcome::Aborted(stop) = outcome_of(effects) else {
            panic!("not aborted");
        };
        assert_eq!(stop, sigint());
    }
}

// Restart(why)

#[test]
fn a_restart_request_stops_the_run_unless_it_is_stopping() {
    for mut state in [starting(), running()] {
        assert_eq!(
            step(&mut state, Input::Restart("configuration changed".into())),
            ["stopping", "timer deadline 3s", "stop frontline"]
        );
        let Outcome::RestartRequested(why) = end(&mut state) else {
            panic!("not a restart");
        };
        assert_eq!(why, "configuration changed");
    }
    for reason in ["stop", "fault"] {
        let mut state = stopping_for(reason);
        assert_eq!(step(&mut state, Input::Restart("again".into())), NONE);
    }
}

// Timer(Startup)

#[test]
fn the_startup_timer_fails_a_run_that_is_not_ready() {
    let mut state = starting();
    state.step(Input::Ready(LOG));
    assert_eq!(
        step(&mut state, Input::Timer(Timer::Startup)),
        ["stopping", "timer deadline 3s", "stop frontline"]
    );
    let Outcome::StartupFailed(error) = end(&mut state) else {
        panic!("not a startup failure");
    };
    assert_eq!(
        (error.etype().name(), error.to_string()),
        ("StartupTimedOut", "not ready within 30s: http".to_string())
    );
    for mut state in [running(), stopping_for("stop")] {
        assert_eq!(step(&mut state, Input::Timer(Timer::Startup)), NONE);
    }
}

// Timer(Deadline)

#[test]
fn the_deadline_ends_a_stopping_run_by_its_reason() {
    for (reason, outcome) in [
        ("stop", "stop-timed-out"),
        ("restart", "restart-requested"),
        ("fault", "fault"),
    ] {
        let mut state = stopping_for(reason);
        assert_eq!(
            step(&mut state, Input::Timer(Timer::Deadline)),
            ["stopped".to_string(), format!("finish {outcome}")]
        );
    }
    let mut state = starting();
    state.step(failed(HTTP));
    assert_eq!(
        step(&mut state, Input::Timer(Timer::Deadline)),
        ["stopped", "finish startup-failed"]
    );
}

#[test]
fn a_deadline_before_the_run_is_stopping_is_logged_and_ignored() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.subscriber(), || {
        for mut state in [starting(), running()] {
            assert_eq!(step(&mut state, Input::Timer(Timer::Deadline)), NONE);
        }
    });
    assert_eq!(logs.lines("ERROR").len(), 2);
}

#[test]
fn nothing_counts_after_the_end() {
    let mut state = stopping_for("stop");
    end(&mut state);
    for input in [
        Input::Stop(sigint()),
        Input::Restart("again".into()),
        failed(HTTP),
        Input::Timer(Timer::Deadline),
    ] {
        assert_eq!(step(&mut state, input), NONE);
    }
}

// Logging: once, where the failure is handled

/// JSON lines written while a subscriber of these logs is the default.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        let logs = self.clone();
        tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_writer(move || logs.clone())
            .finish()
    }

    /// The lines of this level as `message error.type service.name task.name`.
    fn lines(&self, level: &str) -> Vec<String> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        (text.lines())
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["level"] == level)
            .map(|event| {
                let field = |name: &str| event[name].as_str().unwrap_or("-").to_string();
                let fields = ["message", "error.type", "service.name", "task.name"];
                fields.map(field).join(" ")
            })
            .collect()
    }
}

impl io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_failure_that_stops_the_run_is_an_error_and_later_ones_warnings() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.subscriber(), || {
        let mut state = with(&[("http", Frontline), ("log", Background), ("db", Background)]);
        state.step(Input::Ready(0));
        state.step(Input::ChildFailed(1, "flush".into(), error("disk full")));
        state.step(Input::Ended(1, Exit::Failed(error("cancelled"))));
        state.step(Input::Ended(0, Exit::Panicked("boom".into())));
        state.step(Input::Ended(2, Exit::Returned));
    });
    assert_eq!(
        logs.lines("ERROR"),
        ["service failed Unavailable log flush"]
    );
    assert_eq!(
        logs.lines("WARN"),
        [
            "service failed while the run was stopping ServicePanicked http -",
            // `db` was not ready, but returning while stopping is no failure.
        ]
    );

    let logs = Logs::default();
    tracing::subscriber::with_default(logs.subscriber(), || {
        // A stop, then a failure: the failure becomes the reason and is the error.
        let mut state = stopping_for("stop");
        state.step(failed(LOG));
        state.step(failed(HTTP));
        // The machine's own failure.
        let mut state = starting();
        state.step(Input::Timer(Timer::Startup));
    });
    assert_eq!(
        logs.lines("ERROR"),
        [
            "service failed Unavailable log -",
            "startup failed StartupTimedOut - -"
        ]
    );
    assert_eq!(
        logs.lines("WARN"),
        ["service failed while the run was stopping Unavailable http -"]
    );
}

#[test]
fn phases_only_move_forward() {
    let mut state = running();
    let mut seen = vec![Phase::Running];
    for input in [
        Input::Stop(sigterm()),
        Input::Drained(HTTP),
        Input::Drained(LOG),
    ] {
        for effect in state.step(input) {
            if let Effect::Phase(phase, _) = effect {
                seen.push(phase);
            }
        }
    }
    assert_eq!(seen, [Phase::Running, Phase::Stopping, Phase::Stopped]);
}
