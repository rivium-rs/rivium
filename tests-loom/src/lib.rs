//! Run with RUSTFLAGS="--cfg rivium_scope_model" cargo test --manifest-path tests-loom/Cargo.toml.
#![allow(dead_code)]
#[cfg(not(rivium_scope_model))]
compile_error!("run with RUSTFLAGS=\"--cfg rivium_scope_model\"");
#[cfg(rivium_scope_model)]
mod model_support;
// Compile the production admission and RAII code, rather than copying its algorithm.
#[cfg(rivium_scope_model)]
#[path = "../../crates/rivium/src/scope.rs"]
mod scope;

#[cfg(all(test, rivium_scope_model))]
mod tests {
    use super::{model_support::*, scope::*};
    use loom::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
    };
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn reserve_batch_and_close_are_linearized() {
        loom::model(|| {
            let scope = TaskScope::new();
            let worker_scope = scope.clone();
            let worker = thread::spawn(move || worker_scope.reserve(2));
            scope.close();
            let count_at_close = scope.active_tasks();
            assert!(scope.reserve(0).is_none());
            assert!(scope.reserve(1).is_none());
            let reservation = worker.join().unwrap();
            assert_eq!(count_at_close, if reservation.is_some() { 2 } else { 0 });
            drop(reservation);
            assert_eq!(scope.active_tasks(), 0);
        });
    }

    #[test]
    fn concurrent_close_and_release_never_reopen_admission() {
        loom::model(|| {
            let scope = TaskScope::new();
            let reservations = scope.reserve(2).unwrap();
            let worker = thread::spawn(move || drop(reservations));
            let closer_scope = scope.clone();
            let closer = thread::spawn(move || closer_scope.close());
            scope.close();
            assert!(scope.is_closed());
            assert!(scope.reserve(1).is_none());
            closer.join().unwrap();
            worker.join().unwrap();
            assert_eq!(scope.active_tasks(), 0);
        });
    }

    #[test]
    fn two_batches_release_without_losing_the_other_count() {
        loom::model(|| {
            let scope = TaskScope::new();
            let first = scope.reserve(2).unwrap();
            let second = scope.reserve(3).unwrap();
            scope.close();
            let worker = thread::spawn(move || drop(first));
            drop(second);
            assert!(scope.active_tasks() <= 2);
            worker.join().unwrap();
            assert_eq!(scope.active_tasks(), 0);
        });
    }

    struct Resource(Arc<AtomicBool>);
    impl Future for Resource {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Ready(())
        }
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    fn release_resources(complete: bool) {
        loom::model(move || {
            let scope = TaskScope::new();
            let released = Arc::new(AtomicBool::new(false));
            let tracked = scope
                .reserve(1)
                .unwrap()
                .pop()
                .unwrap()
                .track(Resource(released.clone()));
            scope.close();
            let worker = thread::spawn(move || {
                let mut tracked = Box::pin(tracked);
                if complete {
                    assert!(
                        tracked
                            .as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_ready()
                    );
                }
                drop(tracked);
            });
            if scope.active_tasks() == 0 {
                assert!(released.load(Ordering::Relaxed));
            }
            worker.join().unwrap();
            assert_eq!(scope.active_tasks(), 0);
            assert!(released.load(Ordering::Relaxed));
        });
    }

    #[test]
    fn unpolled_future_drop_releases_resources_before_count() {
        release_resources(false);
    }
    #[test]
    fn completed_future_releases_resources_before_count() {
        release_resources(true);
    }

    #[test]
    fn cancelled_scope_never_admits_after_cancel_returns() {
        loom::model(|| {
            let scope = TaskScope::new();
            let worker_scope = scope.clone();
            let worker = thread::spawn(move || worker_scope.reserve(1));
            scope.cancel();
            assert!(scope.cancellation_token().is_cancelled());
            assert!(scope.reserve(1).is_none());
            drop(worker.join().unwrap());
            assert_eq!(scope.active_tasks(), 0);
        });
    }
}
