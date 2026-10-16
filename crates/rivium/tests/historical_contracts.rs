#![cfg(feature = "rt")]

mod common;

use rivium::{
    Error, Health, Kind, Outcome, Phase, StopReason, Supervisor,
    error::{Class, kinds},
    scope::Drain,
    service,
};
use std::time::Duration;

async fn healthy_store() -> (common::Run, rivium::HealthHandle) {
    let supervisor = Supervisor::default().with(rivium_test::idle("front", Kind::Frontline));
    let check = supervisor.readiness().health().register("store").unwrap();
    check.set(Health::Healthy);
    (common::ready(supervisor).await, check)
}

#[tokio::test]
async fn empty_root_sets_and_empty_names_fail_before_execution() {
    for supervisor in [
        Supervisor::default(),
        Supervisor::default().with(service("", Kind::Background, |_| async {
            panic!("invalid root ran")
        })),
    ] {
        let report = supervisor.run().await;
        assert!(matches!(
            report.outcome,
            Outcome::StartupFailed(ref error) if error.class() == Class::InvalidInput
        ));
        assert_eq!(report.drain, Drain::Complete);
    }
}

#[tokio::test]
async fn a_ready_service_failure_is_a_runtime_fault_while_another_service_initializes() {
    let supervisor = Supervisor::default()
        .with(service("ready", Kind::Background, |cx| async move {
            cx.ready();
            Err(Error::internal("ready service failed"))
        }))
        .with(service("initializing", Kind::Frontline, |cx| async move {
            cx.stopped().await;
            Ok(())
        }));
    let report = supervisor.run().await;
    assert!(
        matches!(report.outcome, Outcome::Failed(_)),
        "{:?}",
        report.outcome
    );
    assert_eq!(report.drain, Drain::Complete);
}

#[tokio::test]
async fn running_panic_and_frontline_early_return_are_faults() {
    for panic in [false, true] {
        let supervisor =
            Supervisor::default().with(service("front", Kind::Frontline, move |cx| async move {
                common::declare_ready(&cx).await;
                assert!(!panic, "running service panic");
                Ok(())
            }));
        assert!(matches!(supervisor.run().await.outcome, Outcome::Failed(_)));
    }
}

#[tokio::test]
async fn ready_is_idempotent_and_phases_do_not_move_back_after_stop_or_finish() {
    let (context_tx, context_rx) = tokio::sync::oneshot::channel();
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    let supervisor =
        Supervisor::default().with(service("front", Kind::Frontline, move |cx| async move {
            context_tx.send(cx.clone()).ok();
            cx.ready();
            cx.ready();
            cx.stopped().await;
            cx.ready();
            stopping_tx.send(()).ok();
            gate.await.ok();
            Ok(())
        }));
    let mut readiness = supervisor.readiness();
    let control = supervisor.control();
    assert_eq!(readiness.phase(), Phase::Starting);
    let run = tokio::spawn(supervisor.run());
    let cx = context_rx.await.unwrap();
    rivium_test::until_ready(&mut readiness).await.unwrap();
    cx.ready();
    assert_eq!(readiness.phase(), Phase::Running);
    control.stop(StopReason::Requested);
    stopping_rx.await.unwrap();
    assert_eq!(readiness.phase(), Phase::Stopping);
    assert!(!readiness.is_ready());
    release.send(()).unwrap();
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
    cx.ready();
    assert_eq!(
        cx.spawn("too late", async { Ok(()) }),
        Err(rivium::SubmitError::Closed)
    );
    control.stop(StopReason::Signal(2));
    control.restart("late");
    assert_eq!(readiness.phase(), Phase::Stopped);
    assert!(!readiness.is_ready());
    assert_eq!(readiness.drain(Duration::ZERO).await, Drain::Complete);
}

#[tokio::test]
async fn a_health_observer_can_request_restart_and_receive_its_reason() {
    let ((control, readiness, run), check) = healthy_store().await;
    check.set(Health::Unhealthy("disk full".into()));
    assert!(!readiness.is_ready());
    control.restart("configuration changed");
    let report = run.await.unwrap();
    assert!(matches!(report.outcome,Outcome::Restart(ref why) if why=="configuration changed"));
    assert_eq!(report.drain, Drain::Complete);
}

#[tokio::test]
async fn a_failure_during_stop_or_restart_replaces_only_the_successful_reason() {
    for restart in [false, true] {
        let supervisor =
            Supervisor::default().with(service("front", Kind::Frontline, move |cx| async move {
                common::declare_ready(&cx).await;
                if restart {
                    cx.control().restart("requested");
                } else {
                    cx.control().stop(StopReason::Requested);
                }
                cx.stopped().await;
                Err(Error::internal("shutdown failure"))
            }));
        assert!(matches!(supervisor.run().await.outcome, Outcome::Failed(_)));
    }
}

