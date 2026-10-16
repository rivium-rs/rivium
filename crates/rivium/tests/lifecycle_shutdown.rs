#![cfg(feature = "rt")]

use rivium::{Kind, Outcome, Supervisor, scope::Drain, service};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

async fn admitted_children_survive_close() {
    let completed = Arc::new(AtomicUsize::new(0));
    let children = completed.clone();
    let supervisor = Supervisor::new(Duration::from_secs(1), Duration::from_secs(1)).with(service(
        "parent",
        Kind::Background,
        move |ctx| async move {
            ctx.ready();
            let scope = ctx.task_scope();
            let asynchronous = children.clone();
            ctx.spawn("async", async move {
                asynchronous.fetch_add(1, Ordering::Release);
                Ok(())
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            ctx.spawn_blocking("blocking", move || {
                children.fetch_add(1, Ordering::Release);
                Ok(())
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            // Admission closure must not revoke successful queued submissions.
            scope.close();
            assert!(!ctx.is_stopping());
            assert!(ctx.spawn("late", async { Ok(()) }).is_err());
            assert_eq!(scope.drain(Duration::from_secs(1)).await, Drain::Complete);
            Ok(())
        },
    ));
    let readiness = supervisor.readiness();
    assert!(matches!(supervisor.run().await.outcome, Outcome::Stopped));
    assert_eq!(completed.load(Ordering::Acquire), 2);
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn queued_children_survive_close_on_current_thread() {
    admitted_children_survive_close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_children_survive_close_on_two_workers() {
    admitted_children_survive_close().await;
}

#[tokio::test]
async fn cancelling_scope_still_discards_queued_children() {
    let completed = Arc::new(AtomicUsize::new(0));
    let child = completed.clone();
    let supervisor =
        Supervisor::default().with(service("parent", Kind::Background, move |ctx| async move {
            ctx.ready();
            ctx.spawn("queued", async move {
                child.fetch_add(1, Ordering::Release);
                Ok(())
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            let scope = ctx.task_scope();
            scope.cancel();
            assert_eq!(scope.drain(Duration::from_secs(1)).await, Drain::Complete);
            Ok(())
        }));
    let readiness = supervisor.readiness();
    assert!(matches!(supervisor.run().await.outcome, Outcome::Stopped));
    assert_eq!(completed.load(Ordering::Acquire), 0);
    assert_eq!(readiness.active_tasks(), 0);
}

#[tokio::test]
async fn duplicate_queued_children_remain_errors_after_close() {
    let supervisor =
        Supervisor::default().with(service("parent", Kind::Background, move |ctx| async move {
            ctx.ready();
            let stop = ctx.stop();
            ctx.spawn("same", async move {
                stop.stopped().await;
                Ok(())
            })
            .map_err(|error| rivium::Error::internal(error.to_string()))?;
            ctx.spawn("same", async { Ok(()) })
                .map_err(|error| rivium::Error::internal(error.to_string()))?;
            let scope = ctx.task_scope();
            scope.close();
            assert_eq!(scope.drain(Duration::from_secs(1)).await, Drain::Complete);
            Ok(())
        }));
    let readiness = supervisor.readiness();
    let outcome = supervisor.run().await;
    assert!(matches!(outcome.outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(readiness.active_tasks(), 0);
}
