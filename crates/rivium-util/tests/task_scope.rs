#![cfg(feature = "task")]

use rivium::{Kind, Outcome, Supervisor, rt::StopReason, scope::TaskScope, service};
use rivium_util::task::{
    Context, DesiredSource, DriveOptions, DriveRound, Lifecycle, Manager, Policy, drive,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

async fn idle(_: &'static str, _: u32, ctx: Context) -> rivium::Result<()> {
    ctx.cancel.cancelled().await;
    Ok(())
}

fn scoped_manager() -> (TaskScope, Manager<&'static str, u32>) {
    let scope = TaskScope::new();
    let manager = Manager::with_scope(Policy::default(), scope.clone()).unwrap();
    (scope, manager)
}

#[tokio::test]
async fn factory_panic_releases_unused_reservations_and_keeps_started_tasks_owned() {
    use std::{future::Future, panic::AssertUnwindSafe, task::Waker};

    let (scope, mut manager) = scoped_manager();
    let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut round = Box::pin(manager.reconcile(
            vec![("a", 1), ("b", 1), ("c", 1)],
            |key, spec, context| {
                assert_ne!(key, "b", "scripted factory panic");
                idle(key, spec, context)
            },
        ));
        let _ = round
            .as_mut()
            .poll(&mut std::task::Context::from_waker(Waker::noop()));
    }));
    assert!(panic.is_err());
    assert_eq!(scope.active_tasks(), 1);
    assert_eq!(manager.len(), 1);
    manager.shutdown().await.unwrap();
    assert_eq!(scope.active_tasks(), 0);
}

#[tokio::test]
async fn shutting_down_one_manager_keeps_other_managers_admission_open() {
    let activity = TaskScope::default();
    let mut a = Manager::with_scope(Policy::default(), activity.clone()).unwrap();
    let mut b = Manager::with_scope(Policy::default(), activity.clone()).unwrap();
    a.reconcile(vec![("a", 1)], idle).await.unwrap();
    b.reconcile(vec![("b", 1)], idle).await.unwrap();
    assert_eq!(activity.active_tasks(), 2);
    a.shutdown().await.unwrap();
    assert_eq!(activity.active_tasks(), 1);
    b.reconcile(vec![("b", 1), ("c", 1)], idle).await.unwrap();
    assert_eq!(activity.active_tasks(), 2);
    activity.close();
    assert_eq!(
        b.reconcile(vec![("b", 1), ("c", 1), ("d", 1)], idle)
            .await
            .unwrap()
            .lifecycle,
        Lifecycle::Closed
    );
    assert_eq!(activity.active_tasks(), 2);
    b.shutdown().await.unwrap();
    assert_eq!(activity.active_tasks(), 0);
}

