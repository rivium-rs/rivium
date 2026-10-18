#![cfg(feature = "rt")]

use rivium::{Kind, Supervisor, scope::Drain};
use std::time::Duration;
use tracing::{
    Event, Metadata, Subscriber,
    instrument::WithSubscriber,
    span::{Attributes, Id, Record},
};

struct PanicOnSpan;

impl Subscriber for PanicOnSpan {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        panic!("injected subscriber new_span failure")
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn panic_after_scope_registration_closes_the_registered_scope() {
    let supervisor = Supervisor::default().with(rivium_test::idle("idle", Kind::Frontline));
    let readiness = supervisor.readiness();
    let failure = tokio::spawn(supervisor.run().with_subscriber(PanicOnSpan))
        .await
        .unwrap_err();
    assert!(failure.is_panic());
    assert_eq!(readiness.drain(Duration::ZERO).await, Drain::Complete);
}
