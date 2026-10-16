#![cfg(feature = "task")]

use rivium_util::task::{Context, DesiredError, Manager, Policy, ReconcileError};

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
