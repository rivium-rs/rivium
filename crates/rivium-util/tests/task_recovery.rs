#![cfg(feature = "task")]

use rivium::{Error, Result};
use rivium_util::task::{Context, Exit, Manager, Policy, Report};
use std::time::Duration;
use tokio::sync::oneshot;

fn policy(max_failures: u32) -> Policy {
    Policy {
        max_failures,
        backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(32),
        ..Default::default()
    }
}

async fn idle(_: &'static str, _: u32, context: Context) -> Result<()> {
    context.cancel.cancelled().await;
    Ok(())
}

struct Running {
    context: Context,
    exit: oneshot::Sender<Result<()>>,
    finished: oneshot::Receiver<()>,
}

impl Running {
    async fn finish(self, result: Result<()>) {
        self.exit.send(result).unwrap();
        self.finished.await.unwrap();
    }
}

async fn start(
    manager: &mut Manager<&'static str, u32>,
    spec: u32,
) -> (Report<&'static str>, Running) {
    let (exit, receiver) = oneshot::channel();
    let (finished, done) = oneshot::channel();
    let mut channels = Some((receiver, finished));
    let mut captured = None;
    let report = manager
        .reconcile(vec![("worker", spec)], |_, _, context| {
            captured = Some(context);
            let (receiver, finished) = channels.take().expect("one worker starts");
            async move {
                let result = receiver.await.unwrap();
                finished.send(()).unwrap();
                result
            }
        })
        .await
        .unwrap();
    (
        report,
        Running {
            context: captured.expect("worker admitted"),
            exit,
            finished: done,
        },
    )
}