#[tokio::test]
async fn abort_before_first_poll_releases_activity_and_captured_resources() {
    let (activity, mut manager) = scoped_manager();
    let resource = Arc::new(());
    let held = resource.clone();
    let mut held = Some(held);
    manager
        .reconcile(vec![("worker", 1)], |_, _, _| {
            let held = held.take().unwrap();
            async move {
                let _held = held;
                std::future::pending().await
            }
        })
        .await
        .unwrap();
    assert_eq!(activity.active_tasks(), 1);
    drop(manager);
    tokio::time::timeout(Duration::from_secs(5), async {
        while activity.active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(Arc::strong_count(&resource), 1);
}

struct Resource {
    activity: TaskScope,
    released: Option<oneshot::Sender<usize>>,
}

impl Drop for Resource {
    fn drop(&mut self) {
        let _ = self
            .released
            .take()
            .unwrap()
            .send(self.activity.active_tasks());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_abort_keeps_dynamic_child_counted_until_its_resources_are_released() {
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (released_tx, released_rx) = oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(5), Duration::from_millis(10)).with(service(
        "parent",
        Kind::Frontline,
        move |ctx| async move {
            let activity = ctx.task_scope();
            let mut manager = Manager::with_scope(Policy::default(), activity.clone())?;
            let mut child = Some((
                gate,
                ready_tx,
                Resource {
                    activity,
                    released: Some(released_tx),
                },
            ));
            manager
                .reconcile(vec![("worker", 1)], |_, _, _| {
                    let (gate, ready, resource) = child.take().unwrap();
                    async move {
                        let _resource = resource;
                        let _ = ready.send(());
                        let _ = gate.recv();
                        std::future::pending().await
                    }
                })
                .await?;
            ctx.ready();
            // The supervisor must abort this parent; normal shutdown is deliberately bypassed.
            std::future::pending::<()>().await;
            manager.shutdown().await
        },
    ));
    let readiness = sup.readiness();
    let control = sup.control();
    let parent = tokio::spawn(sup.run());
    ready_rx.await.unwrap();
    control.stop(StopReason::Requested);
    let outcome = tokio::time::timeout(Duration::from_secs(5), parent)
        .await
        .unwrap()
        .unwrap();
    let remaining = readiness.active_tasks();
    // Release the worker before assertions so failures cannot leave the test runtime hung.
    drop(release);
    let during_resource_drop = released_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while readiness.active_tasks() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(outcome.outcome, Outcome::TimedOut { .. }));
    assert_eq!(remaining, 1);
    assert_eq!(during_resource_drop, 1);
}

struct GatedSource {
    desired: Vec<(&'static str, u32)>,
    entered: Option<oneshot::Sender<()>>,
    release: Option<oneshot::Receiver<()>>,
}

impl GatedSource {
    fn immediate(desired: Vec<(&'static str, u32)>) -> Self {
        Self {
            desired,
            entered: None,
            release: None,
        }
    }
}

impl DesiredSource for GatedSource {
    type Key = &'static str;
    type Spec = u32;

    async fn list_desired(&mut self) -> rivium::Result<Vec<(Self::Key, Self::Spec)>> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
        }
        if let Some(release) = self.release.take() {
            release.await.unwrap();
        }
        Ok(self.desired.clone())
    }

    fn tick_interval(&mut self) -> Duration {
        Duration::from_secs(60)
    }
}

#[tokio::test(start_paused = true)]
async fn stop_during_frontline_replacement_preserves_stopped_outcome() {
    let (old_cancelled, old_cancelled_rx) = oneshot::channel();
    let (release_old, release_old_rx) = oneshot::channel();
    let (drive_cancelled, drive_cancelled_rx) = oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(5), Duration::from_secs(5)).with(service(
        "frontline",
        Kind::Frontline,
        move |ctx| async move {
            let mut manager = Manager::with_scope(Policy::default(), ctx.task_scope())?;
            let mut old = Some((old_cancelled, release_old_rx));
            manager
                .reconcile(vec![("worker", 1)], |_, _, worker: Context| {
                    let (cancelled, release) = old.take().unwrap();
                    async move {
                        worker.cancel.cancelled().await;
                        let _ = cancelled.send(());
                        release.await.unwrap();
                        Ok(())
                    }
                })
                .await?;
            ctx.ready();
            let cancel = CancellationToken::new();
            let stop = ctx.stop();
            let (_wake_tx, wake) = watch::channel(0);
            let (result, ()) = tokio::join!(
                drive(
                    &mut manager,
                    GatedSource::immediate(vec![("worker", 2)]),
                    idle,
                    wake,
                    cancel.clone(),
                    DriveOptions::default(),
                    |_| async { Ok(()) }
                ),
                async {
                    stop.stopped().await;
                    cancel.cancel();
                    let _ = drive_cancelled.send(());
                }
            );
            assert!(manager.is_empty());
            result
        },
    ));
    let control = sup.control();
    let readiness = sup.readiness();
    let run = tokio::spawn(sup.run());
    old_cancelled_rx.await.unwrap();
    control.stop(StopReason::Requested);
    drive_cancelled_rx.await.unwrap();
    release_old.send(()).unwrap();
    let outcome = run.await.unwrap();
    assert!(matches!(outcome.outcome, Outcome::Stopped), "{outcome:?}");
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test(start_paused = true)]
async fn background_admission_stays_open_until_frontline_drains() {
    let (front_stopping, front_stopping_rx) = oneshot::channel();
    let (release_front, release_front_rx) = oneshot::channel();
    let (source_entered, source_entered_rx) = oneshot::channel();
    let (release_source, release_source_rx) = oneshot::channel();
    let (attempted, attempted_rx) = oneshot::channel();
    let (child_cancelled, mut child_cancelled_rx) = oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(5), Duration::from_secs(5))
        .with(service(
            "frontline",
            Kind::Frontline,
            move |ctx| async move {
                ctx.ready();
                ctx.stopped().await;
                let _ = front_stopping.send(());
                release_front_rx.await.unwrap();
                Ok(())
            },
        ))
        .with(service(
            "background",
            Kind::Background,
            move |ctx| async move {
                let mut manager = Manager::with_scope(Policy::default(), ctx.task_scope())?;
                let mut child_cancelled = Some(child_cancelled);
                manager
                    .reconcile(vec![("existing", 1)], |_, _, worker: Context| {
                        let cancelled = child_cancelled.take().unwrap();
                        async move {
                            worker.cancel.cancelled().await;
                            let _ = cancelled.send(());
                            Ok(())
                        }
                    })
                    .await?;
                ctx.ready();
                let cancel = CancellationToken::new();
                let stop = ctx.stop();
                let (_wake_tx, wake) = watch::channel(0);
                let mut attempted = Some(attempted);
                let (result, ()) = tokio::join!(
                    drive(
                        &mut manager,
                        GatedSource {
                            desired: vec![("existing", 1), ("new", 1)],
                            entered: Some(source_entered),
                            release: Some(release_source_rx),
                        },
                        |key, spec, worker| {
                            assert!(!ctx.is_stopping(), "background must still serve frontline");
                            let _ = attempted.take().unwrap().send(());
                            idle(key, spec, worker)
                        },
                        wake,
                        cancel.clone(),
                        DriveOptions::default(),
                        |_| async { Ok(()) }
                    ),
                    async {
                        stop.stopped().await;
                        cancel.cancel();
                    }
                );
                assert!(manager.is_empty());
                result
            },
        ));
    let control = sup.control();
    let readiness = sup.readiness();
    let run = tokio::spawn(sup.run());
    source_entered_rx.await.unwrap();
    control.stop(StopReason::Requested);
    front_stopping_rx.await.unwrap();
    release_source.send(()).unwrap();
    attempted_rx.await.unwrap();
    let remained_alive = tokio::time::timeout(Duration::from_millis(1), &mut child_cancelled_rx)
        .await
        .is_err();
    release_front.send(()).unwrap();
    let outcome = run.await.unwrap();
    assert!(
        remained_alive,
        "background child stopped before frontline drained"
    );
    child_cancelled_rx.await.unwrap();
    assert!(matches!(outcome.outcome, Outcome::Stopped), "{outcome:?}");
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test(start_paused = true)]
async fn genuine_callback_error_during_stop_still_fails_supervisor() {
    let (callback_entered, callback_entered_rx) = oneshot::channel();
    let (release_callback, release_callback_rx) = oneshot::channel();
    let (drive_cancelled, drive_cancelled_rx) = oneshot::channel();
    let sup = Supervisor::new(Duration::from_secs(5), Duration::from_secs(5)).with(service(
        "frontline",
        Kind::Frontline,
        move |ctx| async move {
            let mut manager = Manager::with_scope(Policy::default(), ctx.task_scope())?;
            ctx.ready();
            let cancel = CancellationToken::new();
            let stop = ctx.stop();
            let (_wake_tx, wake) = watch::channel(0);
            let mut callback = Some((callback_entered, release_callback_rx));
            let (result, ()) = tokio::join!(
                drive(
                    &mut manager,
                    GatedSource::immediate(vec![("worker", 1)]),
                    idle,
                    wake,
                    cancel.clone(),
                    DriveOptions::default(),
                    |_| {
                        let (entered, release) = callback.take().unwrap();
                        async move {
                            let _ = entered.send(());
                            release.await.unwrap();
                            Err(rivium::Error::internal("real callback failure"))
                        }
                    }
                ),
                async {
                    stop.stopped().await;
                    cancel.cancel();
                    let _ = drive_cancelled.send(());
                }
            );
            assert!(manager.is_empty());
            result
        },
    ));
    let control = sup.control();
    let readiness = sup.readiness();
    let run = tokio::spawn(sup.run());
    callback_entered_rx.await.unwrap();
    control.stop(StopReason::Requested);
    drive_cancelled_rx.await.unwrap();
    release_callback.send(()).unwrap();
    let outcome = run.await.unwrap();
    let Outcome::Failed(error) = outcome.outcome else {
        panic!("{outcome:?}")
    };
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains("real callback failure"))
    );
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn closed_scope_rejects_remove_and_add_before_destructive_changes_or_factory() {
    let (scope, mut manager) = scoped_manager();
    let (cancelled, mut cancelled_rx) = oneshot::channel();
    let mut cancelled = Some(cancelled);
    manager
        .reconcile(vec![("old", 1)], |_, _, ctx| {
            let cancelled = cancelled.take().unwrap();
            async move {
                ctx.cancel.cancelled().await;
                let _ = cancelled.send(());
                Ok(())
            }
        })
        .await
        .unwrap();
    scope.close();
    let report = manager
        .reconcile(vec![("new", 1)], |_, _, _| {
            panic!("rejected round must not construct futures");
            #[allow(unreachable_code)]
            async {
                Ok(())
            }
        })
        .await
        .unwrap();
    assert_eq!(report.lifecycle, Lifecycle::Closed);
    assert!(report.removed.is_empty());
    assert!(report.started.is_empty());
    assert!(report.exits.is_empty());
    assert_eq!(manager.len(), 1);
    assert_eq!(scope.active_tasks(), 1);
    assert!(matches!(
        cancelled_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    manager.shutdown().await.unwrap();
    cancelled_rx.await.unwrap();
    assert_eq!(scope.active_tasks(), 0);
}

#[tokio::test]
async fn admitted_round_survives_close_and_reports_remove_and_replacement() {
    let (scope, mut manager) = scoped_manager();
    manager
        .reconcile(vec![("removed", 1), ("replacement", 1)], |_, _, ctx| {
            let scope = scope.clone();
            async move {
                ctx.cancel.cancelled().await;
                scope.close();
                Ok(())
            }
        })
        .await
        .unwrap();
    let report = manager
        .reconcile(vec![("replacement", 2), ("added", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.lifecycle, Lifecycle::Open);
    assert_eq!(report.removed, vec!["removed"]);
    assert_eq!(report.restarted, vec!["replacement"]);
    assert_eq!(report.started, vec!["added"]);
    assert_eq!(report.exits.len(), 2);
    assert_eq!(scope.active_tasks(), 2);
    let denied = manager
        .reconcile(vec![("replacement", 3)], idle)
        .await
        .unwrap();
    assert_eq!(denied.lifecycle, Lifecycle::Closed);
    assert!(denied.exits.is_empty());
    assert_eq!(manager.len(), 2);
    manager.shutdown().await.unwrap();
    assert_eq!(scope.active_tasks(), 0);
}

#[tokio::test]
async fn closed_round_delivers_observed_exits_then_shuts_down() {
    let (scope, mut manager) = scoped_manager();
    manager
        .reconcile(vec![("finished", 1)], |_, _, _| async { Ok(()) })
        .await
        .unwrap();
    while scope.active_tasks() != 0 {
        tokio::task::yield_now().await;
    }
    scope.close();
    let (_wake_tx, wake) = watch::channel(0);
    let mut callbacks = 0;
    drive(
        &mut manager,
        GatedSource::immediate(vec![("added", 1)]),
        idle,
        wake,
        CancellationToken::new(),
        DriveOptions::default(),
        |round| {
            callbacks += 1;
            let DriveRound::Reconciled(report) = round else {
                panic!("expected report")
            };
            assert_eq!(report.lifecycle, Lifecycle::Closed);
            assert_eq!(report.exits.len(), 1);
            assert_eq!(report.exits[0].0, "finished");
            assert!(report.removed.is_empty());
            async { Ok(()) }
        },
    )
    .await
    .unwrap();
    assert_eq!(callbacks, 1);
    assert!(manager.is_empty());
}

#[tokio::test]
async fn drive_observes_owner_scope_cancellation_without_a_bridge() {
    let (scope, mut manager) = scoped_manager();
    let (_wake_tx, wake) = watch::channel(0);
    drive(
        &mut manager,
        GatedSource::immediate(vec![("child", 1)]),
        idle,
        wake,
        CancellationToken::new(),
        DriveOptions::default(),
        |_| {
            scope.cancel();
            async { Ok(()) }
        },
    )
    .await
    .unwrap();
    assert!(manager.is_empty());
    assert_eq!(scope.active_tasks(), 0);
}

#[tokio::test]
async fn dropping_reconcile_releases_unused_reservations_but_keeps_old_task_owned() {
    let (scope, mut manager) = scoped_manager();
    let (stopping, stopping_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let mut child = Some((stopping, release_rx));
    manager
        .reconcile(vec![("old", 1)], |_, _, ctx| {
            let (stopping, release) = child.take().unwrap();
            async move {
                ctx.cancel.cancelled().await;
                let _ = stopping.send(());
                let _ = release.await;
                Ok(())
            }
        })
        .await
        .unwrap();
    {
        let round = manager.reconcile(vec![("old", 2), ("new", 1)], idle);
        tokio::pin!(round);
        tokio::select! {
            _ = stopping_rx => {},
            _ = &mut round => panic!("old task has not acknowledged exit"),
        }
        assert_eq!(scope.active_tasks(), 3);
    }
    assert_eq!(scope.active_tasks(), 1);
    assert_eq!(manager.len(), 1);
    release.send(()).unwrap();
    manager.shutdown().await.unwrap();
    assert_eq!(scope.active_tasks(), 0);
}
