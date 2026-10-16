//! Dependency-free batched microbenchmarks; see docs/development.md for limitations.

use std::{hint::black_box, time::Instant};

use rivium::scope::TaskScope;
use rivium_util::task::{Context, Manager, Policy};
use tokio::runtime::{Builder, Runtime};

const SAMPLES: usize = 21;

fn measure(name: &str, iterations: usize, mut batch: impl FnMut(usize)) {
    if std::env::args().any(|arg| arg == "--test") {
        batch(1);
        return;
    }

    batch(iterations); // Untimed warm-up, including runtime scheduling and allocation.
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        batch(iterations);
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }

    samples.sort_by(f64::total_cmp);
    println!(
        "{name}, {iterations}, {SAMPLES}, {:.1}, {:.1}, {:.1}",
        samples[SAMPLES / 2],
        samples[0],
        samples[SAMPLES - 1],
    );
}

async fn worker(_: usize, _: usize, context: Context) -> rivium::Result<()> {
    context.cancel.cancelled().await;
    Ok(())
}

fn manager_benches(runtime: &Runtime, label: &str) {
    for (size, replacing, iterations) in [(128, false, 1_000), (1, true, 1_000), (64, true, 100)] {
        let scope = TaskScope::new();
        let mut manager = Manager::with_scope(Policy::default(), scope.clone()).unwrap();
        runtime.block_on(async {
            let report = manager
                .reconcile((0..size).map(|key| (key, 0)).collect(), worker)
                .await
                .unwrap();
            assert_eq!(report.started.len(), size);
            tokio::task::yield_now().await;
        });

        let mut generation = 0;
        let operation = if replacing { "replace" } else { "unchanged" };
        measure(
            &format!("manager/{label}/{operation}/{size}"),
            iterations,
            |n| {
                runtime.block_on(async {
                    for _ in 0..n {
                        generation += usize::from(replacing);
                        let report = manager
                            .reconcile((0..size).map(|key| (key, generation)).collect(), worker)
                            .await
                            .unwrap();
                        assert_eq!(report.restarted.len(), if replacing { size } else { 0 });
                        assert!(report.unconfirmed.is_empty());
                        black_box(report);
                    }
                });
            },
        );

        runtime.block_on(manager.shutdown()).unwrap();
        assert!(manager.is_empty());
        assert_eq!(scope.active_tasks(), 0);
    }
}

fn main() {
    println!(
        "case, iterations_per_sample, samples, median_ns_per_op, min_ns_per_op, max_ns_per_op"
    );
    let scope = TaskScope::new();
    for size in [0, 1, 64] {
        measure(&format!("scope/reserve_drop/{size}"), 100_000, |n| {
            for _ in 0..n {
                drop(black_box(scope.reserve(black_box(size)).unwrap()));
            }
        });
        assert_eq!(scope.active_tasks(), 0);
    }

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    measure("scope/reserve_track_ready/1", 100_000, |n| {
        runtime.block_on(async {
            for _ in 0..n {
                let reservation = scope.reserve(1).unwrap().pop().unwrap();
                black_box(reservation.track(std::future::ready(black_box(1))).await);
            }
        });
    });
    assert_eq!(scope.active_tasks(), 0);

    scope.close();
    measure("scope/closed_rejection", 100_000, |n| {
        for _ in 0..n {
            assert!(black_box(scope.reserve(1)).is_none());
        }
    });

    manager_benches(&runtime, "current_thread");
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    manager_benches(&runtime, "2_workers");
}
