#![cfg(feature = "task")]

use rivium_util::task::{Manager, Policy, View, plan};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn spec_change_resets_give_up_and_capacity_is_stable() {
    let now = tokio::time::Instant::now();
    let desired = BTreeMap::from([("a", 2), ("b", 1), ("c", 1)]);
    let current = BTreeMap::from([(
        "a",
        View {
            spec: 1,
            alive: false,
            stopping: false,
            failures: 99,
            retry_at: now + Duration::from_secs(10),
        },
    )]);
    let policy = Policy {
        capacity: 2,
        ..Default::default()
    };
    let p = plan(&desired, &current, now, &policy);
    assert_eq!(p.stop, ["a"]);
    assert_eq!(p.start, ["a", "b"]);
    assert_eq!(p.limited, ["c"]);
}

#[tokio::test]
async fn replace_never_overlaps_and_drop_aborts() {
    struct Count(Arc<AtomicUsize>);

    impl Drop for Count {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut manager = Manager::new(Policy {
        stop_timeout: Duration::from_millis(10),
        ..Default::default()
    })
    .unwrap();
    for spec in [1, 2, 3] {
        let live = live.clone();
        let peak = peak.clone();
        manager
            .reconcile(vec![("x", spec)], move |_, _, _| {
                let live = live.clone();
                let peak = peak.clone();
                async move {
                    let count = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    let _guard = Count(live);
                    std::future::pending().await
                }
            })
            .await
            .unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(peak.load(Ordering::SeqCst), 1);
    manager.shutdown().await.unwrap();
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert!(
        manager
            .reconcile(vec![("x", 4)], |_, _, _| async { Ok(()) })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn duplicate_keys_leave_running_tasks_unchanged() {
    let mut manager = Manager::new(Policy::default()).unwrap();
    assert!(
        manager
            .reconcile(vec![("x", 1), ("x", 2)], |_, _, _| async { Ok(()) })
            .await
            .is_err()
    );
    assert!(manager.is_empty());
}
