//! Closed-loop Supervisor generation measurements, not a production arrival model.
//!
//! Defaults cover P-CORE's 1/32/256 roots, 0/8 children and 1/4 workers.
//! Each CSV row is one generation; startup excludes constructing its service factories.
//! `RIVIUM_SCALE_ROUNDS`, `RIVIUM_SCALE_HOLD_MS` and `RIVIUM_SCALE_WARMUP`
//! select repetition/steady ownership without changing the production quotas.
//! An external process sampler/watchdog supplies RSS/FD/thread evidence.
//! Allocation counts require a separate profiler.
//! Restart rows request a Supervisor restart and construct the next generation only
//! after the old report and Drop barrier; they do not exercise embedded Host restart.

use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use rivium::{Kind, Outcome, Phase, Supervisor, rt::StopReason, service};

struct Capture(Arc<AtomicUsize>);

impl Drop for Capture {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn parameter(name: &str, fallback: usize, max: usize) -> usize {
    let value = std::env::var(name).map_or(fallback, |value| {
        value.parse().unwrap_or_else(|_| panic!("invalid {name}"))
    });
    assert!(value <= max, "{name} exceeds {max}");
    value
}

async fn generation(roots: usize, children: usize, hold: Duration, restart: bool) -> [u128; 5] {
    let dropped = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let expected = roots * (children + 1);
    let mut supervisor = Supervisor::new(Duration::from_secs(10), Duration::from_secs(3));
    for root in 0..roots {
        let dropped = dropped.clone();
        let entered = entered.clone();
        let completed = completed.clone();
        supervisor = supervisor.with(service(
            format!("root-{root}"),
            Kind::Frontline,
            move |context| async move {
                let _capture = Capture(dropped.clone());
                entered.fetch_add(1, Ordering::SeqCst);
                for child in 0..children {
                    let capture = Capture(dropped.clone());
                    let entered = entered.clone();
                    let completed = completed.clone();
                    let stop = context.stop();
                    context
                        .spawn(format!("child-{child}"), async move {
                            let _capture = capture;
                            entered.fetch_add(1, Ordering::SeqCst);
                            stop.stopped().await;
                            completed.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .expect("configured fanout is within the fixed supervisor quota");
                }
                context.ready();
                context.stopped().await;
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        ));
    }

    let mut readiness = supervisor.readiness();
    let control = supervisor.control();
    let started = Instant::now();
    let run = tokio::spawn(supervisor.run());
    tokio::time::timeout(Duration::from_secs(10), async {
        while readiness.phase() != Phase::Running {
            readiness
                .changed()
                .await
                .expect("readiness ended before running");
        }
    })
    .await
    .expect("startup watchdog expired");
    let ready_ns = started.elapsed().as_nanos();
    tokio::time::timeout(Duration::from_secs(10), async {
        while entered.load(Ordering::SeqCst) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fanout did not enter all execution bodies");
    let fanout_ns = started.elapsed().as_nanos();
    assert_eq!(readiness.active_tasks(), expected);

    // One explicit deadline measures timer/scheduler lag. It is not request latency.
    let due = tokio::time::Instant::now() + hold;
    tokio::time::sleep_until(due).await;
    let lag_ns = tokio::time::Instant::now()
        .saturating_duration_since(due)
        .as_nanos();
    let stopping = Instant::now();
    if restart {
        control.restart("scale generation");
    } else {
        control.stop(StopReason::Requested);
    }
    let report = tokio::time::timeout(Duration::from_secs(9), run)
        .await
        .expect("external watchdog must also bound a stalled runtime")
        .expect("supervisor task panicked");
    let stop_ns = stopping.elapsed().as_nanos();
    let outcome = report.outcome;
    assert!(report.drain.is_complete());
    if restart {
        assert!(matches!(outcome, Outcome::Restart(_)));
    } else {
        assert!(matches!(outcome, Outcome::Stopped));
    }
    assert_eq!(readiness.phase(), Phase::Stopped);
    assert_eq!(completed.load(Ordering::SeqCst), expected);
    assert_eq!(readiness.active_tasks(), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), expected);
    [ready_ns, fanout_ns, lag_ns, stop_ns, expected as u128]
}

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--test");
    let rounds = if smoke {
        2
    } else {
        parameter("RIVIUM_SCALE_ROUNDS", 1_000, 1_000_000)
    };
    assert!(rounds > 0, "at least one measured generation is required");
    let warmup = if smoke {
        0
    } else {
        parameter("RIVIUM_SCALE_WARMUP", 10, 100_000)
    };
    let hold = Duration::from_millis(parameter("RIVIUM_SCALE_HOLD_MS", 1, 600_000) as u64);
    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(
        output,
        "pid,workers,roots,children,round,restart,ready_ns,fanout_ns,timer_lag_ns,stop_ns,active_peak,active_final,captures_dropped"
    )
    .unwrap();
    for workers in [1, 4] {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .enable_all()
            .build()
            .unwrap();
        for roots in [1, 32, 256] {
            for children in [0, 8] {
                for index in 0..warmup + rounds {
                    let restart = !index.is_multiple_of(2);
                    let [ready, fanout, lag, stop, peak] =
                        runtime.block_on(generation(roots, children, hold, restart));
                    if index >= warmup {
                        writeln!(
                            output,
                            "{},{workers},{roots},{children},{},{restart},{ready},{fanout},{lag},{stop},{peak},0,{peak}",
                            std::process::id(),
                            index - warmup
                        )
                        .unwrap();
                        output.flush().unwrap();
                    }
                }
            }
        }
    }
}
