//! Models only the upstream cancellation flag, not Tokio's wakeup machinery.

use loom::sync::atomic::AtomicBool;
pub use loom::sync::atomic::{AtomicUsize, Ordering};
pub use loom::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub async fn cancelled(&self) {
        std::future::pending::<()>().await;
    }
}
