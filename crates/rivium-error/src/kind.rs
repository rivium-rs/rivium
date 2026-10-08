//! Error kinds and their classes.

use std::fmt;

/// The nature of a failure, independent of any transport. Adapters derive their own codes from
/// it, such as the HTTP status of a response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Class {
    /// A parameter of the request breaks a rule: a query or path value, an argument.
    InvalidInput,
    /// The body of the request cannot be parsed, or one of its fields breaks a rule.
    InvalidBody,
    /// The caller is not identified.
    Unauthenticated,
    /// The caller is identified but not allowed to do this.
    Forbidden,
    /// The target does not exist.
    NotFound,
    /// The request conflicts with the current state, such as a stale version.
    Conflict,
    /// The caller exceeded a quota.
    TooManyRequests,
    /// A dependency is unavailable or failed to answer properly.
    Unavailable,
    /// Waiting took too long.
    Timeout,
    /// A defect in this service or a broken invariant.
    Internal,
}

impl Class {
    /// The name, as logged in `error.class`, such as `NotFound`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Class::InvalidInput => "InvalidInput",
            Class::InvalidBody => "InvalidBody",
            Class::Unauthenticated => "Unauthenticated",
            Class::Forbidden => "Forbidden",
            Class::NotFound => "NotFound",
            Class::Conflict => "Conflict",
            Class::TooManyRequests => "TooManyRequests",
            Class::Unavailable => "Unavailable",
            Class::Timeout => "Timeout",
            Class::Internal => "Internal",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error kind: a `PascalCase` name, a class and an optional short title. Define each kind once,
/// as a constant of type [`ErrorType`](crate::ErrorType):
///
/// ```
/// use rivium_error::{Class, ErrorKind, ErrorType};
///
/// /// No device has the given serial number.
/// pub const DEVICE_NOT_FOUND: ErrorType = &ErrorKind::new("DeviceNotFound", Class::NotFound);
/// ```
///
/// The name is a stable identifier: it appears in logs as `error.type` and in the messages that
/// embedded hosts return, so renaming a kind is a breaking change.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ErrorKind {
    name: &'static str,
    class: Class,
    title: Option<&'static str>,
}

impl ErrorKind {
    /// A kind with this name and class, without a title.
    #[must_use]
    pub const fn new(name: &'static str, class: Class) -> Self {
        ErrorKind {
            name,
            class,
            title: None,
        }
    }

    /// The same kind with a short title for callers, such as `Device not found`.
    #[must_use]
    pub const fn titled(self, title: &'static str) -> Self {
        ErrorKind {
            title: Some(title),
            ..self
        }
    }

    /// The name, such as `DeviceNotFound`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The class.
    #[must_use]
    pub const fn class(&self) -> Class {
        self.class
    }

    /// The title, if the kind has one.
    #[must_use]
    pub const fn title(&self) -> Option<&'static str> {
        self.title
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}