#[tokio::test(start_paused = true)]
async fn completed_task_restarts_in_same_round_and_clears_failure_history() {
    let mut manager = Manager::new(policy(2)).unwrap();
    let (_, first) = start(&mut manager, 1).await;
    first.finish(Err(Error::internal("first failure"))).await;
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert!(report.restarted.is_empty());
    assert!(report.exhausted.is_empty());

    tokio::time::advance(Duration::from_secs(2)).await;
    let (_, second) = start(&mut manager, 1).await;
    second.finish(Ok(())).await;

    let (report, third) = start(&mut manager, 1).await;
    assert_eq!(report.restarted, ["worker"]);
    assert!(report.started.is_empty());
    assert!(matches!(
        report.exits.as_slice(),
        [("worker", Exit::Completed)]
    ));
    third
        .finish(Err(Error::internal("new failure streak")))
        .await;
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert!(report.exhausted.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn failure_limit_is_exact_and_exhaustion_is_reported_once() {
    let mut manager = Manager::new(policy(1)).unwrap();
    let (report, worker) = start(&mut manager, 1).await;
    assert_eq!(report.started, ["worker"]);
    let exhausted_context = worker.context.clone();
    worker.finish(Err(Error::invalid("invalid response"))).await;

    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert_eq!(report.exhausted, ["worker"]);
    assert!(report.restarted.is_empty());
    let [(key, Exit::Failed(error))] = report.exits.as_slice() else {
        panic!("expected the original task failure: {:?}", report.exits);
    };
    assert_eq!(*key, "worker");
    assert_eq!(error.kind(), rivium::error::kinds::INVALID_INPUT);
    assert_eq!(error.to_string(), "invalid response");

    exhausted_context.mark_progress();
    tokio::time::advance(Duration::from_secs(100)).await;
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert!(report.started.is_empty());
    assert!(report.restarted.is_empty());
    assert!(report.exits.is_empty());
    assert!(report.exhausted.is_empty());

    let (report, replacement) = start(&mut manager, 2).await;
    assert_eq!(report.restarted, ["worker"]);
    assert!(report.exhausted.is_empty());
    replacement
        .finish(Err(Error::internal("changed spec failed")))
        .await;
    let report = manager.reconcile(vec![("worker", 2)], idle).await.unwrap();
    assert_eq!(report.exhausted, ["worker"]);
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn panic_retains_join_error_and_consumes_failure_budget() {
    let mut manager = Manager::new(policy(1)).unwrap();
    let (finished, done) = oneshot::channel();
    let mut finished = Some(finished);
    manager
        .reconcile(vec![("worker", 1)], |_, _, _| {
            let finished = finished.take().unwrap();
            async move {
                finished.send(()).unwrap();
                panic!("worker panic");
            }
        })
        .await
        .unwrap();
    done.await.unwrap();

    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert_eq!(report.exhausted, ["worker"]);
    let [("worker", Exit::Panicked(error))] = report.exits.as_slice() else {
        panic!("expected panic classification: {:?}", report.exits);
    };
    assert!(error.is_panic());
    assert!(report.restarted.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn progress_resets_failures_even_when_observed_only_after_exit() {
    for observe_while_running in [false, true] {
        let mut manager = Manager::new(policy(2)).unwrap();
        let (_, first) = start(&mut manager, 1).await;
        first.finish(Err(Error::internal("first failure"))).await;
        manager.reconcile(vec![("worker", 1)], idle).await.unwrap();

        tokio::time::advance(Duration::from_secs(2)).await;
        let (_, second) = start(&mut manager, 1).await;
        second.context.mark_progress();
        if observe_while_running {
            let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
            assert!(report.exits.is_empty());
            assert!(report.restarted.is_empty());
        }
        second
            .finish(Err(Error::internal("failed after progress")))
            .await;

        let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
        assert!(report.exhausted.is_empty());
        tokio::time::advance(Duration::from_secs(2)).await;
        let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
        assert_eq!(report.restarted, ["worker"]);
        manager.shutdown().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn old_context_cannot_reset_a_new_instances_failure_history() {
    let mut manager = Manager::new(policy(2)).unwrap();
    let (_, first) = start(&mut manager, 1).await;
    let old_context = first.context.clone();
    first.finish(Err(Error::internal("first failure"))).await;
    manager.reconcile(vec![("worker", 1)], idle).await.unwrap();

    tokio::time::advance(Duration::from_secs(2)).await;
    let (_, second) = start(&mut manager, 1).await;
    old_context.mark_progress();
    old_context.cancel.cancel();
    assert!(!second.context.cancel.is_cancelled());
    second.finish(Err(Error::internal("second failure"))).await;

    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert_eq!(report.exhausted, ["worker"]);
    assert!(report.restarted.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn retry_backoff_starts_when_exit_is_observed() {
    let mut manager = Manager::new(policy(2)).unwrap();
    let (_, worker) = start(&mut manager, 1).await;
    worker.finish(Err(Error::internal("retry later"))).await;
    tokio::time::advance(Duration::from_secs(20)).await;

    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert!(report.restarted.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert!(report.restarted.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    let report = manager.reconcile(vec![("worker", 1)], idle).await.unwrap();
    assert_eq!(report.restarted, ["worker"]);
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn report_distinguishes_removal_replacement_and_first_start() {
    let mut manager = Manager::new(policy(2)).unwrap();
    let report = manager
        .reconcile(vec![("a", 1), ("b", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.started, ["a", "b"]);

    let report = manager
        .reconcile(vec![("a", 2), ("c", 1)], idle)
        .await
        .unwrap();
    assert_eq!(report.removed, ["b"]);
    assert_eq!(report.restarted, ["a"]);
    assert_eq!(report.started, ["c"]);
    assert!(report.unconfirmed.is_empty());
    assert!(matches!(
        report.exits.as_slice(),
        [("a", Exit::Completed), ("b", Exit::Completed)]
    ));

    let report = manager
        .reconcile(vec![("a", 2), ("c", 1)], idle)
        .await
        .unwrap();
    assert!(report.started.is_empty());
    assert!(report.restarted.is_empty());
    assert!(report.removed.is_empty());
    assert!(report.exits.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn oversize_rejection_does_not_consume_failure_budget() {
    let mut manager = Manager::new(Policy {
        capacity: 1,
        ..policy(1)
    })
    .unwrap();
    manager.reconcile(vec![("a", 1)], idle).await.unwrap();
    for _ in 0..3 {
        let error = manager
            .reconcile(vec![("a", 1), ("b", 1)], idle)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            rivium_util::task::ReconcileError::InvalidDesired(
                rivium_util::task::DesiredError::TooMany {
                    requested: 2,
                    capacity: 1
                }
            )
        ));
        assert_eq!(manager.len(), 1);
    }
    let report = manager.reconcile(vec![("b", 1)], idle).await.unwrap();
    assert_eq!(report.removed, ["a"]);
    assert_eq!(report.started, ["b"]);
    assert!(report.exhausted.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn requested_stops_preserve_error_and_panic_without_exhaustion() {
    let mut manager = Manager::new(policy(1)).unwrap();
    manager
        .reconcile(vec![("a", 1), ("b", 1)], |key, _, context| async move {
            context.cancel.cancelled().await;
            if key == "a" {
                return Err(Error::internal("cleanup failed"));
            }
            panic!("cleanup panicked");
        })
        .await
        .unwrap();

    let report = manager.reconcile(vec![], idle).await.unwrap();
    assert_eq!(report.removed, ["a", "b"]);
    assert!(report.exhausted.is_empty());
    assert!(report.unconfirmed.is_empty());
    assert!(matches!(
        report.exits.as_slice(),
        [("a", Exit::Failed(_)), ("b", Exit::Panicked(_))]
    ));
    assert!(manager.is_empty());
    manager.shutdown().await.unwrap();
}
