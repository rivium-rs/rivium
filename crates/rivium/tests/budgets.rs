#![cfg(feature = "rt")]
use rivium::{Kind, Outcome, StopReason, Supervisor, error::Class};
use std::time::Duration;

#[tokio::test]
async fn unrepresentable_budgets_fail_validation_without_panicking_in_control_or_run() {
    for (startup, grace) in [
        (Duration::MAX, Duration::from_secs(1)),
        (Duration::from_secs(1), Duration::MAX),
    ] {
        let supervisor =
            Supervisor::new(startup, grace).with(rivium_test::idle("idle", Kind::Frontline));
        let control = supervisor.control();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || control.restart("before run")
            ))
            .is_ok()
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || control.stop(StopReason::Requested)
            ))
            .is_ok()
        );
        let report = tokio::spawn(supervisor.run())
            .await
            .expect("invalid budgets must not panic");
        match report.outcome {
            Outcome::StartupFailed(error) => assert_eq!(error.class(), Class::InvalidInput),
            outcome => panic!("unexpected outcome {outcome:?}"),
        }
        assert!(report.drain.is_complete());
    }
}

#[tokio::test]
async fn unrepresentable_periodic_interval_is_a_validation_failure() {
    let supervisor = Supervisor::default().with(rivium::periodic(
        "invalid-ticker",
        Duration::MAX,
        || async { Ok(()) },
    ));
    match supervisor.run().await.outcome {
        Outcome::StartupFailed(error) => assert_eq!(error.class(), Class::InvalidInput),
        outcome => panic!("unexpected {outcome:?}"),
    }
}
