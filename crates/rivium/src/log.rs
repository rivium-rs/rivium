//! Logging. The hosts install it: one output per destination, each with its own filter and
//! nothing filtering them all, so an event can reach several outputs.
//!
//! | Output | Filter | Written by |
//! | --- | --- | --- |
//! | standard output, text or JSON | `log.console.filter`; empty: `log.filter` on a terminal, `warn` otherwise | the process host |
//! | the main file `<name>.log` | `log.file.filter`; empty: `log.filter` | both hosts |
//! | each category file `<category>.log` of `[[log.files]]` | its own | both hosts |
//! | Android's log, tagged with the service name | `log.logcat.filter` | the embedded host, on Android |
//! | layers from `App::log_layers` | their own | both hosts |
//!
//! Files live in `log.file.dir` (`logs/<name>` below the root) and roll by UTC day and by
//! `log.file.max_file_size`; rolled files are compressed, and the oldest are deleted to keep
//! the directory within `log.file.max_total_size` (plus one file size per active file). Records
//! of the `log` crate are logged as events too. A process installs logging once; later
//! installations, as in-process restarts make, only reload the filters.
//!
//! With the feature `log-export`, `LogExporter` packs log files into an archive to download;
//! `[log.export]` sets its size, a share of the directory's budget, and how long it is kept.

mod budget;
#[cfg(feature = "log-export")]
mod export;
mod files;
mod install;
#[cfg(any(test, target_os = "android"))]
mod logcat;
mod panic;
mod settings;
#[cfg(test)]
mod tests;

use std::sync::PoisonError;
use std::time::Duration;

use rivium_error::{Error, kinds};

#[cfg(feature = "log-export")]
pub use export::{Archive, Date, ExportId, ExportRequest, ExportState, LogExporter, Progress};
pub(crate) use install::{InstallError, LogInputs, install};
pub(crate) use panic::{install_panic_hook, payload_text};
pub(crate) use settings::LogSettings;

/// A layer for the global subscriber, as `App::log_layers` adds them: for example a bridge to
/// another telemetry system. It brings its own filter.
pub type BoxedLayer =
    Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>;

/// Waits until every event logged before the call is in the log files: written to the
/// operating system, not synced to disk. Returns at once when logging is not installed.
///
/// # Errors
///
/// A [`kinds::TIMEOUT`] error when the log files' writer did not get there within `timeout`.
pub fn flush(timeout: Duration) -> crate::Result<()> {
    let installed = install::INSTALLED
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    match installed.as_ref() {
        Some(installed) if !installed.files.flush(timeout) => Error::e_explain(
            kinds::TIMEOUT,
            format!("the log files were not flushed within {timeout:?}"),
        ),
        _ => Ok(()),
    }
}

/// The exporter of the process's log files; it fails every export until logging is installed.
#[cfg(feature = "log-export")]
pub(crate) fn exporter() -> LogExporter {
    let installed = install::INSTALLED.lock();
    let installed = installed.unwrap_or_else(PoisonError::into_inner);
    LogExporter(
        installed
            .as_ref()
            .map(|installed| installed.exports.clone()),
    )
}

/// Cancels the log export in progress, if any, and waits up to `within` for it to end: the
/// hosts call it as they tear down, but not between rounds.
pub(crate) fn cancel_export(within: Duration) {
    #[cfg(feature = "log-export")]
    if let Some(exports) = exporter().0 {
        exports.cancel_running(within);
    }
    #[cfg(not(feature = "log-export"))]
    let _ = within;
}

/// Whether logging is installed in this process.
pub(crate) fn installed() -> bool {
    let installed = install::INSTALLED.lock();
    installed.unwrap_or_else(PoisonError::into_inner).is_some()
}

/// The counters of the installed log files: bytes written, write failures, files deleted by the
/// budget.
pub(crate) fn counters() -> [u64; 3] {
    use std::sync::atomic::Ordering::Relaxed;
    let installed = install::INSTALLED
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    installed.as_ref().map_or([0; 3], |installed| {
        let counters = &installed.files.counters;
        [
            counters.bytes.load(Relaxed),
            counters.failures.load(Relaxed),
            counters.deleted.load(Relaxed),
        ]
    })
}
