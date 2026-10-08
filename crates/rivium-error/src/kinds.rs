//! Kinds for failures that every service has. Services define their own kinds for the rest.

use crate::{Class, ErrorKind, ErrorType};

/// A defect or a broken invariant.
pub const INTERNAL: ErrorType = &ErrorKind::new("Internal", Class::Internal);
/// A parameter of the request breaks a rule.
pub const INVALID_INPUT: ErrorType = &ErrorKind::new("InvalidInput", Class::InvalidInput);
/// The body of the request cannot be parsed, or one of its fields breaks a rule.
pub const INVALID_BODY: ErrorType = &ErrorKind::new("InvalidBody", Class::InvalidBody);
/// The target does not exist.
pub const NOT_FOUND: ErrorType = &ErrorKind::new("NotFound", Class::NotFound);
/// The request conflicts with the current state.
pub const CONFLICT: ErrorType = &ErrorKind::new("Conflict", Class::Conflict);
/// A dependency is unavailable.
pub const UNAVAILABLE: ErrorType = &ErrorKind::new("Unavailable", Class::Unavailable);
/// Waiting took too long.
pub const TIMEOUT: ErrorType = &ErrorKind::new("Timeout", Class::Timeout);
