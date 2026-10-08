//! The supervisor end to end, through the public `lifecycle` API: scripted services and stop
//! requests on paused time. The rule table itself is tested cell by cell inside the crate;
//! these tests check that the driver carries it out: stop order, deadlines and budgets, tasks,
//! drop-safety, blocking work, restarts, `periodic`, the logs, and that the outcome does not
//! depend on scheduling.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rivium::lifecycle::{Outcome, StopHandle, StopReason, Supervisor, stop_channel};
use rivium::{
    BoxFuture, Error, Result, Service, ServiceContext, ServiceKind, StopSignal, error::kinds,
};
use rivium_test::{ScriptedService, Step, capture_logs};
use tokio::time::Instant;

use ServiceKind::{Background, Frontline};

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn millis(n: u64) -> Duration {
    Duration::from_millis(n)
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

fn http(steps: Vec<Step>) -> ScriptedService {
    ScriptedService::new("http", Frontline, steps)
}

fn log(steps: Vec<Step>) -> ScriptedService {
    ScriptedService::new("log", Background, steps)
}

fn serving() -> Vec<Step> {
    vec![Step::Ready, Step::UntilStopped]
}

fn supervisor(services: Vec<Box<dyn Service>>) -> Supervisor {
    (services.into_iter()).fold(Supervisor::new(secs(30), secs(3)), Supervisor::with)
}

fn scripted(services: Vec<ScriptedService>) -> Vec<Box<dyn Service>> {
    (services.into_iter())
        .map(|service| Box::new(service) as Box<dyn Service>)
        .collect()
}

/// Sends each stop request at its time after now.
fn stops_at(stops: Vec<(Duration, StopReason)>) -> rivium::lifecycle::StopReceiver {
    let (handle, receiver) = stop_channel();
    let start = Instant::now();
    tokio::spawn(async move {
        for (at, stop) in stops {
            tokio::time::sleep_until(start + at).await;
            handle.stop(stop);
        }
        // Keep the handle, so the run never sees the channel close.
        std::future::pending::<StopHandle>().await
    });
    receiver
}

async fn run(services: Vec<ScriptedService>, stops: Vec<(Duration, StopReason)>) -> Outcome {
    ended(supervisor(scripted(services)).run(stops_at(stops))).await
}

/// The outcome of a run; a run that never ends fails the test rather than hang it.
async fn ended(run: impl Future<Output = Outcome>) -> Outcome {
    let within = secs(7_200);
    let outcome = tokio::time::timeout(within, run).await;
    outcome.unwrap_or_else(|_| panic!("the run did not end within {within:?}"))
}

fn name(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Stopped => "stopped",
        Outcome::StopTimedOut => "stop-timed-out",
        Outcome::StartupFailed(_) => "startup-failed",
        Outcome::Fault(_) => "fault",
        Outcome::RestartRequested(_) => "restart-requested",
        Outcome::Aborted(_) => "aborted",
        _ => "other",
    }
}

