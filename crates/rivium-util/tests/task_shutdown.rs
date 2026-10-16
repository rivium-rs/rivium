#![cfg(feature = "task")]

use rivium::Result;
use rivium_util::task::{Context, Exit, Manager, Policy};
use std::time::Duration;
use tokio::sync::oneshot;

async fn idle(_: &'static str, _: u32, ctx: Context) -> Result<()> {
    ctx.cancel.cancelled().await;
    Ok(())
}

fn policy(capacity: usize) -> Policy {
    Policy {
        capacity,
        stop_timeout: Duration::from_millis(10),
        ..Default::default()
    }
}

async fn blocked_manager() -> (Manager<&'static str, u32>, Context) {
    let mut manager = Manager::new(policy(1)).unwrap();
    let mut original = None;
    manager
        .reconcile(vec![("worker", 1)], |_, _, ctx| {
            original = Some(ctx);
            std::future::pending()
        })
        .await
        .unwrap();
    let original = original.unwrap();
    (manager, original)
}

#[tokio::test(start_paused = true)]
async fn cancelled_round_preserves_previously_reaped_exit() {
    use std::{future::Future, task::Poll};
    let scope = rivium::scope::TaskScope::new();
    let mut manager = Manager::with_scope(policy(2), scope.clone()).unwrap();
    manager
        .reconcile(vec![("a", 1), ("b", 1)], |key, _, _| async move {
            if key == "b" {
                std::future::pending::<()>().await;
            }
            Ok(())
        })
        .await
        .unwrap();
    while scope.active_tasks() != 1 {
        tokio::task::yield_now().await;
    }
    let mut removing = Box::pin(manager.reconcile(vec![], idle));
    std::future::poll_fn(|cx| match removing.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(result) => panic!("b must still be pending: {result:?}"),
    })
    .await;
    drop(removing);
    let report = manager.reconcile(vec![], idle).await.unwrap();
    assert!(
        matches!(
            report.exits.as_slice(),
            [("a", Exit::Completed), ("b", Exit::Cancelled)]
        ),
        "the resumed round must deliver both exits exactly once: {:?}",
        report.exits
    );
    let replay = manager.reconcile(vec![], idle).await.unwrap();
    assert!(replay.exits.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_reconcile_cannot_revive_a_stopping_incarnation() {
    let (mut manager, original) = blocked_manager().await;
    let mut changing = Box::pin(manager.reconcile(vec![("worker", 2)], idle));
    tokio::select! {
        biased;
        result = &mut changing => panic!("old task should still be stopping: {result:?}"),
        _ = original.cancel.cancelled() => {}
    }
    drop(changing);

    // Reverting the desired specification must not keep the already-cancelled task.
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert_eq!(report.restarted, ["worker"]);
    assert!(report.unconfirmed.is_empty());
    assert!(matches!(
        report.exits.as_slice(),
        [("worker", Exit::Cancelled)]
    ));
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_shutdown_keeps_ownership_and_can_be_retried() {
    let (mut manager, original) = blocked_manager().await;
    let mut stopping = Box::pin(manager.shutdown());
    tokio::select! {
        biased;
        result = &mut stopping => panic!("old task should still be stopping: {result:?}"),
        _ = original.cancel.cancelled() => {}
    }
    drop(stopping);
    assert!(manager.reconcile(vec![("worker", 2)], idle).await.is_err());
    manager.shutdown().await.unwrap();
    assert!(manager.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconfirmed_exit_preserves_partial_report_and_occupies_capacity() {
    let mut manager = Manager::new(policy(2)).unwrap();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let (started, ready) = oneshot::channel();
    let (finished, done) = oneshot::channel();
    let mut blocked = Some((gate, started, finished));
    manager
        .reconcile(vec![("a", 1), ("b", 1)], |key, _, ctx| {
            let blocked = if key == "a" { blocked.take() } else { None };
            async move {
                if let Some((gate, started, finished)) = blocked {
                    let _ = started.send(());
                    let _ = gate.recv();
                    let _ = finished.send(());
                    std::future::pending::<()>().await;
                }
                ctx.cancel.cancelled().await;
                Ok(())
            }
        })
        .await
        .unwrap();
    ready.await.unwrap();

    let report = manager
        .reconcile(vec![("a", 2), ("c", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.unconfirmed, ["a"]);
    assert_eq!(report.removed, ["b"]);
    assert_eq!(report.started, ["c"]);
    assert!(report.restarted.is_empty());
    assert_eq!(manager.len(), 2);

    let report = manager
        .reconcile(vec![("c", 1), ("d", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.unconfirmed, ["a"]);
    assert_eq!(report.limited, ["d"]);
    assert!(report.restarted.is_empty());
    assert!(report.started.is_empty());
    drop(release);
    done.await.unwrap();

    let report = manager
        .reconcile(vec![("a", 1), ("c", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.restarted, ["a"]);
    assert!(report.unconfirmed.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn all_stops_share_one_abort_confirmation_budget() {
    let mut manager = Manager::new(policy(6)).unwrap();
    let mut releases = vec![];
    let mut started = vec![];
    let mut finished = vec![];
    manager
        .reconcile((0..6).map(|key| (key, 1)).collect(), |_, _, _| {
            let (release, gate) = std::sync::mpsc::channel::<()>();
            releases.push(release);
            let (ready, ready_rx) = oneshot::channel();
            let (done, done_rx) = oneshot::channel();
            started.push(ready_rx);
            finished.push(done_rx);
            async move {
                let _ = ready.send(());
                let _ = gate.recv();
                let _ = done.send(());
                std::future::pending().await
            }
        })
        .await
        .unwrap();
    for ready in started {
        ready.await.unwrap();
    }
    let start = std::time::Instant::now();
    let result = manager.shutdown().await;
    let elapsed = start.elapsed();
    let retained = manager.len();
    // Dropping the senders also releases blocked workers if any later assertion fails.
    drop(releases);
    for done in finished {
        done.await.unwrap();
    }
    manager.shutdown().await.unwrap();
    assert!(result.is_err());
    assert_eq!(retained, 6);
    assert!(manager.is_empty());
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed < Duration::from_millis(400), "elapsed: {elapsed:?}");
}

#[cfg(feature = "log")]
#[tokio::test]
async fn managed_tasks_inherit_the_scoped_log_dispatch() {
    use tracing::instrument::WithSubscriber;

    let logs = rivium_test::Logs::default();
    let mut manager = Manager::new(policy(1)).unwrap();
    let (started, ready) = oneshot::channel();
    let mut started = Some(started);
    manager
        .reconcile(vec![("worker", 1)], |_, _, ctx| {
            let started = started.take().unwrap();
            async move {
                tracing::info!("dynamic task has its owner's dispatch");
                let _ = started.send(());
                ctx.cancel.cancelled().await;
                Ok(())
            }
        })
        .with_subscriber(logs.dispatch())
        .await
        .unwrap();
    ready.await.unwrap();
    manager.shutdown().await.unwrap();
    assert!(
        logs.text()
            .contains("dynamic task has its owner's dispatch")
    );
}
