#![cfg(feature = "rt")]
use rivium::{Kind, Supervisor, service};
#[tokio::test]
async fn phase_and_finish_events_are_emitted_once_and_children_inherit_dispatch() {
    use tracing::instrument::WithSubscriber;
    let logs = rivium_test::Logs::default();
    let sup = Supervisor::default().with(service("owner", Kind::Background, |cx| async move {
        cx.ready();
        cx.spawn("event", async {
            tracing::info!("child output");
            Ok(())
        })
        .unwrap();
        Ok(())
    }));
    sup.run().with_subscriber(logs.dispatch()).await;
    let text = logs.text();
    assert_eq!(text.matches("phase changed").count(), 4, "{text}");
    assert_eq!(text.matches("run finished").count(), 1, "{text}");
    assert!(text.contains("child output"), "{text}");
    assert!(text.contains("task_key=event"), "{text}");
}
