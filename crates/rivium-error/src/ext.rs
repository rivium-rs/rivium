//! Extension traits that turn other results and options into this crate's errors.

use std::borrow::Cow;
use std::error::Error as StdError;

use crate::{Error, ErrorType, Result, kinds};

/// Turns the error of any `Result` into an [`Error`].
pub trait OrErr<T, E> {
    /// An error of `kind` with this context, caused by the original error.
    ///
    /// # Errors
    ///
    /// When `self` is an error.
    fn or_err(self, kind: ErrorType, context: impl Into<Cow<'static, str>>) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>;

    /// Like [`OrErr::or_err`], with the context built only on error.
    ///
    /// # Errors
    ///
    /// When `self` is an error.
    fn or_err_with<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce() -> C,
    ) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>;

    /// An error of `kind` whose context is built from the original error, which is dropped. For
    /// errors that cannot be kept as a cause.
    ///
    /// # Errors
    ///
    /// When `self` is an error.
    fn explain_err<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce(E) -> C,
    ) -> Result<T>;

    /// An [`kinds::INTERNAL`] error caused by the original error, without context. Prefer
    /// [`OrErr::or_err`], which says what was being done.
    ///
    /// # Errors
    ///
    /// When `self` is an error.
    fn or_fail(self) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>;
}

impl<T, E> OrErr<T, E> for std::result::Result<T, E> {
    fn or_err(self, kind: ErrorType, context: impl Into<Cow<'static, str>>) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>,
    {
        self.map_err(|error| Error::because(kind, context, error))
    }

    fn or_err_with<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce() -> C,
    ) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>,
    {
        self.map_err(|error| Error::because(kind, context(), error))
    }

    fn explain_err<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce(E) -> C,
    ) -> Result<T> {
        self.map_err(|error| Error::explain(kind, context(error)))
    }

    fn or_fail(self) -> Result<T>
    where
        E: Into<Box<dyn StdError + Send + Sync>>,
    {
        self.map_err(|error| Error::because(kinds::INTERNAL, "", error))
    }
}

/// Turns a `None` into an [`Error`].
pub trait OkOrErr<T> {
    /// An error of `kind` with this context when `self` is `None`.
    ///
    /// # Errors
    ///
    /// When `self` is `None`.
    fn or_err(self, kind: ErrorType, context: impl Into<Cow<'static, str>>) -> Result<T>;

    /// Like [`OkOrErr::or_err`], with the context built only when `self` is `None`.
    ///
    /// # Errors
    ///
    /// When `self` is `None`.
    fn or_err_with<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce() -> C,
    ) -> Result<T>;
}

impl<T> OkOrErr<T> for Option<T> {
    fn or_err(self, kind: ErrorType, context: impl Into<Cow<'static, str>>) -> Result<T> {
        self.ok_or_else(|| Error::explain(kind, context))
    }

    fn or_err_with<C: Into<Cow<'static, str>>>(
        self,
        kind: ErrorType,
        context: impl FnOnce() -> C,
    ) -> Result<T> {
        self.ok_or_else(|| Error::explain(kind, context()))
    }
}

/// Adds context to the error of a [`Result`].
pub trait Context<T> {
    /// The error wrapped with more context; see [`Error::more_context`].
    ///
    /// # Errors
    ///
    /// When `self` is an error.
    fn err_context<C: Into<Cow<'static, str>>>(self, context: impl FnOnce() -> C) -> Result<T>;
}

impl<T> Context<T> for Result<T> {
    fn err_context<C: Into<Cow<'static, str>>>(self, context: impl FnOnce() -> C) -> Result<T> {
        self.map_err(|error| error.more_context(context()))
    }
}
