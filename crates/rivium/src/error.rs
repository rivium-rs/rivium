//! Transport-independent failures with opt-in public disclosure.

use std::{fmt, sync::Arc, time::Duration};

/// Broad failure category; transports choose their own mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Class {
    /// Invalid caller-supplied data.
    InvalidInput,
    /// Request body is malformed.
    InvalidBody,
    /// Request payload exceeds its limit.
    PayloadTooLarge,
    /// Credentials are absent or invalid.
    Unauthenticated,
    /// The operation is forbidden.
    Forbidden,
    /// The requested item does not exist.
    NotFound,
    /// The operation conflicts with current state.
    Conflict,
    /// A configured resource budget was exhausted.
    TooManyRequests,
    /// The operation exceeded its deadline.
    Timeout,
    /// A dependency is temporarily unavailable.
    Unavailable,
    /// An unexpected implementation failure.
    Internal,
}

/// The party responsible for correcting a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// The direct caller.
    Caller,
    /// This service.
    Local,
    /// A downstream dependency.
    Upstream,
}

/// An operation provider's hint, never a proof of idempotence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retry {
    /// Do not automatically retry.
    Never,
    /// Retrying may be safe if the caller's own policy permits it.
    Safe,
    /// Retry no earlier than this delay.
    After(Duration),
}

/// A stable error name and transport-independent classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorKind {
    /// Stable machine-readable name.
    pub name: &'static str,
    /// Broad classification.
    pub class: Class,
    /// Safe transport-facing title.
    pub title: Option<&'static str>,
}

impl ErrorKind {
    /// Define an application-specific stable kind.
    pub const fn new(name: &'static str, class: Class) -> Self {
        Self {
            name,
            class,
            title: None,
        }
    }

    /// Attach a safe default title.
    pub const fn titled(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    /// Stable machine-readable name.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Broad classification.
    pub const fn class(self) -> Class {
        self.class
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// Static error classification shared across error instances.
pub type ErrorType = &'static ErrorKind;

/// Built-in stable kinds.
pub mod kinds {
    use super::{Class, ErrorKind, ErrorType};

    /// Invalid caller input.
    pub const INVALID_INPUT: ErrorType = &ErrorKind::new("InvalidInput", Class::InvalidInput);

    /// Malformed request body.
    pub const INVALID_BODY: ErrorType = &ErrorKind::new("InvalidBody", Class::InvalidBody);

    /// Request payload exceeds its limit.
    pub const PAYLOAD_TOO_LARGE: ErrorType =
        &ErrorKind::new("PayloadTooLarge", Class::PayloadTooLarge);

    /// Missing or invalid credentials.
    pub const UNAUTHENTICATED: ErrorType =
        &ErrorKind::new("Unauthenticated", Class::Unauthenticated);

    /// Insufficient permission.
    pub const FORBIDDEN: ErrorType = &ErrorKind::new("Forbidden", Class::Forbidden);

    /// Requested item absent.
    pub const NOT_FOUND: ErrorType = &ErrorKind::new("NotFound", Class::NotFound);

    /// Current state conflicts with an operation.
    pub const CONFLICT: ErrorType = &ErrorKind::new("Conflict", Class::Conflict);

    /// Resource limit reached.
    pub const TOO_MANY_REQUESTS: ErrorType =
        &ErrorKind::new("TooManyRequests", Class::TooManyRequests);

    /// Deadline exceeded.
    pub const TIMEOUT: ErrorType = &ErrorKind::new("Timeout", Class::Timeout);

    /// Dependency unavailable.
    pub const UNAVAILABLE: ErrorType = &ErrorKind::new("Unavailable", Class::Unavailable);

    /// Unexpected implementation failure.
    pub const INTERNAL: ErrorType = &ErrorKind::new("Internal", Class::Internal);
}

/// A cheaply cloned failure. Diagnostic text is never public by default.
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorType,
    side: Side,
    retry: Retry,
    message: Arc<str>,
    public: Option<Arc<str>>,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Construct a failure with private diagnostic text.
    pub fn new(kind: ErrorType, message: impl Into<String>) -> Self {
        let side = match kind.class() {
            Class::InvalidInput
            | Class::InvalidBody
            | Class::PayloadTooLarge
            | Class::TooManyRequests
            | Class::Unauthenticated
            | Class::Forbidden
            | Class::NotFound
            | Class::Conflict => Side::Caller,
            _ => Side::Local,
        };
        Self {
            kind,
            side,
            retry: Retry::Never,
            message: message.into().into(),
            public: None,
            source: None,
        }
    }

    /// Unexpected implementation failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(kinds::INTERNAL, message)
    }

    /// Invalid input failure.
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(kinds::INVALID_INPUT, message)
    }

    /// Invalid input failure.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::invalid_input(message)
    }

    /// Mark a dependency as the source of a failure.
    pub fn upstream(mut self) -> Self {
        self.side = Side::Upstream;
        self
    }

    /// Stable kind.
    pub fn kind(&self) -> ErrorType {
        self.kind
    }

    /// Broad category.
    pub fn class(&self) -> Class {
        self.kind.class()
    }

    /// Responsible party.
    pub fn side(&self) -> Side {
        self.side
    }

    /// Provider retry hint.
    pub fn retry(&self) -> Retry {
        self.retry
    }

    /// Private diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Explicitly approved public text, if any.
    pub fn public_message(&self) -> Option<&str> {
        self.public.as_deref()
    }

    /// Set the responsible party.
    pub fn with_side(mut self, side: Side) -> Self {
        self.side = side;
        self
    }

    /// Set the operation provider's retry hint.
    pub fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    /// Opt in to disclosure, truncated to at most 1024 UTF-8 bytes.
    pub fn with_public_message(mut self, message: impl Into<String>) -> Self {
        let mut text = message.into();
        if text.len() > 1024 {
            let mut end = 1024;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        self.public = Some(text.into());
        self
    }

    /// Traverse this diagnostic failure followed by its original causes.
    pub fn chain(&self) -> impl Iterator<Item = &(dyn std::error::Error + 'static)> {
        std::iter::successors(Some(self as &(dyn std::error::Error + 'static)), |error| {
            error.source()
        })
    }

    /// Attach the original diagnostic cause.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// Add private context while preserving all classification and public text.
    pub fn context(self, message: impl Into<String>) -> Self {
        Self {
            message: message.into().into(),
            source: Some(Arc::new(self.clone())),
            ..self
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if f.alternate() {
            for source in self.chain().skip(1) {
                write!(f, ": {source}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(kinds::INTERNAL, error.to_string())
            .with_side(Side::Local)
            .with_source(error)
    }
}

/// Default result type.
pub type Result<T = ()> = std::result::Result<T, Error>;

/// Attach diagnostic context without losing classification.
pub trait ResultExt<T> {
    /// Add diagnostic context on failure.
    fn context(self, message: impl Into<String>) -> Result<T>;

    /// Lazily produce diagnostic context on failure.
    fn with_context(self, message: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> ResultExt<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.map_err(
            |cause| match (&cause as &dyn std::error::Error).downcast_ref::<Error>() {
                Some(typed) => typed.clone().context(message),
                None => Error::internal(message).with_source(cause),
            },
        )
    }

    fn with_context(self, message: impl FnOnce() -> String) -> Result<T> {
        match self {
            Ok(value) => Ok(value),
            Err(cause) => Err::<T, E>(cause).context(message()),
        }
    }
}
