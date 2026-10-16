#![cfg(feature = "rt")]
use rivium::{BoxFuture, Context, Kind, Outcome, Result, Service, Supervisor, scope::Drain};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Metadata {
    panic_name: bool,
    panic_kind: bool,
    names: Arc<AtomicUsize>,
    kinds: Arc<AtomicUsize>,
}
impl Service for Metadata {
    fn name(&self) -> &str {
        self.names.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_name, "name panic");
        "metadata"
    }
    fn kind(&self) -> Kind {
        self.kinds.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_kind, "kind panic");
        Kind::Background
    }
    fn run(self: Box<Self>, cx: Context) -> BoxFuture<Result<()>> {
        Box::pin(async move {
            cx.ready();
            Ok(())
        })
    }
}
#[tokio::test]
async fn metadata_panics_produce_a_complete_startup_failure_report() {
    for (panic_name, panic_kind) in [(true, false), (false, true)] {
        let supervisor = Supervisor::default().with(Box::new(Metadata {
            panic_name,
            panic_kind,
            names: Arc::new(AtomicUsize::new(0)),
            kinds: Arc::new(AtomicUsize::new(0)),
        }));
        let report = tokio::spawn(supervisor.run())
            .await
            .expect("metadata panic must become a report");
        assert!(matches!(report.outcome, Outcome::StartupFailed(_)));
        assert_eq!(report.drain, Drain::Complete);
    }
}
#[tokio::test]
async fn validated_metadata_is_read_once_and_reused_for_launch() {
    let names = Arc::new(AtomicUsize::new(0));
    let kinds = Arc::new(AtomicUsize::new(0));
    let supervisor = Supervisor::default().with(Box::new(Metadata {
        panic_name: false,
        panic_kind: false,
        names: names.clone(),
        kinds: kinds.clone(),
    }));
    assert!(matches!(supervisor.run().await.outcome, Outcome::Stopped));
    assert_eq!(names.load(Ordering::SeqCst), 1);
    assert_eq!(kinds.load(Ordering::SeqCst), 1);
}