#[tokio::test]
async fn child_failure_keeps_its_kind_name_and_cause_when_the_root_also_fails() {
    let supervisor =
        Supervisor::default().with(service("owner", Kind::Background, |cx| async move {
            cx.ready();
            cx.spawn("connection", async {
                Err(Error::new(kinds::UNAVAILABLE, "connection reset"))
            })
            .unwrap();
            cx.stopped().await;
            Err(Error::internal("later root failure"))
        }));
    let report = supervisor.run().await;
    let Outcome::Failed(error) = report.outcome else {
        panic!("wrong outcome")
    };
    assert_eq!(error.kind(), kinds::UNAVAILABLE);
    let chain = format!("{error:#}");
    assert!(chain.contains("owner/connection"));
    assert!(chain.contains("connection reset"));
    assert!(!chain.contains("later root failure"));
}

#[tokio::test(start_paused = true)]
async fn a_stop_shortens_a_restart_deadline_and_timeout_remains_a_timeout() {
    let (context_tx, context_rx) = tokio::sync::oneshot::channel();
    let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_secs(1)).with(service(
        "front",
        Kind::Frontline,
        |cx| async move {
            cx.ready();
            cx.stopped().await;
            context_tx.send(cx).ok();
            std::future::pending().await
        },
    ));
    let (control, _readiness, run) = common::ready(supervisor).await;
    let start = tokio::time::Instant::now();
    control.restart("replace");
    let cx = context_rx.await.unwrap();
    assert_eq!(cx.deadline(), Some(start + Duration::from_secs(1)));
    control.stop(StopReason::Deadline(Duration::from_millis(20)));
    assert_eq!(cx.deadline(), Some(start + Duration::from_millis(20)));
    assert!(matches!(
        run.await.unwrap().outcome,
        Outcome::TimedOut { .. }
    ));
    assert_eq!(start.elapsed(), Duration::from_millis(20));
}

#[tokio::test(start_paused = true)]
async fn a_primary_failure_survives_a_later_stop_and_cleanup_deadline() {
    let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_millis(10))
        .with(service("primary", Kind::Frontline, |cx| async move {
            common::declare_ready(&cx).await;
            Err(Error::internal("primary"))
        }))
        .with(service("slow", Kind::Frontline, |cx| async move {
            cx.ready();
            cx.stopped().await;
            cx.control().stop(StopReason::Requested);
            cx.control().restart("ignored");
            std::future::pending().await
        }));
    let report = supervisor.run().await;
    assert!(matches!(
        report.outcome,
        Outcome::Failed(ref error) if format!("{error:#}").contains("primary")
    ));
    assert_eq!(report.drain, Drain::Complete);
}

#[test]
fn public_outcomes_are_stable_across_runtime_and_registration_order() {
    for multi in [false, true] {
        let mut builder = if multi {
            let mut builder = tokio::runtime::Builder::new_multi_thread();
            builder.worker_threads(2);
            builder
        } else {
            tokio::runtime::Builder::new_current_thread()
        };
        let runtime = builder.enable_all().build().unwrap();
        for reverse in [false, true] {
            for mode in [0, 1, 2] {
                let action = service("action", Kind::Frontline, move |cx| async move {
                    common::declare_ready(&cx).await;
                    match mode {
                        0 => cx.control().stop(StopReason::Requested),
                        1 => cx.control().restart("requested"),
                        _ => return Err(Error::internal("fault")),
                    };
                    cx.stopped().await;
                    Ok(())
                });
                let other = rivium_test::idle("other", Kind::Background);
                let supervisor = if reverse {
                    Supervisor::default().with(other).with(action)
                } else {
                    Supervisor::default().with(action).with(other)
                };
                let report = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(1), supervisor.run())
                        .await
                        .unwrap()
                });
                assert!(matches!(
                    (mode, report.outcome),
                    (0, Outcome::Stopped) | (1, Outcome::Restart(_)) | (2, Outcome::Failed(_))
                ));
                assert_eq!(report.drain, Drain::Complete);
            }
        }
    }
}

#[tokio::test]
async fn a_ready_background_completion_does_not_skip_another_roots_initialization() {
    let (finished, done) = tokio::sync::oneshot::channel();
    let (start, gate) = tokio::sync::oneshot::channel();
    let supervisor = Supervisor::default()
        .with(service("background", Kind::Background, |cx| async move {
            cx.ready();
            finished.send(()).ok();
            Ok(())
        }))
        .with(service("front", Kind::Frontline, |cx| async move {
            gate.await.ok();
            cx.ready();
            cx.stopped().await;
            Ok(())
        }));
    let (control, mut readiness, run) = common::spawn(supervisor);
    done.await.unwrap();
    while readiness.active_tasks() != 1 {
        tokio::task::yield_now().await;
    }
    assert_eq!(readiness.phase(), Phase::Starting);
    assert!(!readiness.is_ready());
    start.send(()).unwrap();
    rivium_test::until_ready(&mut readiness).await.unwrap();
    control.stop(StopReason::Requested);
    assert!(matches!(run.await.unwrap().outcome, Outcome::Stopped));
}

#[tokio::test]
async fn health_entries_remain_registered_after_their_last_update_handle_drops() {
    let ((control, readiness, run), check) = healthy_store().await;
    let registry = readiness.health();
    check.set(Health::Unhealthy("disk full".into()));
    drop(check);
    assert!(!readiness.is_ready());
    assert!(matches!(
        registry.snapshot().get("store"),
        Some(Health::Unhealthy(_))
    ));
    control.stop(StopReason::Requested);
    assert!(run.await.unwrap().drain.is_complete());
}

