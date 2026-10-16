#![cfg(feature = "rt")]
use rivium::{Error, Outcome, Supervisor, periodic, rt::StopReason};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn tick_is_immediate_and_missed_intervals_are_skipped_without_overlapping() {
    let timestamps = Arc::new(Mutex::new(Vec::new()));
    let observed = timestamps.clone();
    let start = Instant::now();
    let mut count = 0;
    let sup = Supervisor::default().with(periodic("ticker", Duration::from_secs(1), move || {
        count += 1;
        let index = count;
        observed.lock().unwrap().push(start.elapsed());
        async move {
            if index == 2 {
                tokio::time::sleep(Duration::from_millis(2500)).await;
            }
            if index == 4 {
                Err(Error::internal("tick ended"))
            } else {
                Ok(())
            }
        }
    }));
    assert!(matches!(sup.run().await.outcome, Outcome::Failed(_)));
    assert_eq!(
        *timestamps.lock().unwrap(),
        [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_millis(3500),
            Duration::from_secs(4)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn stop_during_tick_waits_for_cooperation_until_the_shared_grace_expires() {
    let (started, receive) = tokio::sync::oneshot::channel();
    let mut started = Some(started);
    let sup = Supervisor::new(Duration::from_secs(1), Duration::from_secs(2)).with(periodic(
        "ticker",
        Duration::from_secs(1),
        move || {
            started.take().unwrap().send(()).ok();
            std::future::pending()
        },
    ));
    let control = sup.control();
    tokio::spawn(async move {
        receive.await.unwrap();
        control.stop(StopReason::Requested);
    });
    let start = Instant::now();
    assert!(matches!(sup.run().await.outcome, Outcome::TimedOut { .. }));
    assert_eq!(start.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn stop_between_ticks_does_not_start_another_tick() {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let sup = Supervisor::default().with(periodic("ticker", Duration::from_secs(60), move || {
        observed.fetch_add(1, Ordering::Relaxed);
        async { Ok(()) }
    }));
    let control = sup.control();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        control.stop(StopReason::Requested);
    });
    assert!(matches!(sup.run().await.outcome, Outcome::Stopped));
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn zero_interval_fails_without_panicking() {
    let sup = Supervisor::default().with(periodic("ticker", Duration::ZERO, || async { Ok(()) }));
    assert!(matches!(sup.run().await.outcome, Outcome::StartupFailed(_)));
}
