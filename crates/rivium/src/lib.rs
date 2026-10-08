//! The Rivium service foundation.
//!
//! This crate will provide configuration loading, logging with a disk budget, the service
//! lifecycle kernel, the process host and the embedded host. It is pre-release: so far it
//! provides [`config`], [`log`], the service contract ([`Service`], [`ServiceContext`],
//! [`service()`], [`periodic()`]) and the [`lifecycle`] kernel that runs services,
//! [`fs::atomic_write`] and [`stats()`]; the hosts come next.

use std::future::Future;
use std::pin::Pin;

pub use rivium_error::{self as error, BError, Error, Result};

pub mod config;
pub mod fs;
pub mod lifecycle;
pub mod log;
mod stats;

pub use lifecycle::health::{Health, HealthHandle, HealthRegistry, Phase, Readiness};
pub use lifecycle::service::{
    Restarter, Service, ServiceContext, ServiceKind, StopSignal, periodic, service,
};
pub use stats::{Stats, stats};

/// A boxed future that can move between threads, as [`Service::run`] returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
