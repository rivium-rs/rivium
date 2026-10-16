#![cfg(feature = "rt")]

mod common;

use rivium::{
    Error, Health, Kind, Outcome, Phase, Supervisor,
    error::Class,
    rt::{StopReason, SubmitError, TASK_CAPACITY},
    scope::Drain,
    service,
};
use rivium_test::until_ready;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn child_names_are_checked_before_queueing_and_reusable_after_resource_release() {
    let sup = Supervisor::default().with(service("owner", Kind::Background, |cx| async move {
        cx.ready();
        assert_eq!(
            cx.spawn("", async { Ok(()) }),
            Err(SubmitError::InvalidName)
        );
        cx.spawn("repeat", async { Ok(()) }).unwrap();
        assert_eq!(
            cx.spawn("repeat", async { Ok(()) }),
            Err(SubmitError::DuplicateName)
        );
        while cx.task_scope().active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
        cx.spawn("repeat", async { Ok(()) }).unwrap();
        while cx.task_scope().active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
        cx.task_scope().close();
        assert_eq!(
            cx.spawn("closed", async { Ok(()) }),
            Err(SubmitError::Closed)
        );
        Ok(())
    }));
    let report = sup.run().await;
    assert!(matches!(report.outcome, Outcome::Stopped));
    assert_eq!(report.drain, Drain::Complete);
}

#[tokio::test]
async fn root_and_queued_children_share_one_budget_and_rejection_drops_captures() {
    let dropped = Arc::new(AtomicBool::new(false));
    let marker = common::Dropped(dropped.clone());
    let sup =
        Supervisor::default().with(service("owner", Kind::Background, move |cx| async move {
            cx.ready();
            for index in 1..TASK_CAPACITY {
                cx.spawn(index.to_string(), std::future::pending()).unwrap();
            }
            assert_eq!(cx.task_scope().active_tasks(), TASK_CAPACITY - 1);
            let rejected = async move {
                let _marker = marker;
                Ok(())
            };
            assert_eq!(cx.spawn("excess", rejected), Err(SubmitError::Overloaded));
            assert_eq!(
                cx.spawn_blocking("excess-blocking", || panic!("rejected closure ran")),
                Err(SubmitError::Overloaded)
            );
            cx.task_scope().cancel();
            Ok(())
        }));
    let report = sup.run().await;
    assert!(matches!(report.outcome, Outcome::Stopped));
    assert_eq!(report.drain, Drain::Complete);
    assert!(dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn health_names_and_startup_window_are_bounded() {
    let sup = Supervisor::default().with(rivium_test::idle("owner", Kind::Frontline));
    let mut state = sup.readiness();
    let registry = state.health();
    assert!(registry.register("").is_err());
    assert!(registry.register("x".repeat(129)).is_err());
    registry
        .register("x".repeat(128))
        .unwrap()
        .set(Health::Healthy);
    for index in 1..TASK_CAPACITY {
        registry
            .register(index.to_string())
            .unwrap()
            .set(Health::Healthy);
    }
    assert_eq!(
        registry.register("over-limit").err().unwrap().class(),
        Class::TooManyRequests
    );
    let control = sup.control();
    let run = tokio::spawn(sup.run());
    until_ready(&mut state).await.unwrap();
    assert_eq!(
        registry.register("late").err().unwrap().class(),
        Class::Conflict
    );
    control.stop(StopReason::Requested);
    assert_eq!(run.await.unwrap().drain, Drain::Complete);
}

#[tokio::test]
async fn finished_outcome_does_not_erase_unreleased_blocking_work() {
    let (started, receive) = tokio::sync::oneshot::channel();
    let (release, block) = std::sync::mpsc::channel::<()>();
    let sup = Supervisor::new(Duration::from_secs(1), Duration::from_millis(1)).with(service(
        "owner",
        Kind::Frontline,
        |cx| async move {
            cx.spawn_blocking("device", move || {
                started.send(()).ok();
                block.recv().ok();
                Ok(())
            })
            .unwrap();
            receive.await.unwrap();
            cx.ready();
            cx.control().stop(StopReason::Requested);
            cx.stopped().await;
            Ok(())
        },
    ));
    let state = sup.readiness();
    let report = sup.run().await;
    assert!(matches!(report.outcome, Outcome::TimedOut { .. }));
    assert_eq!(report.drain, Drain::Incomplete { active_tasks: 1 });
    assert_eq!(state.phase(), Phase::Stopped);
    drop(release);
    assert_eq!(state.drain(Duration::from_secs(1)).await, Drain::Complete);
}

#[tokio::test]
async fn terminal_child_failure_is_observed_even_after_its_resources_drop() {
    let sup = Supervisor::default().with(service("owner", Kind::Background, |cx| async move {
        cx.ready();
        cx.spawn("failed", async {
            Err(Error::internal("retained business failure"))
        })
        .unwrap();
        while cx.task_scope().active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
        Ok(())
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::Failed(_)));
}

#[tokio::test(start_paused = true)]
async fn ordinary_stop_does_not_turn_an_unready_cooperative_root_into_startup_failure() {
    let sup =
        Supervisor::default().with(service("initializing", Kind::Frontline, |cx| async move {
            cx.control().stop(StopReason::Requested);
            cx.stopped().await;
            Ok(())
        }));
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
}

#[tokio::test]
async fn first_business_failure_is_not_replaced_by_failure_during_shutdown() {
    let sup = Supervisor::default()
        .with(service("first", Kind::Frontline, |cx| async move {
            common::declare_ready(&cx).await;
            Err(Error::internal("primary failure"))
        }))
        .with(service("cleanup", Kind::Background, |cx| async move {
            cx.ready();
            cx.stopped().await;
            Err(Error::internal("secondary failure"))
        }));
    match sup.run().await.outcome {
        Outcome::Failed(error) => assert!(
            error
                .chain()
                .any(|cause| cause.to_string() == "primary failure")
        ),
        outcome => panic!("unexpected {outcome:?}"),
    }
}

#[tokio::test]
async fn concurrent_submission_has_one_winner_for_a_service_child_name() {
    let sup = Supervisor::default().with(service("owner", Kind::Background, |cx| async move {
        cx.ready();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let cx = cx.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    cx.spawn("one", async { Ok(()) })
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(SubmitError::DuplicateName))
                .count(),
            7
        );
        Ok(())
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
}

#[tokio::test]
async fn invalid_root_set_never_calls_a_factory() {
    let invoked = Arc::new(AtomicBool::new(false));
    let marker = invoked.clone();
    let sup = Supervisor::default()
        .with(service("same", Kind::Background, move |_| {
            marker.store(true, Ordering::Relaxed);
            async { Ok(()) }
        }))
        .with(service("same", Kind::Background, |_| async { Ok(()) }));
    let report = sup.run().await;
    assert!(matches!(report.outcome, Outcome::StartupFailed(_)));
    assert_eq!(report.drain, Drain::Complete);
    assert!(!invoked.load(Ordering::Relaxed));
}
