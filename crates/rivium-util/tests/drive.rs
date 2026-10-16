#![cfg(feature = "task")]

use rivium::{Error, Result};
use rivium_util::task::{Context, DesiredSource, DriveOptions, DriveRound, Manager, Policy, drive};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

async fn idle(_: &'static str, _: u32, ctx: Context) -> Result<()> {
    ctx.cancel.cancelled().await;
    Ok(())
}

struct Source {
    rounds: usize,
    intervals: Vec<Duration>,
    fail_first: bool,
}

enum FixedSource {
    Duplicate,
    Timeout,
    Replacement,
}

impl DesiredSource for FixedSource {
    type Key = &'static str;
    type Spec = u32;

    async fn list_desired(&mut self) -> Result<Vec<(Self::Key, Self::Spec)>> {
        match self {
            Self::Duplicate => Ok(vec![("worker", 1), ("worker", 2)]),
            Self::Timeout => std::future::pending().await,
            Self::Replacement => Ok(vec![("worker", 2)]),
        }
    }

    fn tick_interval(&mut self) -> Duration {
        Duration::from_secs(if matches!(self, Self::Replacement) {
            60
        } else {
            1
        })
    }
}

#[tokio::test]
async fn rejected_desired_round_keeps_the_existing_task_until_explicit_stop() {
    let mut manager = Manager::new(Policy::default()).unwrap();
    let mut stopped = seed_cancellable_task(&mut manager).await;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let (_wake, receiver) = watch::channel(0);
    let mut rounds = 0;
    drive(
        &mut manager,
        FixedSource::Duplicate,
        |_, _, _| async { panic!("rejected candidate cannot start work") },
        receiver,
        cancel,
        DriveOptions::default(),
        |round| {
            assert!(matches!(round, DriveRound::DesiredRejected(_)));
            assert!(matches!(
                stopped.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            rounds += 1;
            stop.cancel();
            async { Ok(()) }
        },
    )
    .await
    .unwrap();
    assert_eq!(rounds, 1);
    stopped.await.unwrap();
    assert!(manager.is_empty());
}

impl DesiredSource for Source {
    type Key = &'static str;
    type Spec = u32;

    fn list_desired(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Vec<(Self::Key, Self::Spec)>>> + Send {
        self.rounds += 1;
        let failed = self.fail_first && self.rounds == 1;
        async move {
            if failed {
                Err(Error::new(
                    rivium::error::kinds::UNAVAILABLE,
                    "source offline",
                ))
            } else {
                Ok(vec![("worker", 1)])
            }
        }
    }

    fn tick_interval(&mut self) -> Duration {
        self.intervals
            .get(self.rounds.saturating_sub(1))
            .copied()
            .unwrap_or(Duration::from_secs(60))
    }
}

type BackgroundDriver = (
    watch::Sender<u64>,
    CancellationToken,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<Result<()>>,
);

fn drive_in_background(
    mut manager: Manager<&'static str, u32>,
    source: Source,
) -> BackgroundDriver {
    let (wake_tx, wake) = watch::channel(0_u64);
    let cancel = CancellationToken::new();
    let drive_cancel = cancel.clone();
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let seen = outcomes.clone();
    let task = tokio::spawn(async move {
        drive(
            &mut manager,
            source,
            idle,
            wake,
            drive_cancel,
            DriveOptions {
                list_timeout: Duration::from_secs(5),
            },
            move |round| {
                seen.lock().unwrap().push(match round {
                    DriveRound::SourceFailed(error) => error.to_string(),
                    DriveRound::Reconciled(_) => "reconciled".into(),
                    DriveRound::SourceTimedOut { .. } => "timeout".into(),
                    DriveRound::DesiredRejected(error) => error.to_string(),
                });
                async { Ok(()) }
            },
        )
        .await
    });
    (wake_tx, cancel, outcomes, task)
}

#[tokio::test(start_paused = true)]
async fn drive_runs_immediately_and_uses_each_round_interval() {
    let manager = Manager::new(Policy::default()).unwrap();
    let source = Source {
        rounds: 0,
        intervals: vec![Duration::from_secs(60), Duration::from_secs(10)],
        fail_first: false,
    };
    let (wake_tx, cancel, rounds, task) = drive_in_background(manager, source);
    tokio::task::yield_now().await;
    assert_eq!(rounds.lock().unwrap().len(), 1);
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(rounds.lock().unwrap().len(), 1);
    wake_tx.send_replace(1);
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert_eq!(rounds.lock().unwrap().len(), 2);
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn source_failures_are_reported_and_keep_existing_tasks() {
    let mut manager = Manager::new(Policy::default()).unwrap();
    let mut stopped = seed_cancellable_task(&mut manager).await;
    let source = Source {
        rounds: 0,
        intervals: vec![Duration::from_secs(1)],
        fail_first: true,
    };
    let (wake_tx, cancel, outcomes, task) = drive_in_background(manager, source);
    tokio::task::yield_now().await;
    assert_eq!(outcomes.lock().unwrap().as_slice(), ["source offline"]);
    assert!(matches!(
        stopped.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    wake_tx.send_replace(1);
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert_eq!(outcomes.lock().unwrap().len(), 2);
    cancel.cancel();
    task.await.unwrap().unwrap();
    stopped.await.unwrap();
}

#[tokio::test]
async fn desired_rejection_callback_error_shuts_down_existing_tasks_before_returning() {
    let error = drive_failure(FixedSource::Duplicate, Duration::from_secs(5), |round| {
        let DriveRound::DesiredRejected(error) = round else {
            panic!("expected rejected input")
        };
        Err(Error::invalid(error.to_string()).with_source(error))
    })
    .await;
    assert_eq!(error.class(), rivium::error::Class::InvalidInput);
}

#[tokio::test]
async fn callback_and_zero_interval_errors_also_shutdown_tasks() {
    for zero_interval in [false, true] {
        let source = Source {
            rounds: 0,
            intervals: vec![if zero_interval {
                Duration::ZERO
            } else {
                Duration::from_secs(1)
            }],
            fail_first: false,
        };
        let error = drive_failure(source, Duration::from_secs(5), |_| {
            if zero_interval {
                Ok(())
            } else {
                Err(Error::internal("callback failed"))
            }
        })
        .await;
        assert!(error.to_string().contains(if zero_interval {
            "tick interval"
        } else {
            "callback failed"
        }));
    }
}

#[tokio::test]
async fn timeout_callback_error_also_shuts_down_tasks() {
    let error = drive_failure(FixedSource::Timeout, Duration::from_millis(1), |_| {
        Err(Error::internal("timeout callback failed"))
    })
    .await;
    assert!(error.to_string().contains("timeout callback failed"));
}

async fn seed_cancellable_task(
    manager: &mut Manager<&'static str, u32>,
) -> tokio::sync::oneshot::Receiver<()> {
    let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel();
    let (started, ready) = tokio::sync::oneshot::channel();
    let mut signals = Some((cancelled_tx, started));
    manager
        .reconcile(vec![("worker", 1)], |_, _, ctx: Context| {
            let (tx, started) = signals.take().unwrap();
            async move {
                started.send(()).unwrap();
                ctx.cancel.cancelled().await;
                let _ = tx.send(());
                Ok(())
            }
        })
        .await
        .unwrap();
    ready.await.unwrap();
    cancelled_rx
}

#[tokio::test]
async fn source_failure_callback_error_shuts_down_existing_tasks() {
    let source = Source {
        rounds: 0,
        intervals: vec![Duration::from_secs(1)],
        fail_first: true,
    };
    let error = drive_failure(source, Duration::from_secs(5), |round| {
        assert!(matches!(round, DriveRound::SourceFailed(_)));
        Err(Error::internal("source callback failed"))
    })
    .await;
    assert_eq!(error.to_string(), "source callback failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconfirmed_cleanup_error_keeps_original_error_and_task_ownership() {
    for callback_fails in [false, true] {
        let mut manager = Manager::new(Policy {
            stop_timeout: Duration::from_millis(10),
            capacity: 1,
            ..Default::default()
        })
        .unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let mut channels = Some((release_rx, started_tx, finished_tx));
        manager
            .reconcile(vec![("worker", 1)], |_, _, ctx: Context| {
                let (release, started, finished) = channels.take().unwrap();
                async move {
                    let _ = started.send(());
                    let _ = release.recv();
                    let _ = finished.send(());
                    ctx.cancel.cancelled().await;
                    Ok(())
                }
            })
            .await
            .unwrap();
        started_rx.await.unwrap();

        let (_wake_tx, wake) = watch::channel(0_u64);
        let mut saw_unconfirmed = false;
        let error = drive(
            &mut manager,
            FixedSource::Replacement,
            |_, _, _: Context| async {
                panic!("replacement must not start before the blocked incarnation exits");
            },
            wake,
            CancellationToken::new(),
            DriveOptions::default(),
            |round| {
                let DriveRound::Reconciled(report) = round else {
                    panic!("expected reconciliation report");
                };
                assert_eq!(report.unconfirmed, ["worker"]);
                assert!(report.restarted.is_empty());
                saw_unconfirmed = true;
                async move {
                    if callback_fails {
                        Err(Error::internal("after round failed"))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await
        .unwrap_err();

        // Release before assertions so a failed assertion cannot strand a runtime worker.
        drop(release_tx);
        assert!(saw_unconfirmed);
        assert_eq!(manager.len(), 1);
        assert!(error.to_string().contains("manager shutdown failed"));
        let original = error.chain().last().unwrap().to_string();
        if callback_fails {
            assert_eq!(error.kind(), rivium::error::kinds::INTERNAL);
            assert_eq!(original, "after round failed");
        } else {
            assert_eq!(error.kind(), rivium::error::kinds::TIMEOUT);
            assert_eq!(
                error
                    .chain()
                    .last()
                    .unwrap()
                    .downcast_ref::<Error>()
                    .unwrap()
                    .kind(),
                rivium::error::kinds::TIMEOUT
            );
        }
        assert!(error.chain().count() >= 2);
        finished_rx.await.unwrap();
        manager.shutdown().await.unwrap();
        assert!(manager.is_empty());
    }
}

#[tokio::test]
async fn zero_list_timeout_shuts_down_existing_tasks() {
    let error = drive_failure(FixedSource::Timeout, Duration::ZERO, |_| {
        panic!("invalid options must be rejected before reading the source")
    })
    .await;
    assert!(error.to_string().contains("list timeout"));
}

async fn drive_failure(
    source: impl DesiredSource<Key = &'static str, Spec = u32>,
    list_timeout: Duration,
    mut callback: impl FnMut(DriveRound<&'static str>) -> Result<()>,
) -> Error {
    let mut manager = Manager::new(Policy::default()).unwrap();
    let cancelled = seed_cancellable_task(&mut manager).await;
    let (_, wake) = watch::channel(0_u64);
    let error = drive(
        &mut manager,
        source,
        idle,
        wake,
        CancellationToken::new(),
        DriveOptions { list_timeout },
        |round| std::future::ready(callback(round)),
    )
    .await
    .unwrap_err();
    cancelled.await.unwrap();
    assert!(manager.is_empty());
    error
}
