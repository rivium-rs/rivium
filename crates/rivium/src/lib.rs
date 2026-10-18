//! Small, explicit building blocks for long-running Tokio applications.

/// Owned, sendable asynchronous result used by service adapters.
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

pub mod error;
pub use error::{Error, Result, ResultExt};

#[cfg(feature = "rt")]
pub mod rt;
#[cfg(feature = "task")]
pub mod scope;
#[cfg(feature = "rt")]
pub use rt::{
    Context, Control, Health, HealthHandle, HealthRegistry, Kind, Outcome, Phase, Readiness,
    RunReport, Service, Stop, StopReason, SubmitError, Supervisor, periodic, service,
};
