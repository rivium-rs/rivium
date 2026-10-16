// Each test binary uses the startup helpers relevant to its own assertions.
#![allow(dead_code)]
use rivium::{Context, Control, Phase, Readiness, RunReport, Supervisor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::task::JoinHandle;
pub(crate) struct Dropped(pub Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
pub(crate) type Run = (Control, Readiness, JoinHandle<RunReport>);
pub(crate) fn spawn(supervisor: Supervisor) -> Run {
    let control = supervisor.control();
    let readiness = supervisor.readiness();
    (control, readiness, tokio::spawn(supervisor.run()))
}
pub(crate) async fn ready(supervisor: Supervisor) -> Run {
    let (control, mut readiness, run) = spawn(supervisor);
    rivium_test::until_ready(&mut readiness).await.unwrap();
    (control, readiness, run)
}
pub(crate) async fn declare_ready(context: &Context) {
    context.ready();
    while context.readiness().phase() != Phase::Running {
        tokio::task::yield_now().await;
    }
}
