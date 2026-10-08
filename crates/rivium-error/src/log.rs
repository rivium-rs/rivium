//! Logging at a level chosen at run time, and logging an error with its fields. The macros reach
//! `tracing` through this crate, so callers need no `tracing` dependency of their own to use
//! them.

/// Emits a `tracing` event at a level chosen at run time, such as the level an HTTP layer picks
/// for a response, optionally with a target.
///
/// ```
/// let level = tracing::Level::WARN;
/// rivium_error::log_at_level!(level, http.status = 503, "request failed");
/// rivium_error::log_at_level!(target: "access", level, "request finished");
/// ```
#[macro_export]
macro_rules! log_at_level {
    (target: $target:expr, $level:expr, $($arg:tt)+) => {
        match $level {
            $crate::__private::tracing::Level::ERROR => $crate::__private::tracing::event!(
                target: $target, $crate::__private::tracing::Level::ERROR, $($arg)+),
            $crate::__private::tracing::Level::WARN => $crate::__private::tracing::event!(
                target: $target, $crate::__private::tracing::Level::WARN, $($arg)+),
            $crate::__private::tracing::Level::INFO => $crate::__private::tracing::event!(
                target: $target, $crate::__private::tracing::Level::INFO, $($arg)+),
            $crate::__private::tracing::Level::DEBUG => $crate::__private::tracing::event!(
                target: $target, $crate::__private::tracing::Level::DEBUG, $($arg)+),
            $crate::__private::tracing::Level::TRACE => $crate::__private::tracing::event!(
                target: $target, $crate::__private::tracing::Level::TRACE, $($arg)+),
        }
    };
    ($level:expr, $($arg:tt)+) => {
        match $level {
            $crate::__private::tracing::Level::ERROR => $crate::__private::tracing::event!(
                $crate::__private::tracing::Level::ERROR, $($arg)+),
            $crate::__private::tracing::Level::WARN => $crate::__private::tracing::event!(
                $crate::__private::tracing::Level::WARN, $($arg)+),
            $crate::__private::tracing::Level::INFO => $crate::__private::tracing::event!(
                $crate::__private::tracing::Level::INFO, $($arg)+),
            $crate::__private::tracing::Level::DEBUG => $crate::__private::tracing::event!(
                $crate::__private::tracing::Level::DEBUG, $($arg)+),
            $crate::__private::tracing::Level::TRACE => $crate::__private::tracing::event!(
                $crate::__private::tracing::Level::TRACE, $($arg)+),
        }
    };
}

/// Logs an error once, where it is handled, with the fields `error.type`, `error.class`,
/// `error.source`, `error.retry`, `error.context`, `error.chain` and `error.cause` (see
/// [`Fields`](crate::Fields)), then the given fields and message.
///
/// ```
/// use rivium_error::{Error, kinds};
///
/// let error = Error::explain(kinds::UNAVAILABLE, "the device does not answer");
/// rivium_error::log_error!(tracing::Level::WARN, &error, service.name = "poller", "poll failed");
/// ```
#[macro_export]
macro_rules! log_error {
    ($level:expr, $error:expr, $($arg:tt)+) => {{
        let fields = $crate::Error::fields($error);
        $crate::log_at_level!(
            $level,
            error.type = fields.etype,
            error.class = fields.class,
            error.source = fields.source,
            error.retry = fields.retry,
            error.context = fields.context.as_str(),
            error.chain = fields.chain.as_str(),
            error.cause = fields.cause.as_str(),
            $($arg)+
        )
    }};
}
