//! The error type shared by Rivium services.
//!
//! One [`Error`] type carries a kind ([`ErrorKind`], with a transport-independent [`Class`]), the
//! party responsible ([`ErrorSource`]), a retry hint, a context and a cause chain. Services define
//! their kinds as constants and log each error once, where it is handled, with
//! [`log_error!`]:
//!
//! ```
//! use rivium_error::prelude::*;
//!
//! const DEVICE_SILENT: ErrorType = &ErrorKind::new("DeviceSilent", Class::Unavailable);
//!
//! fn read(port: &str) -> Result<Vec<u8>> {
//!     std::fs::read(port).or_err_with(DEVICE_SILENT, || format!("reading {port}"))
//! }
//!
//! let error = read("/no/such/port").unwrap_err();
//! assert_eq!(error.class(), Class::Unavailable);
//! assert!(format!("{error:#}").starts_with("reading /no/such/port: "));
//! log_error!(tracing::Level::WARN, &error, "poll failed");
//! ```
//!
//! The class travels with the error, so adapters map it to their own codes with a plain `match`;
//! nothing registers kinds anywhere.
#![forbid(unsafe_code)]

mod error;
mod ext;
mod fields;
mod kind;
pub mod kinds;
mod log;

pub use error::{Chain, Error, ErrorSource};
pub use ext::{Context, OkOrErr, OrErr};
pub use fields::Fields;
pub use kind::{Class, ErrorKind};

/// The boxed [`Error`], as errors travel.
pub type BError = Box<Error>;

/// A result whose error is a [`BError`] by default.
pub type Result<T, E = BError> = std::result::Result<T, E>;

/// An error kind: a reference to a constant [`ErrorKind`].
pub type ErrorType = &'static ErrorKind;

/// Not public API: the `tracing` that the macros expand to.
#[doc(hidden)]
pub mod __private {
    pub use tracing;
}

/// The items services import with `use rivium_error::prelude::*;`.
pub mod prelude {
    pub use crate::{
        BError, Class, Context, Error, ErrorKind, ErrorSource, ErrorType, OkOrErr, OrErr, Result,
        log_at_level, log_error,
    };
}
