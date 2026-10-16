#![cfg(feature = "rt")]

mod common;

use common::Dropped;

use rivium::{Kind, Outcome, Phase, Supervisor, rt::StopReason, service};
use rivium_test::{idle, until_ready};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn supervisor() -> Supervisor {
    Supervisor::new(Duration::from_millis(100), Duration::from_millis(100))
}

fn stalled(name: &str, stops: &'static [StopReason]) -> (Supervisor, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let sup = supervisor().with(service(name, Kind::Frontline, move |ctx| async move {
        let _guard = guard;
        ctx.ready();
        for stop in stops {
            ctx.control().stop(*stop);
        }
        std::future::pending().await
    }));
    (sup, dropped)
}

#[tokio::test]
async fn ready_health_and_stop() {
    let sup = supervisor().with(idle("http", Kind::Frontline));
    let mut health = sup.readiness();
    let check = health.health().register("database").unwrap();
    let stop = sup.control();
    let run = tokio::spawn(sup.run());
    while health.phase() != Phase::Running {
        health.changed().await.unwrap();
    }
    assert!(!health.is_ready());
    check.set(rivium::Health::Healthy);
    assert!(health.is_ready());
    stop.stop(StopReason::Requested);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
    assert_eq!(health.phase(), Phase::Stopped);
}

#[tokio::test]
async fn foreground_drains_before_background() {
    let order = Arc::new(Mutex::new(vec![]));
    let mut sup = supervisor();
    for (name, kind) in [("front", Kind::Frontline), ("back", Kind::Background)] {
        let order = order.clone();
        sup = sup.with(service(name, kind, move |ctx| async move {
            ctx.ready();
            ctx.stopped().await;
            order.lock().unwrap().push(name);
            Ok(())
        }));
    }

    let (stop, _readiness, run) = common::ready(sup).await;
    stop.stop(StopReason::Requested);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
    assert_eq!(*order.lock().unwrap(), ["front", "back"]);
}

