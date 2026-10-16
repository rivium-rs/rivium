#![cfg(feature = "rt")]

use rivium::{Kind, Outcome, Phase, Supervisor, scope::Drain, service};
use std::time::{Duration, Instant};

async fn final_scope_release_races_shutdown_wait() {
    // The final reservation is released by an external owner, including while the
    // supervisor is still polling its shutdown decision on a current-thread runtime.
    for round in 0..64 {
        let (send, receive) = std::sync::mpsc::channel();
        let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_secs(3)).with(
            service("parent", Kind::Background, move |cx| async move {
                send.send(cx.task_scope().reserve(1).unwrap().pop().unwrap())
                    .ok();
                cx.ready();
                Ok(())
            }),
        );
        let readiness = supervisor.readiness();
        let observed = readiness.clone();
        let owner = std::thread::spawn(move || {
            let reservation = receive.recv().unwrap();
            while observed.phase() != Phase::Stopping {
                std::thread::yield_now();
            }

            for _ in 0..round % 8 {
                std::thread::yield_now();
            }
            drop(reservation);
        });
        let started = Instant::now();
        let report = supervisor.run().await;
        owner.join().unwrap();
        assert!(matches!(report.outcome, Outcome::Stopped), "{report:?}");
        assert_eq!(report.drain, Drain::Complete);
        assert_eq!(readiness.active_tasks(), 0);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "last release waited for the 3s grace instead of waking shutdown"
        );
    }
}

#[tokio::test]
async fn final_scope_release_wakes_shutdown_on_current_thread() {
    final_scope_release_races_shutdown_wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_scope_release_wakes_shutdown_on_two_workers() {
    final_scope_release_races_shutdown_wait().await;
}
