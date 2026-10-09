//! The Rivium service foundation.
//!
//! A service program implements [`App`]: its configuration and its [`Service`]s, built with
//! [`service()`], [`periodic()`] or by hand. A host runs it, with configuration ([`config`]),
//! logging with a disk budget ([`log`]), stop requests, restarts and result codes ([`Code`])
//! handled: [`process::run`] as a program of its own, or [`embedded::Host`] inside another
//! program, such as an Android app through rivium-jni. The [`lifecycle`] kernel supervises the
//! services. Before 1.0, a new minor version may break the API.

use std::future::Future;
use std::pin::Pin;

pub use rivium_error::{self as error, BError, Error, Result};

mod app;
mod code;
pub mod config;
pub mod embedded;
pub mod fs;
mod host;
pub mod lifecycle;
pub mod log;
pub mod process;
mod stats;

pub use app::{App, AppContext, Identity};
pub use code::Code;
pub use lifecycle::health::{Health, HealthHandle, HealthRegistry, Phase, Readiness};
pub use lifecycle::service::{
    Restarter, Service, ServiceContext, ServiceKind, StopSignal, periodic, service,
};
pub use stats::{Stats, stats};

/// A boxed future that can move between threads, as [`Service::run`] returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
