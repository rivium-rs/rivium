#![cfg(feature = "rt")]
use rivium::{Error, Kind, Outcome, SubmitError, Supervisor, service};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continuous_admission_cannot_starve_a_completed_child_failure() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    // A legal long root label makes admission outrun completion bookkeeping.
    let supervisor = Supervisor::default().with(service(
        "producer".repeat(1024),
        Kind::Frontline,
        move |cx| async move {
            cx.ready();
            cx.spawn("failed", async {
                Err(Error::internal("first child failed"))
            })
            .unwrap();
            // Keep the bounded command channel occupied as finished tasks return quota.
            for serial in 0..20_000 {
                if cx.is_stopping() {
                    break;
                }
                match cx.spawn(serial.to_string(), async { Ok(()) }) {
                    Ok(()) => {
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(SubmitError::Overloaded) => tokio::task::yield_now().await,
                    Err(SubmitError::Closed) => break,
                    Err(error) => panic!("unexpected admission {error}"),
                }
            }
            cx.stopped().await;
            Ok(())
        },
    ));
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::task::unconstrained(supervisor.run()),
    )
    .await
    .unwrap();
    assert!(matches!(report.outcome, Outcome::Failed(_)));
    assert!(
        accepted.load(Ordering::Relaxed) < 8192,
        "completed failure was starved by {} new commands",
        accepted.load(Ordering::Relaxed)
    );
    assert!(report.drain.is_complete());
}
