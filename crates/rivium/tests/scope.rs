#![cfg(feature = "task")]
use rivium::scope::TaskScope;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};

struct Probe {
    activity: TaskScope,
    dropped: Arc<AtomicBool>,
    panic_on_poll: bool,
}

impl Future for Probe {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        assert!(!self.panic_on_poll, "test poll panic");
        Poll::Ready(())
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        assert_eq!(
            self.activity.active_tasks(),
            1,
            "released before future resources"
        );
        self.dropped.store(true, Ordering::Release);
    }
}

#[test]
fn completion_unpolled_drop_and_panic_release_future_before_registration() {
    for mode in 0..3 {
        let activity = TaskScope::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let tracked = activity.reserve(1).unwrap().pop().unwrap().track(Probe {
            activity: activity.clone(),
            dropped: dropped.clone(),
            panic_on_poll: mode == 2,
        });
        activity.close();
        assert!(activity.reserve(0).is_none());
        assert_eq!(activity.active_tasks(), 1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut tracked = Box::pin(tracked);
            if mode != 0 {
                assert!(
                    tracked
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_ready()
                );
                assert_eq!(activity.active_tasks(), 0);
                assert!(dropped.load(Ordering::Acquire));
            }
        }));
        assert_eq!(result.is_err(), mode == 2);
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(activity.active_tasks(), 0);
    }
}

#[test]
fn close_and_registration_have_one_order_across_threads() {
    for _ in 0..64 {
        let activity = TaskScope::default();
        let barrier = Arc::new(Barrier::new(2));
        let worker_activity = activity.clone();
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            worker_activity.reserve(1)
        });
        barrier.wait();
        activity.close();
        let count_at_close = activity.active_tasks();
        let registered = worker.join().unwrap();
        assert_eq!(count_at_close, usize::from(registered.is_some()));
        assert!(activity.reserve(0).is_none());
        drop(registered);
        assert_eq!(activity.active_tasks(), 0);
    }
}

#[tokio::test]
async fn reservations_survive_close_and_wait_for_actual_resource_release() {
    use rivium::scope::Drain;
    use std::time::Duration;
    let scope = TaskScope::new();
    let mut reserved = scope.reserve(2).unwrap();
    scope.close();
    assert!(scope.reserve(0).is_none());
    assert_eq!(
        scope.drain(Duration::ZERO).await,
        Drain::Incomplete { active_tasks: 2 }
    );
    reserved.pop().unwrap().track(async {}).await;
    assert_eq!(scope.active_tasks(), 1);
    drop(reserved);
    assert_eq!(scope.drain(Duration::ZERO).await, Drain::Complete);
}

#[tokio::test]
async fn cancelling_token_also_rejects_reservation() {
    let scope = TaskScope::new();
    scope.cancellation_token().cancel();
    assert!(scope.reserve(0).is_none());
    assert!(scope.reserve(1).is_none());
    scope.cancel();
    scope.wait().await;
}

#[tokio::test]
async fn blocking_abort_is_incomplete_until_closure_releases_resources() {
    use rivium::scope::Drain;
    use std::time::Duration;
    let scope = TaskScope::new();
    let reservation = scope.reserve(1).unwrap().pop().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let task = tokio::task::spawn_blocking(move || {
        reservation.run(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        })
    });
    started_rx.await.unwrap();
    scope.cancel();
    task.abort();
    let state = scope.drain(Duration::ZERO).await;
    drop(release_tx);
    task.await.unwrap();
    assert_eq!(state, Drain::Incomplete { active_tasks: 1 });
    assert_eq!(scope.drain(Duration::ZERO).await, Drain::Complete);
}

#[test]
fn impossible_reservation_does_not_poison_accounting() {
    let scope = TaskScope::new();
    let result = std::panic::catch_unwind(|| scope.reserve(usize::MAX));
    assert!(matches!(result, Ok(None)));
    assert_eq!(scope.active_tasks(), 0);
    assert!(scope.reserve(1).is_some());
}