#[tokio::test]
async fn child_failure_stops_siblings() {
    let ended = Arc::new(AtomicBool::new(false));
    let mark = ended.clone();
    let sup = supervisor()
        .with(service("parent", Kind::Frontline, |ctx| async move {
            ctx.ready();
            ctx.spawn("bad", async {
                Err(rivium::Error::internal("child failed"))
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            ctx.stopped().await;
            Ok(())
        }))
        .with(service(
            "sibling",
            Kind::Background,
            move |ctx| async move {
                ctx.ready();
                ctx.stopped().await;
                mark.store(true, Ordering::Release);
                Ok(())
            },
        ));
    assert!(matches!(sup.run().await.outcome, Outcome::Failed(_)));
    assert!(ended.load(Ordering::Acquire));
}

#[tokio::test]
async fn children_queued_before_parent_exit_are_drained() {
    let done = Arc::new(AtomicBool::new(false));
    let mark = done.clone();
    let sup = supervisor().with(service("job", Kind::Background, move |ctx| async move {
        ctx.ready();
        ctx.spawn("child", async move {
            tokio::task::yield_now().await;
            mark.store(true, Ordering::Release);
            Ok(())
        })
        .map_err(|error| rivium::Error::internal(error.to_string()))?;
        Ok(())
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
    assert!(done.load(Ordering::Acquire));
}

#[tokio::test]
async fn service_panic_is_contained() {
    let sup = supervisor().with(service("panic", Kind::Frontline, |_| async {
        panic!("test panic")
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::StartupFailed(_)));
}

#[tokio::test]
async fn startup_timeout_is_a_failure() {
    let sup = supervisor().with(service("stalled", Kind::Frontline, |ctx| async move {
        ctx.stopped().await;
        Ok(())
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::StartupFailed(_)));
}

#[tokio::test]
async fn shared_deadline_reports_uncooperative_tasks() {
    let (sup, dropped) = stalled("stalled", &[StopReason::Requested]);
    let readiness = sup.readiness();
    assert!(matches!(sup.run().await.outcome,Outcome::TimedOut{tasks,..} if tasks==["stalled"]));
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn second_stop_aborts() {
    let (sup, dropped) = stalled("stalled", &[StopReason::Signal(15), StopReason::Signal(2)]);
    let readiness = sup.readiness();
    assert!(matches!(sup.run().await.outcome, Outcome::Aborted(2)));
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(readiness.active_tasks(), 0);
}

async fn blocking_shutdown(second_stop: bool) {
    let mut releases = vec![];
    let mut gates = vec![];
    for _ in 0..8 {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        releases.push(release);
        gates.push(gate);
    }

    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(5), Duration::from_millis(20)).with(service(
        "parent",
        Kind::Frontline,
        move |ctx| async move {
            let mut started = vec![];
            for (index, gate) in gates.into_iter().enumerate() {
                let (tx, rx) = tokio::sync::oneshot::channel();
                started.push(rx);
                ctx.spawn_blocking(format!("blocking-{index}"), move || {
                    let _ = tx.send(());
                    let _ = gate.recv();
                    Ok(())
                })
                .map_err(|error| rivium::Error::internal(error.to_string()))?;
            }

            for start in started {
                start.await.unwrap();
            }
            ctx.ready();
            let _ = stopping_tx.send(tokio::time::Instant::now());
            ctx.control().stop(StopReason::Requested);
            if second_stop {
                ctx.control().stop(StopReason::Signal(2));
            }
            ctx.stopped().await;
            Ok(())
        },
    ));
    let readiness = sup.readiness();
    let outcome = tokio::time::timeout(Duration::from_secs(5), sup.run())
        .await
        .expect("shutdown must not wait for blocking work indefinitely");
    let elapsed = stopping_rx.await.unwrap().elapsed();
    let active = readiness.active_tasks();
    // Release every blocking worker before assertions, including on test failure.
    drop(releases);
    tokio::time::timeout(Duration::from_secs(5), async {
        while readiness.active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("released blocking workers must finish");
    assert_eq!(readiness.phase(), Phase::Stopped);
    assert_eq!(active, 8);
    assert!(elapsed >= Duration::from_millis(100));
    assert!(
        elapsed < Duration::from_millis(500),
        "abort confirmation must share one budget, elapsed: {elapsed:?}"
    );
    if second_stop {
        assert!(matches!(outcome.outcome, Outcome::Aborted(2)));
    } else {
        let expected: Vec<_> = (0..8).map(|i| format!("parent/blocking-{i}")).collect();
        assert!(
            matches!(outcome.outcome, Outcome::TimedOut { tasks, blocking }
            if tasks == expected && blocking == expected)
        );
    }
}

#[tokio::test]
async fn timeout_confirmation_is_bounded_and_retains_blocking_activity() {
    blocking_shutdown(false).await;
}

#[tokio::test]
async fn second_stop_confirmation_is_bounded_and_retains_blocking_activity() {
    blocking_shutdown(true).await;
}

#[tokio::test]
async fn restart_is_overridden_by_external_stop() {
    let sup = supervisor().with(idle("a", Kind::Frontline));
    let stop = sup.control();
    stop.restart("new config");
    stop.stop(StopReason::Requested);
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
}

#[tokio::test]
async fn dropping_supervisor_cancels_owned_tasks() {
    let (sup, dropped) = stalled("owned", &[]);
    let (_control, readiness, run) = common::ready(sup).await;
    run.abort();
    let _ = run.await;
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(readiness.phase(), Phase::Stopped);
}

#[tokio::test]
async fn duplicate_services_are_rejected() {
    let sup = supervisor()
        .with(idle("same", Kind::Frontline))
        .with(idle("same", Kind::Background));
    assert!(matches!(sup.run().await.outcome, Outcome::StartupFailed(_)));
}

#[tokio::test]
async fn separately_owned_work_survives_parent_abort_and_closes_admission() {
    let (send, receive) = tokio::sync::oneshot::channel();
    let sup = supervisor().with(service("parent", Kind::Frontline, move |ctx| async move {
        let activity = ctx.task_scope();
        let child = tokio::spawn(
            activity
                .reserve(1)
                .unwrap()
                .pop()
                .unwrap()
                .track(std::future::pending::<()>()),
        );
        send.send((activity, child))
            .unwrap_or_else(|_| panic!("receiver gone"));
        ctx.ready();
        std::future::pending().await
    }));
    let readiness = sup.readiness();
    let run = tokio::spawn(sup.run());
    let (activity, child) = receive.await.unwrap();
    assert_eq!(readiness.active_tasks(), 2);
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(activity.reserve(0).is_none());
    while readiness.active_tasks() != 1 {
        tokio::task::yield_now().await;
    }
    assert!(!child.is_finished());
    child.abort();
    assert!(child.await.unwrap_err().is_cancelled());
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn queued_work_is_counted_before_supervisor_receives_it() {
    let sup = supervisor().with(service("parent", Kind::Background, move |ctx| async move {
        let activity = ctx.task_scope();
        ctx.spawn("child", async { Ok(()) })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
        assert_eq!(activity.active_tasks(), 1);
        ctx.ready();
        Ok(())
    }));
    let readiness = sup.readiness();
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn frontline_child_outlives_root_and_background_admission_stays_open_until_drain() {
    let (scope_tx, scope_rx) = tokio::sync::oneshot::channel();
    let (root_tx, root_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let sup = supervisor()
        .with(service("front", Kind::Frontline, move |ctx| async move {
            let scope = ctx.task_scope();
            let child = scope.reserve(1).unwrap().pop().unwrap().track(async move {
                let _ = release_rx.await;
            });
            tokio::spawn(child);
            ctx.ready();
            ctx.stopped().await;
            let _ = root_tx.send(());
            Ok(())
        }))
        .with(service("back", Kind::Background, move |ctx| async move {
            scope_tx.send(ctx.task_scope()).ok().unwrap();
            ctx.ready();
            ctx.stopped().await;
            Ok(())
        }));
    let (stop, mut readiness, run) = common::spawn(sup);
    let background = scope_rx.await.unwrap();
    until_ready(&mut readiness).await.unwrap();
    stop.stop(StopReason::Requested);
    root_rx.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!run.is_finished());
    assert!(!background.cancellation_token().is_cancelled());
    let reserved = background
        .reserve(1)
        .expect("background still admitted during front drain");
    drop(reserved);
    release_tx.send(()).unwrap();
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
    assert_eq!(readiness.active_tasks(), 0);
    assert!(background.reserve(0).is_none());
}

#[tokio::test]
async fn finished_background_root_does_not_report_success_with_unreleased_child() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let sup = supervisor().with(service("back", Kind::Background, move |ctx| async move {
        let scope = ctx.task_scope();
        let child = scope.reserve(1).unwrap().pop().unwrap().track(async move {
            let _ = release_rx.await;
        });
        tokio::spawn(child);
        ctx.ready();
        Ok(())
    }));
    let readiness = sup.readiness();
    let result = sup.run().await;
    assert!(matches!(result.outcome, Outcome::TimedOut { tasks, .. } if tasks == ["back/<scope>"]));
    assert_eq!(readiness.active_tasks(), 1);
    release_tx.send(()).unwrap();
    assert_eq!(
        readiness.drain(Duration::from_secs(1)).await,
        rivium::scope::Drain::Complete
    );
}

#[tokio::test]
async fn root_can_drain_its_scope_without_waiting_for_itself() {
    let sup = supervisor().with(service("parent", Kind::Background, |ctx| async move {
        let scope = ctx.task_scope();
        assert_eq!(scope.active_tasks(), 0);
        assert_eq!(ctx.readiness().active_tasks(), 1);
        let token = scope.cancellation_token();
        tokio::spawn(scope.reserve(1).unwrap().pop().unwrap().track(async move {
            token.cancelled().await;
        }));
        scope.cancel();
        assert_eq!(
            scope.drain(Duration::from_secs(1)).await,
            rivium::scope::Drain::Complete
        );
        ctx.ready();
        Ok(())
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
}

#[tokio::test(start_paused = true)]
async fn background_duplicate_rejection_preserves_deadline() {
    let (deadline_tx, deadline_rx) = tokio::sync::oneshot::channel();
    let (checked_tx, checked_rx) = tokio::sync::oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(1), Duration::from_secs(1))
        .with(service("front", Kind::Frontline, move |ctx| async move {
            ctx.ready();
            ctx.stopped().await;
            deadline_tx.send(ctx.deadline()).unwrap();
            std::future::pending().await
        }))
        .with(service("back", Kind::Background, move |ctx| async move {
            let stop = ctx.stop();
            ctx.spawn("same", async move {
                stop.stopped().await;
                Ok(())
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            ctx.ready();
            let original = deadline_rx.await.unwrap();
            assert_eq!(
                ctx.spawn("same", async { Ok(()) }),
                Err(rivium::rt::SubmitError::DuplicateName)
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
            checked_tx.send(ctx.deadline() == original).unwrap();
            ctx.stopped().await;
            Ok(())
        }));
    let (control, _readiness, run) = common::ready(sup).await;
    control.stop(StopReason::Deadline(Duration::from_millis(20)));
    assert!(
        checked_rx.await.unwrap(),
        "later command failure extended shutdown deadline"
    );
    assert!(matches!(
        run.await.unwrap().outcome,
        Outcome::TimedOut { .. }
    ));
}
