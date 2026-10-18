#![cfg(all(feature = "task", feature = "rt"))]

use rivium::{Kind, Readiness, Supervisor, scope::TaskScope, service};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Wake, Waker},
    time::Duration,
};

struct AdmissionObserver {
    scope: TaskScope,
    unlocked: AtomicBool,
}

impl Wake for AdmissionObserver {
    fn wake(self: Arc<Self>) {
        let scope = self.scope.clone();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            send.send(scope.reserve(0).is_none()).ok();
        });
        // A regression fails within one second rather than deadlocking inside the waker.
        self.unlocked.store(
            receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or(false),
            Ordering::SeqCst,
        );
    }
}

#[test]
fn closing_an_empty_scope_wakes_without_holding_the_admission_lock() {
    let scope = TaskScope::new();
    let observer = Arc::new(AdmissionObserver {
        scope: scope.clone(),
        unlocked: AtomicBool::new(false),
    });
    let waker = Waker::from(observer.clone());
    let mut wait = Box::pin(scope.wait());
    assert!(
        wait.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    scope.close();
    assert!(observer.unlocked.load(Ordering::SeqCst));
    assert!(
        wait.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_ready()
    );
}

struct GenerationObserver {
    readiness: Readiness,
    count: AtomicUsize,
}

impl Wake for GenerationObserver {
    fn wake(self: Arc<Self>) {
        self.count
            .store(self.readiness.active_tasks(), Ordering::SeqCst);
    }
}

#[tokio::test]
async fn child_drain_notification_observes_release_in_the_run_count() {
    let supervisor =
        Supervisor::default().with(service("root", Kind::Background, |cx| async move {
            let scope = cx.task_scope();
            let reserved = scope.reserve(1).unwrap();
            scope.close();
            let observer = Arc::new(GenerationObserver {
                readiness: cx.readiness(),
                count: AtomicUsize::new(usize::MAX),
            });
            let waker = Waker::from(observer.clone());
            let mut wait = Box::pin(scope.wait());
            assert!(
                wait.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            drop(reserved);
            assert_eq!(
                observer.count.load(Ordering::SeqCst),
                1,
                "only the root may remain visible"
            );
            cx.ready();
            Ok(())
        }));
    assert!(supervisor.run().await.drain.is_complete());
}
