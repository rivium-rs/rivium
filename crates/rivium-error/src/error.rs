//! The error type.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use crate::{BError, Class, ErrorType, Result};

/// A cause: any error, including another [`Error`].
type Cause = Box<dyn StdError + Send + Sync>;

/// The error of a Rivium service: a kind, the party responsible for it, a retry hint, an
/// optional context and an optional cause. Errors travel boxed, as [`BError`]:
///
/// ```
/// use rivium_error::{Error, kinds};
///
/// let io = std::io::Error::other("disk full");
/// let error = Error::because(kinds::UNAVAILABLE, "writing the snapshot", io);
/// assert_eq!(error.to_string(), "writing the snapshot");
/// assert_eq!(format!("{error:#}"), "writing the snapshot: disk full");
/// ```
///
/// `{}` shows this error's own context, or the kind's name when it has none; `{:#}` shows the
/// whole chain, outermost first, joined by `: `.
#[derive(Debug)]
pub struct Error {
    kind: ErrorType,
    esource: ErrorSource,
    retry: bool,
    context: Option<Cow<'static, str>>,
    cause: Option<Cause>,
}

/// The party responsible for an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorSource {
    /// A remote service this one called.
    Upstream,
    /// The caller of this service.
    Downstream,
    /// This service.
    Internal,
    /// Not decided. New errors start here: set it last, with [`Error::into_up`] and its siblings.
    Unset,
}

impl ErrorSource {
    /// The name, as logged in `error.source`: `upstream`, `downstream`, `internal` or `unset`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorSource::Upstream => "upstream",
            ErrorSource::Downstream => "downstream",
            ErrorSource::Internal => "internal",
            ErrorSource::Unset => "unset",
        }
    }
}

impl Error {
    fn create(kind: ErrorType, context: Option<Cow<'static, str>>, cause: Option<Cause>) -> BError {
        let cause = cause.map(unbox);
        // A cause of this type passes its retry hint on.
        let retry = (cause.as_deref())
            .and_then(|cause| cause.downcast_ref::<Error>())
            .is_some_and(|cause| cause.retry);
        let context = context.filter(|context| !context.is_empty());
        Box::new(Error {
            kind,
            esource: ErrorSource::Unset,
            retry,
            context,
            cause,
        })
    }

    /// An error of this kind, without context or cause.
    #[must_use]
    pub fn new(kind: ErrorType) -> BError {
        Error::create(kind, None, None)
    }

    /// An error of this kind that explains what went wrong.
    #[must_use]
    pub fn explain(kind: ErrorType, context: impl Into<Cow<'static, str>>) -> BError {
        Error::create(kind, Some(context.into()), None)
    }

    /// An error of this kind, caused by another error: context says what was being done.
    #[must_use]
    pub fn because(
        kind: ErrorType,
        context: impl Into<Cow<'static, str>>,
        cause: impl Into<Box<dyn StdError + Send + Sync>>,
    ) -> BError {
        Error::create(kind, Some(context.into()), Some(cause.into()))
    }

    /// `Err(Error::explain(..))`.
    ///
    /// # Errors
    ///
    /// Always.
    pub fn e_explain<T>(kind: ErrorType, context: impl Into<Cow<'static, str>>) -> Result<T> {
        Err(Error::explain(kind, context))
    }

    /// `Err(Error::because(..))`.
    ///
    /// # Errors
    ///
    /// Always.
    pub fn e_because<T>(
        kind: ErrorType,
        context: impl Into<Cow<'static, str>>,
        cause: impl Into<Box<dyn StdError + Send + Sync>>,
    ) -> Result<T> {
        Err(Error::because(kind, context, cause))
    }

    /// The kind.
    #[must_use]
    pub fn etype(&self) -> ErrorType {
        self.kind
    }

    /// The class of the kind.
    #[must_use]
    pub fn class(&self) -> Class {
        self.kind.class()
    }

    /// The party responsible.
    #[must_use]
    pub fn esource(&self) -> &ErrorSource {
        &self.esource
    }

    /// Whether retrying may help: a hint for callers (such as `Retry-After`), not a retry policy.
    #[must_use]
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Sets the retry hint.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }

    /// This error's own context, if it has one.
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// The same error, with [`ErrorSource::Upstream`] responsible.
    #[must_use]
    pub fn into_up(mut self: BError) -> BError {
        self.esource = ErrorSource::Upstream;
        self
    }

    /// The same error, with [`ErrorSource::Downstream`] responsible.
    #[must_use]
    pub fn into_down(mut self: BError) -> BError {
        self.esource = ErrorSource::Downstream;
        self
    }

    /// The same error, with [`ErrorSource::Internal`] responsible.
    #[must_use]
    pub fn into_in(mut self: BError) -> BError {
        self.esource = ErrorSource::Internal;
        self
    }

    /// A new error of the same kind, responsible party and retry hint, with more context and
    /// this error as its cause.
    #[must_use]
    pub fn more_context(self: BError, context: impl Into<Cow<'static, str>>) -> BError {
        let (kind, esource) = (self.kind, self.esource);
        // The retry hint passes on from the cause, as for every cause of this type.
        let mut error = Error::create(kind, Some(context.into()), Some(self));
        error.esource = esource;
        error
    }

    /// This error, then each cause in turn.
    #[must_use]
    pub fn chain(&self) -> Chain<'_> {
        Chain { next: Some(self) }
    }

    /// The innermost cause, or this error when it has none.
    #[must_use]
    pub fn root_cause(&self) -> &(dyn StdError + 'static) {
        self.chain().last().unwrap_or(self)
    }

    /// The text `{}` shows: the context, or the kind's name.
    fn own_text(&self) -> &str {
        self.context.as_deref().unwrap_or(self.kind.name())
    }
}

/// A cause of this type is boxed twice when it arrives as `Box<dyn Error>`; keep one box, so it
/// downcasts to [`Error`].
fn unbox(cause: Cause) -> Cause {
    match cause.downcast::<BError>() {
        Ok(error) => *error,
        Err(cause) => cause,
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !f.alternate() {
            return f.write_str(self.own_text());
        }
        for (index, hop) in self.chain().enumerate() {
            if index > 0 {
                f.write_str(": ")?;
            }
            match hop.downcast_ref::<Error>() {
                Some(error) => f.write_str(error.own_text())?,
                None => write!(f, "{hop}")?,
            }
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn StdError + 'static))
    }
}

/// An error and its causes, outermost first; see [`Error::chain`]. Errors of this type downcast
/// to [`Error`]; others, such as `std::io::Error`, are followed through their `source()`.
#[derive(Clone, Debug)]
pub struct Chain<'a> {
    next: Option<&'a (dyn StdError + 'static)>,
}

impl<'a> Iterator for Chain<'a> {
    type Item = &'a (dyn StdError + 'static);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        // A foreign error may hold an error of this type boxed; show the error itself.
        let current: &'a (dyn StdError + 'static) = match current.downcast_ref::<BError>() {
            Some(error) => &**error,
            None => current,
        };
        self.next = current.source();
        Some(current)
    }
}
