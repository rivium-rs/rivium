//! tokio: a multi-threaded runtime spawns tasks, runs blocking work and timers, and shuts down
//! within a bounded time.

use std::time::Duration;

#[test]
fn multi_thread_runtime_runs_and_shuts_down() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("smoke-worker")
        .enable_all()
        .build()
        .unwrap();
    let sum = runtime.block_on(async {
        let tasks: Vec<_> = (1..=8u64)
            .map(|i| {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    i
                })
            })
            .collect();
        let blocking =
            tokio::task::spawn_blocking(|| std::thread::current().name().map(str::to_owned));
        let mut sum = 0;
        for task in tasks {
            sum += task.await.unwrap();
        }
        assert_eq!(blocking.await.unwrap().as_deref(), Some("smoke-worker"));
        let pending = tokio::time::timeout(Duration::from_millis(10), std::future::pending::<()>());
        assert!(pending.await.is_err());
        sum
    });
    assert_eq!(sum, 36);
    runtime.shutdown_timeout(Duration::from_millis(400));
}
