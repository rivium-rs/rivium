//! The Rivium service foundation.
//!
//! This crate will provide configuration loading, logging with a disk budget, the service
//! lifecycle kernel, the process host and the embedded host. It is pre-release: so far it
//! provides [`config`], [`log`], [`fs::atomic_write`] and [`stats()`]; the hosts that load the
//! configuration and install logging come next.

pub use rivium_error::{self as error, BError, Error, Result};

pub mod config;
pub mod fs;
mod lifecycle;
pub mod log;
mod stats;

pub use stats::{Stats, stats};