fn error_of(outcome: &Outcome) -> String {
    match outcome {
        Outcome::StartupFailed(error) | Outcome::Fault(error) => format!("{error:#}"),
        other => panic!("no error: {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_stop_request_stops_frontline_services_before_background_ones() {
    let front = http(vec![Step::Ready, Step::IgnoreStop(secs(1))]);
    let back = log(serving());
    let (front_seen, back_seen) = (front.journal(), back.journal());
    let start = Instant::now();
    let watcher = tokio::spawn(async move {
        // Stopped at 1s; `http` needs one more second.
        tokio::time::sleep_until(start + millis(1_500)).await;
        (front_seen.entries(), back_seen.entries())
    });
    let outcome = run(vec![front, back], vec![(secs(1), sigterm())]).await;
    assert_eq!(name(&outcome), "stopped");
    assert_eq!(start.elapsed(), secs(2));
    let (front_seen, back_seen) = watcher.await.unwrap();
    assert_eq!(front_seen, ["started", "ready", "asked to stop"]);
    assert_eq!(back_seen, ["started", "ready"]);
}

#[tokio::test(start_paused = true)]
async fn a_failure_before_ready_fails_the_startup() {
    let front = http(vec![Step::Sleep(secs(1)), Step::Fail("binding 0.0.0.0:80")]);
    let outcome = run(vec![front, log(serving())], vec![]).await;
    assert_eq!(name(&outcome), "startup-failed");
    assert_eq!(error_of(&outcome), "service http: binding 0.0.0.0:80");
}

#[tokio::test(start_paused = true)]
async fn not_ready_in_time_fails_the_startup() {
    let slow = http(vec![Step::Sleep(secs(60)), Step::Ready, Step::UntilStopped]);
    let start = Instant::now();
    let outcome = run(vec![slow, log(serving())], vec![]).await;
    assert_eq!(name(&outcome), "startup-failed");
    assert_eq!(error_of(&outcome), "not ready within 30s: http");
    // `http` ignores the stop while it sleeps: abandoned at the deadline.
    assert_eq!(start.elapsed(), secs(33));
}

#[tokio::test(start_paused = true)]
async fn a_panic_while_running_is_a_fault() {
    let back = log(vec![Step::Ready, Step::Sleep(secs(1)), Step::Panic("boom")]);
    let outcome = run(vec![http(serving()), back], vec![]).await;
    assert_eq!(name(&outcome), "fault");
    assert_eq!(error_of(&outcome), "service log: panicked: boom");
}

/// A service whose run future waits to be asked to stop, with tasks: `device-1` fails after a
/// second, `device-2` never ends and ignores the stop request.
fn collector() -> Box<dyn Service> {
    rivium::service("collector", Background, |ctx| async move {
        ctx.spawn("device-1", async {
            tokio::time::sleep(secs(1)).await;
            Error::e_explain(kinds::UNAVAILABLE, "no answer")
        });
        ctx.spawn("device-2", std::future::pending());
        ctx.ready();
        ctx.stopped().await;
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn a_failing_task_fails_its_service_at_once() {
    let front = http(serving());
    let seen = front.journal();
    let start = Instant::now();
    let watcher = tokio::spawn(async move {
        tokio::time::sleep_until(start + millis(1_001)).await;
        seen.entries()
    });
    let services = vec![Box::new(front) as Box<dyn Service>, collector()];
    let outcome = ended(supervisor(services).run(stops_at(vec![]))).await;
    assert_eq!(name(&outcome), "fault");
    assert_eq!(
        error_of(&outcome),
        "service collector, task device-1: no answer"
    );
    // The failure stopped the run at 1s, although `device-2` still ran; it was abandoned at
    // the deadline, 3s later.
    let seen = watcher.await.unwrap();
    assert_eq!(seen, ["started", "ready", "asked to stop", "returned"]);
    assert_eq!(start.elapsed(), secs(4));
}

#[tokio::test(start_paused = true)]
async fn a_later_failure_does_not_replace_the_first() {
    let logs = capture_logs();
    let front = http(vec![
        Step::Ready,
        Step::Sleep(secs(1)),
        Step::Fail("accept"),
    ]);
    let back = log(vec![Step::Ready, Step::UntilStopped, Step::Fail("flush")]);
    let outcome = run(vec![front, back], vec![]).await;
    assert_eq!(error_of(&outcome), "service http: accept");
    let failures: Vec<String> = (logs.events().iter())
        .filter(|event| event["level"] != "INFO")
        .map(|event| {
            format!(
                "{} {} {}",
                event["level"], event["message"], event["error.context"]
            )
        })
        .collect();
    assert_eq!(
        failures,
        [
            r#""ERROR" "service failed" "accept""#,
            r#""WARN" "service failed while the run was stopping" "flush""#,
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn services_still_running_at_the_deadline_are_abandoned() {
    let logs = capture_logs();
    // The task starts after `log` has: the list still follows the order of the services.
    let stuck = rivium::service("http", Frontline, |ctx| async move {
        ctx.ready();
        ctx.stopped().await;
        ctx.spawn("drain", std::future::pending());
        std::future::pending().await
    });
    let services: Vec<Box<dyn Service>> = vec![stuck, Box::new(log(serving()))];
    let start = Instant::now();
    let stops = stops_at(vec![(secs(1), sigterm())]);
    let outcome = ended(supervisor(services).run(stops)).await;
    assert_eq!(name(&outcome), "stop-timed-out");
    assert_eq!(start.elapsed(), secs(4));
    let outcome = &logs.with_message("outcome")[0];
    assert_eq!(
        (outcome["level"].as_str(), outcome["abandoned"].as_str()),
        (Some("WARN"), Some("http, http/drain, log"))
    );
}

/// A frontline service that records the deadline it sees when asked to stop, then needs
/// `stopping` to stop.
fn timed(seen: Arc<std::sync::Mutex<Option<Instant>>>, stopping: Duration) -> Box<dyn Service> {
    rivium::service("timed", Frontline, move |ctx| async move {
        ctx.ready();
        ctx.stopped().await;
        *seen.lock().unwrap() = ctx.deadline();
        tokio::time::sleep(stopping).await;
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn the_deadline_reaches_the_services_and_a_shorter_budget_brings_it_forward() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let start = Instant::now();
    let services = vec![timed(Arc::clone(&seen), secs(2))];
    let outcome = ended(supervisor(services).run(stops_at(vec![(secs(1), sigterm())]))).await;
    assert_eq!(name(&outcome), "stopped");
    assert_eq!(
        *seen.lock().unwrap(),
        Some(start + secs(4)),
        "stop_timeout after the stop"
    );

    // The host's budget is shorter than the stop timeout: it sets the deadline.
    let start = Instant::now();
    let host = StopReason::Host { budget: secs(1) };
    let services = vec![timed(Arc::clone(&seen), secs(2))];
    let outcome = ended(supervisor(services).run(stops_at(vec![(secs(1), host)]))).await;
    assert_eq!(name(&outcome), "stop-timed-out");
    assert_eq!(*seen.lock().unwrap(), Some(start + secs(2)));
    assert_eq!(start.elapsed(), secs(2));

    // Stopping for a fault, then the host stops with a shorter budget: the deadline moves
    // forward, and the fault stays the outcome.
    let start = Instant::now();
    let failing = log(vec![
        Step::Ready,
        Step::Sleep(secs(1)),
        Step::Fail("disk full"),
    ]);
    let host = StopReason::Host {
        budget: millis(500),
    };
    let mut services = vec![timed(Arc::clone(&seen), secs(60))];
    services.extend(scripted(vec![failing]));
    let outcome = ended(supervisor(services).run(stops_at(vec![(secs(2), host)]))).await;
    assert_eq!(name(&outcome), "fault");
    assert_eq!(start.elapsed(), millis(2_500));
}

#[tokio::test(start_paused = true)]
async fn a_second_stop_request_aborts_the_stop() {
    let stuck = http(vec![Step::Ready, Step::IgnoreStop(secs(3_600))]);
    let start = Instant::now();
    let stops = vec![(secs(1), sigterm()), (secs(2), sigint())];
    let outcome = run(vec![stuck, log(serving())], stops).await;
    assert!(
        matches!(outcome, Outcome::Aborted(ref stop) if *stop == sigint()),
        "{outcome:?}"
    );
    assert_eq!(start.elapsed(), secs(2));
}

#[tokio::test(start_paused = true)]
async fn a_restart_request_stops_the_run_gracefully() {
    let restarting = rivium::service("config", Background, |ctx| async move {
        ctx.ready();
        tokio::time::sleep(secs(1)).await;
        ctx.restarter().request("configuration changed");
        ctx.stopped().await;
        Ok(())
    });
    let mut services = scripted(vec![http(serving())]);
    services.push(restarting);
    let outcome = ended(supervisor(services).run(stops_at(vec![]))).await;
    assert!(
        matches!(outcome, Outcome::RestartRequested(ref why) if why == "configuration changed")
    );
}

/// Sends on drop: tells the test that a task's future was dropped.
struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _ = self.0.take().map(|done| done.send(()));
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_the_run_aborts_every_service_and_task() {
    let (service_done, service_dropped) = tokio::sync::oneshot::channel();
    let (task_done, task_dropped) = tokio::sync::oneshot::channel();
    let holder = rivium::service("holder", Frontline, move |ctx| async move {
        let task_guard = DropSignal(Some(task_done));
        ctx.spawn("held", async move {
            let _guard = task_guard;
            std::future::pending().await
        });
        let _guard = DropSignal(Some(service_done));
        ctx.ready();
        // Ignores every stop request.
        std::future::pending().await
    });
    let run = supervisor(vec![holder]).run(stops_at(vec![]));
    assert!(
        tokio::time::timeout(secs(10), run).await.is_err(),
        "runs until dropped"
    );
    // The run future is gone: its services and tasks are aborted.
    tokio::time::timeout(secs(1), service_dropped)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(secs(1), task_dropped)
        .await
        .unwrap()
        .unwrap();
}

/// A background service that hands out its stop signal, as to a blocking task.
fn watched(signal: Arc<std::sync::Mutex<Option<StopSignal>>>) -> Box<dyn Service> {
    rivium::service("log", Background, move |ctx| async move {
        *signal.lock().unwrap() = Some(ctx.stop_signal());
        ctx.ready();
        ctx.stopped().await;
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn the_end_of_the_run_is_a_stop_for_every_service() {
    // The run ends before `log` is asked to stop, while `http` ignores its stop: at the
    // deadline, at a second stop request, or when its future is dropped.
    let ends = [
        (vec![(secs(1), sigterm())], Some("stop-timed-out")),
        (
            vec![(secs(1), sigterm()), (secs(2), sigint())],
            Some("aborted"),
        ),
        (vec![], None),
    ];
    for (stops, outcome) in ends {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut services = scripted(vec![http(vec![Step::Ready, Step::IgnoreStop(secs(3_600))])]);
        services.push(watched(Arc::clone(&seen)));
        let run = supervisor(services).run(stops_at(stops));
        let ended = tokio::time::timeout(secs(10), run).await.ok();
        assert_eq!(ended.as_ref().map(name), outcome);
        let signal = seen.lock().unwrap().take().unwrap();
        // What a blocking task checks between calls.
        assert!(signal.is_stopping(), "{outcome:?}");
        tokio::time::timeout(secs(1), signal.stopped())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_blocking_task_cannot_be_aborted_and_is_abandoned_at_the_deadline() {
    // Real time: blocking threads hold up a paused clock.
    let logs = capture_logs();
    let blocking = log(vec![Step::Ready, Step::Block(secs(2)), Step::UntilStopped]);
    let supervisor = Supervisor::new(secs(30), millis(100)).with(Box::new(blocking));
    let (stop, stops) = stop_channel();
    stop.stop(sigterm());
    let start = std::time::Instant::now();
    let outcome = ended(supervisor.run(stops)).await;
    assert_eq!(name(&outcome), "stop-timed-out");
    assert!(start.elapsed() < secs(1), "{:?}", start.elapsed());
    let still = &logs.with_message("blocking tasks cannot be aborted and keep running")[0];
    assert_eq!(still["tasks"], "log/block");
    assert_eq!(logs.with_message("outcome")[0]["abandoned"], "log/block");
}

#[tokio::test(start_paused = true)]
async fn a_service_ends_only_with_its_last_task() {
    let job = rivium::service("job", Background, |ctx| async move {
        ctx.spawn("upload", async {
            tokio::time::sleep(secs(5)).await;
            Ok(())
        });
        ctx.ready();
        Ok(())
    });
    let start = Instant::now();
    let outcome = ended(supervisor(vec![job]).run(stops_at(vec![]))).await;
    // Every service finished on its own: the run is over once the upload is.
    assert_eq!(name(&outcome), "stopped");
    assert_eq!(start.elapsed(), secs(5));
}

#[tokio::test(start_paused = true)]
async fn periodic_ticks_skip_missed_ticks_and_a_failing_tick_fails_the_service() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ticks);
    let poller = rivium::periodic("poller", secs(1), move || {
        let tick = counted.fetch_add(1, Ordering::SeqCst);
        async move {
            // The third tick takes 2.5s: the ticks due at 3s and 4s are skipped.
            if tick == 2 {
                tokio::time::sleep(millis(2_500)).await;
            }
            match tick {
                5 => Error::e_explain(kinds::UNAVAILABLE, "device gone"),
                _ => Ok(()),
            }
        }
    });
    let start = Instant::now();
    let outcome = ended(supervisor(vec![poller]).run(stops_at(vec![]))).await;
    // Ticks at 0s, 1s, 2s (until 4.5s), 5s, 6s, and the failing one at 7s.
    assert_eq!(error_of(&outcome), "service poller: device gone");
    assert_eq!(
        (ticks.load(Ordering::SeqCst), start.elapsed()),
        (6, secs(7))
    );
}

#[tokio::test(start_paused = true)]
async fn a_stop_request_cancels_the_tick_in_progress() {
    let slow = rivium::periodic("slow", secs(1), || async {
        tokio::time::sleep(secs(3_600)).await;
        Ok(())
    });
    let start = Instant::now();
    let outcome = ended(supervisor(vec![slow]).run(stops_at(vec![(millis(500), sigterm())]))).await;
    assert_eq!(name(&outcome), "stopped");
    assert_eq!(start.elapsed(), millis(500));
}

#[tokio::test(start_paused = true)]
async fn each_phase_change_the_listening_address_and_the_outcome_are_logged_once() {
    let logs = capture_logs();
    let listener = rivium::service("udp", Frontline, |ctx| async move {
        ctx.listening("127.0.0.1:9000".parse().unwrap());
        ctx.spawn("reader", async {
            tracing::info!("reading");
            Ok(())
        });
        ctx.ready();
        ctx.stopped().await;
        Ok(())
    });
    let outcome = ended(supervisor(vec![listener]).run(stops_at(vec![(secs(1), sigterm())]))).await;
    assert_eq!(name(&outcome), "stopped");
    let lines: Vec<String> = (logs.events().iter())
        .map(|event| {
            let field = |name: &str| text(event.get(name));
            [
                "message",
                "phase",
                "reason",
                "service.name",
                "listen.addr",
                "outcome",
            ]
            .map(field)
            .join(" ")
        })
        .collect();
    assert_eq!(
        lines,
        [
            "phase changed starting the run started - - -",
            "listening - - udp 127.0.0.1:9000 -",
            "reading - - - - -",
            "phase changed running every service is ready - - -",
            "phase changed stopping SIGTERM - - -",
            "phase changed stopped stopped - - -",
            "outcome - - - - stopped",
        ]
    );
    // The task's event carries the service and the task in its spans.
    let reading = &logs.with_message("reading")[0];
    let spans: Vec<String> = (reading["spans"].as_array().unwrap().iter())
        .map(|span| {
            let field = |name: &str| text(span.get(name));
            ["name", "service.name", "task.name"].map(field).join(" ")
        })
        .collect();
    assert_eq!(spans, ["service udp -", "task - reader"]);
}

/// A text field of a logged event, or `-`.
fn text(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|value| value.as_str())
        .unwrap_or("-")
        .to_string()
}

/// A service that implements `Service` itself rather than through `service()`.
struct Counter;

impl Service for Counter {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "counter".into()
    }

    fn run(self: Box<Self>, ctx: ServiceContext) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        })
    }
}

/// Runs the services on a multi-threaded runtime without paused time, `runs` times, and returns
/// every outcome seen. Stop requests in `stops` are sent before the run starts.
async fn outcomes(
    make: impl Fn() -> Vec<Box<dyn Service>>,
    stops: &[StopReason],
    runs: usize,
) -> Vec<&'static str> {
    let mut seen = Vec::new();
    for _ in 0..runs {
        let (handle, receiver) = stop_channel();
        stops.iter().for_each(|stop| handle.stop(stop.clone()));
        let run = tokio::time::timeout(secs(10), supervisor(make()).run(receiver));
        let outcome = name(&run.await.expect("the run did not end within 10s"));
        if !seen.contains(&outcome) {
            seen.push(outcome);
        }
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outcome_does_not_depend_on_scheduling() {
    let runs = 300;
    // A service is ready and fails at once, while another one becomes ready.
    let ready_then_fail = || {
        let fails = http(vec![Step::Ready, Step::Fail("accept")]);
        scripted(vec![fails, log(serving())])
    };
    assert_eq!(outcomes(ready_then_fail, &[], runs).await, ["fault"]);
    // A service fails before it is ready, while another one becomes ready.
    let fail_before_ready = || scripted(vec![http(vec![Step::Fail("bind")]), log(serving())]);
    assert_eq!(
        outcomes(fail_before_ready, &[], runs).await,
        ["startup-failed"]
    );
    // A task fails at once while its service and another one become ready.
    let task_fails = || {
        let collector = rivium::service("collector", Background, |ctx| async move {
            ctx.spawn("device", async {
                Error::e_explain(kinds::UNAVAILABLE, "gone")
            });
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        });
        vec![collector, Box::new(Counter) as Box<dyn Service>]
    };
    assert_eq!(outcomes(task_fails, &[], runs).await, ["fault"]);
    // A stop request arrives at once, and a service fails when asked to stop.
    let fails_on_stop = || {
        scripted(vec![http(vec![
            Step::Ready,
            Step::UntilStopped,
            Step::Fail("flush"),
        ])])
    };
    assert_eq!(outcomes(fails_on_stop, &[sigterm()], runs).await, ["fault"]);
    // Two stop requests at once, with a service that ignores them.
    let stuck = || scripted(vec![http(vec![Step::Ready, Step::IgnoreStop(secs(3_600))])]);
    assert_eq!(
        outcomes(stuck, &[sigterm(), sigint()], runs).await,
        ["aborted"]
    );
}
