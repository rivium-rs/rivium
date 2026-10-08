//! The Rivium service foundation.
//!
//! This crate will provide configuration loading, logging with a disk budget, the service
//! lifecycle kernel, the process host and the embedded host. It is pre-release: so far it
//! provides the [`config`] types and [`fs::atomic_write`].

pub use rivium_error::{self as error, BError, Error, Result};

pub mod config;
pub mod fs;
mod lifecycle;
mod log;
