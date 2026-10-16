#![cfg(feature = "task")]

use rivium::scope::{Drain, TaskScope};
use rivium_util::task::{Lifecycle, Manager, Policy};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const MANAGERS: usize = 4;
const KEYS: usize = 4;

struct Probe {
    scope: TaskScope,
    slots: [AtomicUsize; MANAGERS * KEYS],
    created: AtomicUsize,
    released: AtomicUsize,
    replacements: AtomicUsize,
}

struct Resource {
    probe: Arc<Probe>,
    slot: usize,
}

impl Resource {
    fn acquire(probe: Arc<Probe>, slot: usize) -> Self {
        assert_eq!(
            probe.slots[slot].fetch_add(1, Ordering::SeqCst),
            0,
            "replacement overlaps the old incarnation's resource"
        );
        probe.created.fetch_add(1, Ordering::SeqCst);
        Self { probe, slot }
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        assert!(
            self.probe.scope.active_tasks() > 0,
            "scope drained before the resource was released"
        );
        assert_eq!(
            self.probe.slots[self.slot].fetch_sub(1, Ordering::SeqCst),
            1
        );
        self.probe.released.fetch_add(1, Ordering::SeqCst);
    }
}

// A logged seed reproduces workload choices; executor interleavings remain nondeterministic.
fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    *seed
}

async fn yields(count: usize) {
    for _ in 0..count {
        tokio::task::yield_now().await;
    }
}

async fn generation(seed: u64) -> (usize, usize) {
    let scope = TaskScope::new();
    let probe = Arc::new(Probe {
        scope: scope.clone(),
        slots: std::array::from_fn(|_| AtomicUsize::new(0)),
        created: AtomicUsize::new(0),
        released: AtomicUsize::new(0),
        replacements: AtomicUsize::new(0),
    });
    let ready = Arc::new(tokio::sync::Barrier::new(MANAGERS + 1));
    let mut actors = Vec::new();
    for actor in 0..MANAGERS {
        let probe = probe.clone();
        let ready = ready.clone();
        actors.push(tokio::spawn(async move {
            let mut random = seed.wrapping_add(actor as u64);
            let mut manager = Manager::with_scope(
                Policy {
                    capacity: KEYS,
                    stop_timeout: Duration::from_secs(1),
                    ..Policy::default()
                },
                probe.scope.clone(),
            )
            .unwrap();
            for round in 0..32 {
                let desired = (0..KEYS)
                    .filter_map(|key| {
                        let choice = next(&mut random);
                        (round == 0 || choice & 3 != 0).then_some((key, choice >> 32 & 3))
                    })
                    .collect::<Vec<_>>();
                let report = manager
                    .reconcile(desired, |key, spec, context| {
                        // Capture before first poll, so abort-before-poll is checked too.
                        let resource = Resource::acquire(probe.clone(), actor * KEYS + key);
                        async move {
                            let _resource = resource;
                            yields(spec as usize).await;
                            context.mark_progress();
                            context.cancel.cancelled().await;
                            yields((spec + 1) as usize).await;
                            Ok(())
                        }
                    })
                    .await
                    .unwrap();
                probe
                    .replacements
                    .fetch_add(report.restarted.len(), Ordering::SeqCst);
                if round == 0 {
                    ready.wait().await;
                }
                assert!(report.unconfirmed.is_empty());
                assert!(report.limited.is_empty());
                assert!(manager.len() <= KEYS);
                if report.lifecycle == Lifecycle::Closed {
                    assert!(report.started.is_empty());
                    assert!(report.restarted.is_empty());
                    assert!(report.removed.is_empty());
                    break;
                }
                yields((next(&mut random) & 7) as usize).await;
            }
            manager.shutdown().await.unwrap();
            assert!(manager.is_empty());
        }));
    }
    let observing = scope.clone();
    let observed_probe = probe.clone();
    let observer = tokio::spawn(async move {
        observing.wait().await;
        assert_eq!(observing.active_tasks(), 0);
        assert!(observing.reserve(1).is_none());
        assert!(
            observed_probe
                .slots
                .iter()
                .all(|slot| slot.load(Ordering::SeqCst) == 0)
        );
        assert_eq!(
            observed_probe.created.load(Ordering::SeqCst),
            observed_probe.released.load(Ordering::SeqCst)
        );
    });
    ready.wait().await;
    yields((seed % 128) as usize).await;
    if seed & 1 == 0 {
        scope.close();
    } else {
        scope.cancel();
    }
    // Exercise lost parents as well as graceful shutdown, including actors waiting
    // for replacement exit while they own an unused batch of reservations.
    for (actor, handle) in actors.iter().enumerate() {
        if seed.wrapping_add(actor as u64).is_multiple_of(3) {
            handle.abort();
        }
    }
    for handle in actors {
        if let Err(error) = handle.await {
            assert!(error.is_cancelled(), "actor panicked: {error}");
        }
    }
    assert_eq!(scope.drain(Duration::from_secs(5)).await, Drain::Complete);
    observer.await.unwrap();
    (
        probe.created.load(Ordering::SeqCst),
        probe.replacements.load(Ordering::SeqCst),
    )
}

async fn run_for(duration: Duration, minimum: usize) {
    let seed = std::env::var("RIVIUM_STRESS_SEED")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("RIVIUM_STRESS_SEED must be a u64")
        })
        .unwrap_or(0x5c0f_e202_6101_5001);
    let started = Instant::now();
    let mut generations = 0;
    let mut resources = 0;
    let mut replacements = 0;
    eprintln!("task stress starting: seed={seed}, budget={duration:?}, minimum={minimum}");
    while generations < minimum || started.elapsed() < duration {
        let (created, restarted) = tokio::time::timeout(
            Duration::from_secs(10),
            generation(seed.wrapping_add(generations as u64)),
        )
        .await
        .unwrap_or_else(|_| panic!("generation {generations} timed out; initial seed={seed}"));
        resources += created;
        replacements += restarted;
        generations += 1;
    }
    assert!(resources > 0);
    assert!(replacements > 0, "stress must exercise actual replacements");
    eprintln!(
        "task stress passed: seed={seed}, generations={generations}, resources={resources}, replacements={replacements}, elapsed={:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_lifecycle_smoke() {
    run_for(Duration::ZERO, 64).await;
}

/// RIVIUM_STRESS_SECONDS=3600 cargo test -p rivium-util --features task \
///     --test task_stress -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in sustained concurrency stress; defaults to 60 seconds"]
async fn concurrent_lifecycle_stress() {
    let seconds = std::env::var("RIVIUM_STRESS_SECONDS")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("RIVIUM_STRESS_SECONDS must be a u64")
        })
        .unwrap_or(60);
    assert!(seconds > 0, "stress duration must be positive");
    run_for(Duration::from_secs(seconds), 64).await;
}
