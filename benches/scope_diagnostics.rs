//! Scope attribution probes; setup is excluded where stated, unlike task_lifecycle.

use std::{
    cell::Cell,
    future::{Future, ready},
    hint::black_box,
    pin::{Pin, pin},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use rivium::scope::TaskScope;

const BATCH: usize = 100_000;
const SAMPLES: usize = 21;

fn measure(name: &str, mut sample: impl FnMut(usize) -> Duration) {
    if std::env::args().any(|arg| arg == "--test") {
        sample(1);
        return;
    }

    sample(BATCH);
    let mut samples = (0..SAMPLES)
        .map(|_| sample(BATCH).as_nanos() as f64 / BATCH as f64)
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    println!(
        "{name}, {BATCH}, {SAMPLES}, {:.1}, {:.1}, {:.1}",
        samples[SAMPLES / 2],
        samples[0],
        samples[SAMPLES - 1],
    );
}

struct PendingOnce(bool);

impl Future for PendingOnce {
    type Output = usize;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if std::mem::replace(&mut self.0, true) {
            Poll::Ready(black_box(1))
        } else {
            Poll::Pending
        }
    }
}

struct Captured<'a>(&'a Cell<usize>);

impl Drop for Captured<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() {
    println!(
        "case, iterations_per_sample, samples, median_ns_per_op, min_ns_per_op, max_ns_per_op"
    );
    let scope = TaskScope::new();
    let mut cx = Context::from_waker(Waker::noop());

    measure("scope_diagnostic/reserve_drop/1", |n| {
        let start = Instant::now();
        for _ in 0..n {
            drop(black_box(scope.reserve(black_box(1)).unwrap()));
        }
        let elapsed = start.elapsed();
        assert_eq!(scope.active_tasks(), 0);
        elapsed
    });

    measure("scope_diagnostic/pre_reserved_track_ready", |n| {
        // One batch reservation; only its last item reaches zero. Its Vec drop is untimed.
        let mut reservations = scope.reserve(n).unwrap().into_iter();
        let start = Instant::now();
        for reservation in reservations.by_ref() {
            let mut tracked = pin!(reservation.track(ready(black_box(1))));
            assert_eq!(black_box(tracked.as_mut().poll(&mut cx)), Poll::Ready(1));
        }
        let elapsed = start.elapsed();
        drop(reservations);
        assert_eq!(scope.active_tasks(), 0);
        elapsed
    });

    measure("scope_diagnostic/reserve_track_pending_ready", |n| {
        let start = Instant::now();
        for _ in 0..n {
            let reservation = scope.reserve(1).unwrap().pop().unwrap();
            let mut tracked = pin!(reservation.track(PendingOnce(false)));
            // Manual second poll isolates wrapper cost; this is not scheduler latency.
            assert!(black_box(tracked.as_mut().poll(&mut cx)).is_pending());
            assert_eq!(black_box(tracked.as_mut().poll(&mut cx)), Poll::Ready(1));
        }
        let elapsed = start.elapsed();
        assert_eq!(scope.active_tasks(), 0);
        elapsed
    });

    measure("scope_diagnostic/reserve_track_capture_drop", |n| {
        let dropped = Cell::new(0);
        let start = Instant::now();
        for _ in 0..n {
            let reservation = scope.reserve(1).unwrap().pop().unwrap();
            let capture = Captured(&dropped);
            let mut tracked = pin!(reservation.track(async move {
                black_box(&capture);
                black_box(1)
            }));
            assert_eq!(black_box(tracked.as_mut().poll(&mut cx)), Poll::Ready(1));
        }
        let elapsed = start.elapsed();
        assert_eq!(dropped.get(), n);
        assert_eq!(scope.active_tasks(), 0);
        elapsed
    });

    let observed = TaskScope::new();
    let wake_count = Arc::new(WakeCount::default());
    let waker = Waker::from(wake_count.clone());
    let mut observed_cx = Context::from_waker(&waker);
    let mut waiter = pin!(observed.wait());
    assert!(waiter.as_mut().poll(&mut observed_cx).is_pending());
    measure("scope_diagnostic/subscribed_waiter_release_rearm", |n| {
        let before = wake_count.0.load(Ordering::Relaxed);
        let start = Instant::now();
        for _ in 0..n {
            drop(black_box(observed.reserve(1).unwrap()));
            // Open-scope wait remains pending, rearming after every final-release wake.
            assert!(black_box(waiter.as_mut().poll(&mut observed_cx)).is_pending());
        }
        let elapsed = start.elapsed();
        assert_eq!(wake_count.0.load(Ordering::Relaxed) - before, n);
        assert_eq!(observed.active_tasks(), 0);
        elapsed
    });
    observed.close();
    assert!(waiter.as_mut().poll(&mut observed_cx).is_ready());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (scope_tx, scope_rx) = tokio::sync::oneshot::channel();
    let supervisor = rivium::Supervisor::default().with(rivium::service(
        "scope-probe",
        rivium::Kind::Background,
        |context| async move {
            scope_tx.send(context.task_scope()).unwrap();
            context.ready();
            context.stopped().await;
            Ok(())
        },
    ));
    let control = supervisor.control();
    let (run, shared_scope) = runtime.block_on(async {
        let run = tokio::spawn(supervisor.run());
        let scope = scope_rx.await.unwrap();
        tokio::task::yield_now().await;
        (run, scope)
    });
    measure("scope_diagnostic/shared_supervisor_release_yield", |n| {
        let start = Instant::now();
        runtime.block_on(async {
            for _ in 0..n {
                drop(black_box(shared_scope.reserve(1).unwrap()));
                // Include the scheduler turn that lets the shared observer rearm.
                tokio::task::yield_now().await;
            }
        });
        let elapsed = start.elapsed();
        assert_eq!(shared_scope.active_tasks(), 0);
        elapsed
    });
    control.stop(rivium::StopReason::Requested);
    assert!(runtime.block_on(run).unwrap().drain.is_complete());
}
