#![cfg(feature = "task")]

use rivium_util::task::{Context, DesiredError, Lifecycle, Manager, Policy, ReconcileError};

#[tokio::test]
async fn oversized_desired_set_is_rejected_before_touching_live_work() {
    let mut manager = Manager::new(Policy {
        capacity: 1,
        ..Policy::default()
    })
    .unwrap();
    manager
        .reconcile(vec![("old", 1)], |_, _, cx: Context| async move {
            cx.cancel.cancelled().await;
            Ok(())
        })
        .await
        .unwrap();
    let result = manager
        .reconcile(vec![("new", 1), ("other", 1)], |_, _, _| async {
            panic!("rejected factory")
        })
        .await;
    assert!(matches!(
        result,
        Err(ReconcileError::InvalidDesired(DesiredError::TooMany {
            requested: 2,
            capacity: 1
        }))
    ));
    assert_eq!(manager.len(), 1);
    manager.shutdown().await.unwrap();
}

#[test]
fn pure_plan_keeps_unconfirmed_removed_keys_in_the_capacity_budget() {
    use rivium_util::task::{View, plan};
    use std::collections::BTreeMap;
    let now = tokio::time::Instant::now();
    let current = BTreeMap::from([(
        "old",
        View {
            spec: 1,
            alive: true,
            stopping: true,
            failures: 0,
            retry_at: now,
        },
    )]);
    let desired = BTreeMap::from([("new", 1)]);
    let p = plan(
        &desired,
        &current,
        now,
        &Policy {
            capacity: 1,
            ..Policy::default()
        },
    );
    assert_eq!(p.stop, ["old"]);
    assert!(p.start.is_empty());
    assert_eq!(p.limited, ["new"]);
}

#[tokio::test]
async fn unchanged_work_preserves_tasks_and_checks_scope_admission() {
    for size in [0, 2] {
        let scope = rivium::scope::TaskScope::new();
        let mut manager = Manager::with_scope(Policy::default(), scope.clone()).unwrap();
        let mut desired: Vec<_> = (0..size).map(|key| (key, 1)).collect();
        manager
            .reconcile(desired.clone(), |_, _, context| async move {
                context.cancel.cancelled().await;
                Ok(())
            })
            .await
            .unwrap();
        // Input order must not turn an unchanged set into replacement work.
        desired.reverse();

        for closed in [false, true] {
            if closed {
                scope.close();
            }
            let mut factory_calls = 0;
            let report = manager
                .reconcile(desired.clone(), |_, _, _| {
                    factory_calls += 1;
                    std::future::ready(Ok(()))
                })
                .await
                .unwrap();
            assert_eq!(
                report.lifecycle,
                if closed {
                    Lifecycle::Closed
                } else {
                    Lifecycle::Open
                }
            );
            assert!(report.started.is_empty());
            assert!(report.restarted.is_empty());
            assert!(report.removed.is_empty());
            assert!(report.exhausted.is_empty());
            assert!(report.limited.is_empty());
            assert!(report.unconfirmed.is_empty());
            assert!(report.exits.is_empty());
            assert_eq!(factory_calls, 0);
            assert_eq!(manager.len(), size);
            assert_eq!(scope.active_tasks(), size);
        }
        manager.shutdown().await.unwrap();
        assert_eq!(scope.active_tasks(), 0);
    }
}