#[tokio::test(start_paused = true)]
async fn startup_timeout_and_stop_budgets_use_the_configured_clock_deadlines() {
    let start = tokio::time::Instant::now();
    let supervisor = Supervisor::new(Duration::from_millis(10), Duration::from_secs(1)).with(
        service("unready", Kind::Frontline, |cx| async move {
            cx.stopped().await;
            Ok(())
        }),
    );
    assert!(matches!(
        supervisor.run().await.outcome,
        Outcome::StartupFailed(_)
    ));
    assert_eq!(start.elapsed(), Duration::from_millis(10));
    for within in [
        Duration::ZERO,
        Duration::from_millis(1),
        Duration::from_secs(10),
    ] {
        let (send, receive) = tokio::sync::oneshot::channel();
        let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_millis(3)).with(
            service("pending", Kind::Frontline, |cx| async move {
                send.send(cx.clone()).ok();
                cx.ready();
                std::future::pending().await
            }),
        );
        let (control, mut readiness, run) = common::spawn(supervisor);
        let cx = receive.await.unwrap();
        rivium_test::until_ready(&mut readiness).await.unwrap();
        let start = tokio::time::Instant::now();
        let effective = within.min(Duration::from_millis(3));
        control.stop(StopReason::Deadline(within));
        assert_eq!(cx.deadline(), Some(start + effective));
        assert!(matches!(
            run.await.unwrap().outcome,
            Outcome::TimedOut { .. }
        ));
        assert_eq!(start.elapsed(), effective);
    }
}

#[tokio::test]
async fn dropping_the_driver_drops_both_root_and_admitted_child_captures() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let root = Arc::new(AtomicBool::new(false));
    let child = Arc::new(AtomicBool::new(false));
    let root_marker = common::Dropped(root.clone());
    let child_marker = common::Dropped(child.clone());
    let supervisor =
        Supervisor::default().with(service("owner", Kind::Frontline, |cx| async move {
            let _root = root_marker;
            cx.spawn("child", async move {
                let _child = child_marker;
                std::future::pending().await
            })
            .unwrap();
            cx.ready();
            std::future::pending().await
        }));
    let (_control, readiness, run) = common::ready(supervisor).await;
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert_eq!(
        readiness.drain(Duration::from_secs(1)).await,
        Drain::Complete
    );
    assert!(root.load(Ordering::Acquire));
    assert!(child.load(Ordering::Acquire));
}

#[tokio::test]
async fn every_service_stop_token_is_cancelled_after_timeout_abort_or_driver_drop() {
    for action in 0..3 {
        let (send, receive) = tokio::sync::oneshot::channel();
        let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_millis(1))
            .with(service("uncooperative", Kind::Frontline, |cx| async move {
                cx.ready();
                std::future::pending().await
            }))
            .with(service("background", Kind::Background, |cx| async move {
                send.send(cx.stop()).ok();
                cx.ready();
                cx.stopped().await;
                Ok(())
            }));
        let (control, mut readiness, run) = common::spawn(supervisor);
        let stop = receive.await.unwrap();
        rivium_test::until_ready(&mut readiness).await.unwrap();
        match action {
            0 => control.stop(StopReason::Requested),
            1 => {
                control.stop(StopReason::Requested);
                control.stop(StopReason::Signal(2));
            }
            _ => run.abort(),
        }

        let report = run.await;
        match action {
            0 => assert!(matches!(report.unwrap().outcome, Outcome::TimedOut { .. })),
            1 => assert!(matches!(report.unwrap().outcome, Outcome::Aborted(2))),
            _ => assert!(report.unwrap_err().is_cancelled()),
        }
        assert!(stop.is_stopping());
        assert_eq!(
            readiness.drain(Duration::from_secs(1)).await,
            Drain::Complete
        );
    }
}

#[tokio::test(start_paused = true)]
async fn restart_that_exceeds_grace_reports_timeout_instead_of_successful_replacement() {
    let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_millis(10)).with(
        service("front", Kind::Frontline, |cx| async move {
            cx.ready();
            cx.control().restart("replace");
            cx.stopped().await;
            std::future::pending().await
        }),
    );
    let report = supervisor.run().await;
    assert!(matches!(report.outcome, Outcome::TimedOut { .. }));
    assert_eq!(report.drain, Drain::Complete);
}

#[tokio::test]
async fn an_unready_root_error_panic_or_natural_completion_fails_startup() {
    for mode in 0..3 {
        let supervisor =
            Supervisor::default().with(service("unready", Kind::Frontline, move |_| async move {
                match mode {
                    0 => Err(Error::internal("initialization failed")),
                    1 => panic!("initialization panic"),
                    _ => Ok(()),
                }
            }));
        let report = supervisor.run().await;
        assert!(matches!(report.outcome, Outcome::StartupFailed(_)));
        assert_eq!(report.drain, Drain::Complete);
    }
}
