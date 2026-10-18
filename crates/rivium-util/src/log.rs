//! Scoped tracing, bounded file logging, and optional diagnostic archives.

#[cfg(feature = "log-export")]
mod export;
mod files;
mod format;
mod settings;

#[cfg(feature = "log-export")]
pub use export::{Archive, ExportId, ExportRequest, ExportState, Exporter};

#[cfg(feature = "log-export")]
use settings::duration;
use settings::size;

use rivium::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tracing::Dispatch;
use tracing_subscriber::{Layer, Registry, filter::EnvFilter, layer::SubscriberExt, reload};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Output settings. Only filters are live reloadable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Default filter for file outputs and an interactive console.
    pub filter: String,
    /// Console formatting and filtering.
    pub console: Console,
    /// Main file and shared disk budget.
    pub file: FileSettings,
    /// Additional independently filtered files.
    pub files: Vec<Category>,
    /// Android logcat filter.
    pub logcat: CategoryFilter,
    /// Diagnostic ZIP limits.
    #[cfg(feature = "log-export")]
    pub export: ExportSettings,
}

/// Console output settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Console {
    /// Empty uses the common filter on a terminal, or `warn` when redirected.
    pub filter: String,
    /// Text or structured JSON.
    pub format: Format,
    /// ANSI color policy for text.
    pub color: Color,
}

/// Console output representation.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Text,
    Json,
}

/// ANSI color policy.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

/// File names and budgets, shared by all categories.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FileSettings {
    /// Absolute path or path relative to the host root.
    pub dir: PathBuf,
    /// Empty inherits `Settings::filter`.
    pub filter: String,
    /// Maximum bytes per active or raw archived file.
    #[serde(with = "size")]
    pub max_file_size: u64,
    /// All owned files plus compression and export reservations.
    #[serde(with = "size")]
    pub max_total_size: u64,
}

/// One named, independently filtered file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Unique ASCII letters, digits, `_` or `-`.
    pub name: String,
    /// Empty inherits the common filter.
    pub filter: String,
}

/// Filtering for a platform-specific output.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CategoryFilter {
    /// Platform output filter; logcat defaults to `warn`.
    pub filter: String,
}

/// Diagnostic ZIP limits.
#[cfg(feature = "log-export")]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ExportSettings {
    /// Maximum ZIP bytes, reserved from the disk budget.
    #[serde(with = "size")]
    pub max_size: u64,
    /// Ready archives are deleted after this duration without requiring polling.
    #[serde(with = "duration")]
    pub expire_after: Duration,
}

impl Default for CategoryFilter {
    fn default() -> Self {
        Self {
            filter: "warn".into(),
        }
    }
}

fn ansi(color: Color, terminal: bool, no_color: bool, dumb: bool) -> bool {
    match color {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => terminal && !no_color && !dumb,
    }
}

impl Default for Console {
    fn default() -> Self {
        Self {
            filter: String::new(),
            format: Format::Text,
            color: Color::Auto,
        }
    }
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            dir: "logs".into(),
            filter: String::new(),
            max_file_size: 16 * 1024 * 1024,
            max_total_size: 256 * 1024 * 1024,
        }
    }
}

#[cfg(feature = "log-export")]
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            max_size: 64 * 1024 * 1024,
            expire_after: Duration::from_secs(1800),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            filter: "info".into(),
            console: Console::default(),
            file: FileSettings::default(),
            files: vec![],
            logcat: CategoryFilter::default(),
            #[cfg(feature = "log-export")]
            export: ExportSettings::default(),
        }
    }
}

impl Settings {
    /// Default settings with `logs/<name>` as directory.
    pub fn for_app(name: &str) -> Self {
        let mut settings = Self::default();
        settings.file.dir.push(name);
        settings
    }

    /// Validate all names, filters, and worst-case storage reservations.
    pub fn validate(&self, name: &str) -> Result<()> {
        if !valid_name(name) || self.files.len() > 16 {
            return Err(Error::invalid(
                "invalid log name or more than 16 categories",
            ));
        }
        let mut names = BTreeSet::from([name]);
        for category in &self.files {
            if !valid_name(&category.name) || !names.insert(category.name.as_str()) {
                return Err(Error::invalid("invalid or duplicate log category"));
            }
        }
        if self.file.max_file_size < 1024 {
            return Err(Error::invalid("log file size must be at least 1024 bytes"));
        }
        let needed = self
            .file
            .max_file_size
            .checked_mul(self.files.len() as u64 + 2)
            .and_then(|x| x.checked_add(self.reserve()));
        if needed.is_none_or(|n| n > self.file.max_total_size) {
            return Err(Error::invalid(
                "log disk budget is smaller than file and export reservations",
            ));
        }
        #[cfg(feature = "log-export")]
        if self.export.max_size == 0
            || self.export.expire_after.is_zero()
            || Instant::now()
                .checked_add(self.export.expire_after)
                .is_none()
        {
            return Err(Error::invalid("export size and expiry must be positive"));
        }
        if self.file.dir.as_os_str().is_empty() {
            return Err(Error::invalid("log directory is empty"));
        }
        for filter in self.filter_strings() {
            parse_filter(filter, &self.filter)?;
        }
        Ok(())
    }

    fn filter_strings(&self) -> Vec<&str> {
        std::iter::once(self.filter.as_str())
            .chain([
                self.console.filter.as_str(),
                self.file.filter.as_str(),
                self.logcat.filter.as_str(),
            ])
            .chain(self.files.iter().map(|c| c.filter.as_str()))
            .collect()
    }

    fn reserve(&self) -> u64 {
        #[cfg(feature = "log-export")]
        {
            self.export.max_size
        }
        #[cfg(not(feature = "log-export"))]
        {
            0
        }
    }
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn parse_filter(value: &str, fallback: &str) -> Result<EnvFilter> {
    EnvFilter::try_new(if value.is_empty() { fallback } else { value })
        .map_err(|e| Error::invalid(format!("invalid log filter: {e}")))
}

/// Application-provided tracing layer; no global subscriber is installed.
pub type ExtraLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Approximate counters belonging to this logger, never process-wide totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogStats {
    /// Events currently waiting in the bounded queue.
    pub queued_events: usize,
    /// Payload bytes currently waiting in the queue.
    pub queued_bytes: usize,
    /// New entries dropped because the queue already contained 1024 entries.
    pub dropped_full: u64,
    /// Entries rejected whole after formatting exceeded 64 KiB.
    pub oversized: u64,
    /// Observed file, compression, or reconciliation failures.
    pub write_failures: u64,
}

// Subscriber hooks are user code and may unwind before Logger exists.
struct ConstructionGuard {
    directory: Option<Arc<files::Directory>>,
    writer: files::Worker,
    #[cfg(feature = "log-export")]
    exporter: Option<Exporter>,
    armed: bool,
}

impl Drop for ConstructionGuard {
    fn drop(&mut self) {
        if self.armed {
            // Release the constructor's lease before waiting for workers. The
            // worker threads own their cloned leases and are then the only
            // remaining directory users during shutdown.
            self.directory.take();
            self.writer.request_close();
            #[cfg(feature = "log-export")]
            if let Some(exporter) = &self.exporter {
                exporter.close();
            }
            // Construction is synchronous filesystem work and has no close
            // budget. No Logger is returned to retain these owners on failure.
            while self.writer.pending() != 0 {
                let _ = self
                    .writer
                    .wait_closed(Instant::now() + Duration::from_secs(1));
            }
            #[cfg(feature = "log-export")]
            if let Some(exporter) = &self.exporter {
                while exporter.pending() != 0 {
                    let _ = exporter.wait_closed(Instant::now() + Duration::from_secs(1));
                }
            }
        }
    }
}

/// Owns a scoped subscriber and all directory users until confirmed shutdown.
pub struct Logger {
    dispatch: Dispatch,
    _callsite_guard: Dispatch,
    writer: files::Worker,
    filters: Vec<reload::Handle<EnvFilter, Registry>>,
    name: String,
    settings: Settings,
    console: bool,
    terminal: bool,
    #[cfg(feature = "log-export")]
    exporter: Exporter,
}

impl Logger {
    /// Start independent file logging; `console` enables console output.
    /// This synchronous constructor confirms worker release before returning an
    /// initialization error or unwinding; close budgets apply after construction.
    pub fn new(
        name: &str,
        root: &Path,
        settings: Settings,
        console: bool,
        extra: Vec<ExtraLayer>,
    ) -> Result<Self> {
        use std::io::IsTerminal;

        settings.validate(name)?;
        // A second live dispatcher prevents tracing's single-dispatch cache from
        // recording Never when another thread reaches a scoped callsite first.
        let callsite_guard = Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let dir = if settings.file.dir.is_absolute() {
            settings.file.dir.clone()
        } else {
            root.join(&settings.file.dir)
        };
        let names: Vec<String> = std::iter::once(name.to_owned())
            .chain(settings.files.iter().map(|c| c.name.clone()))
            .collect();

        let directory = files::Directory::open(&dir)?;
        let writer = files::Worker::new(
            Arc::clone(&directory),
            names.clone(),
            &settings.file,
            settings.reserve(),
        )?;
        let mut construction = ConstructionGuard {
            directory: Some(directory.clone()),
            writer: writer.clone(),
            #[cfg(feature = "log-export")]
            exporter: None,
            armed: true,
        };
        #[cfg(feature = "log-export")]
        let exporter = match Exporter::new(
            Arc::clone(&directory),
            names,
            settings.export.clone(),
            settings.file.max_total_size,
        ) {
            Ok(exporter) => exporter,
            Err(error) => {
                writer.request_close();
                return Err(error);
            }
        };
        #[cfg(feature = "log-export")]
        {
            construction.exporter = Some(exporter.clone());
        }
        drop(directory);

        let terminal = console && std::io::stdout().is_terminal();
        let mut layers = extra;
        let mut filters = Vec::new();
        let mut add = |layer: format::EventLayer, value: &str, fallback: &str| -> Result<()> {
            let (filter, handle) = reload::Layer::new(parse_filter(value, fallback)?);
            filters.push(handle);
            layers.push(layer.with_filter(filter).boxed());
            Ok(())
        };
        add(
            format::EventLayer::file(writer.clone(), 0),
            &settings.file.filter,
            &settings.filter,
        )?;
        for (i, category) in settings.files.iter().enumerate() {
            add(
                format::EventLayer::file(writer.clone(), i + 1),
                &category.filter,
                &settings.filter,
            )?;
        }
        if console {
            add(
                format::EventLayer::console(
                    settings.console.format,
                    ansi(
                        settings.console.color,
                        terminal,
                        std::env::var_os("NO_COLOR").is_some(),
                        std::env::var("TERM").ok().as_deref() == Some("dumb"),
                    ),
                ),
                &settings.console.filter,
                if terminal { &settings.filter } else { "warn" },
            )?;
        }
        #[cfg(target_os = "android")]
        if !console {
            add(
                format::EventLayer::logcat(name),
                &settings.logcat.filter,
                "warn",
            )?;
        }

        let admission = writer.clone();
        let dispatch = Dispatch::new(Registry::default().with(layers).with(
            tracing_subscriber::filter::dynamic_filter_fn(move |_, _| !admission.closed()),
        ));
        let logger = Self {
            dispatch,
            _callsite_guard: callsite_guard,
            writer,
            filters,
            name: name.to_owned(),
            settings,
            console,
            terminal,
            #[cfg(feature = "log-export")]
            exporter,
        };
        construction.directory = None;
        construction.armed = false;
        Ok(logger)
    }

    /// Clone the scoped tracing dispatcher.
    pub fn dispatch(&self) -> Dispatch {
        self.dispatch.clone()
    }

    /// Flush all events accepted before this call within a shared waiting budget.
    pub fn flush(&self, within: Duration) -> Result<()> {
        self.writer.flush(within)
    }

    /// Close admission and wait for every directory worker within one budget.
    /// Permanent failures take precedence over waiting deadlines. After any
    /// error, inspect `pending_workers` before reusing the directory; retrying
    /// this call waits for the same workers and preserves their failures.
    pub fn close(&self, within: Duration) -> Result<()> {
        let until = Instant::now()
            .checked_add(within)
            .unwrap_or_else(Instant::now);
        #[cfg(feature = "log-export")]
        self.exporter.close();
        self.writer.request_close();
        let written = self.writer.wait_closed(until);
        #[cfg(feature = "log-export")]
        let written = close_result(written, self.exporter.wait_closed(until));
        written
    }

    /// Number of actual directory workers still retaining resources.
    pub fn pending_workers(&self) -> usize {
        self.writer.pending() + {
            #[cfg(feature = "log-export")]
            {
                self.exporter.pending()
            }
            #[cfg(not(feature = "log-export"))]
            {
                0
            }
        }
    }

    /// Logger-local queue and error counters.
    pub fn stats(&self) -> LogStats {
        self.writer.stats()
    }

    /// Sum of queue-full and oversized drops for this logger.
    pub fn dropped(&self) -> u64 {
        let stats = self.stats();
        stats.dropped_full.saturating_add(stats.oversized)
    }

    /// Effective settings, including unchanged cold fields after reload.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Reload filters and return structural fields that need a new logger.
    pub fn reload(&mut self, new: &Settings) -> Result<Vec<String>> {
        new.validate(&self.name)?;
        let mut cold = Vec::new();
        if new.file.dir != self.settings.file.dir {
            cold.push("log.file.dir".into());
        }
        if new.file.max_file_size != self.settings.file.max_file_size
            || new.file.max_total_size != self.settings.file.max_total_size
        {
            cold.push("log.file.budget".into());
        }
        if new.console.format != self.settings.console.format
            || new.console.color != self.settings.console.color
        {
            cold.push("log.console.format".into());
        }
        let names_match = new.files.iter().map(|c| &c.name).eq(self
            .settings
            .files
            .iter()
            .map(|c| &c.name));
        if !names_match {
            cold.push("log.files".into());
        }
        #[cfg(feature = "log-export")]
        if new.export != self.settings.export {
            cold.push("log.export".into());
        }
        let mut parsed = vec![parse_filter(&new.file.filter, &new.filter)?];
        for category in &self.settings.files {
            let value = new
                .files
                .iter()
                .find(|c| c.name == category.name)
                .map_or(category.filter.as_str(), |c| c.filter.as_str());
            parsed.push(parse_filter(value, &new.filter)?);
        }
        if self.console {
            parsed.push(parse_filter(
                &new.console.filter,
                if self.terminal { &new.filter } else { "warn" },
            )?);
        }
        #[cfg(target_os = "android")]
        if !self.console {
            parsed.push(parse_filter(&new.logcat.filter, "warn")?);
        }
        for (handle, filter) in self.filters.iter().zip(parsed) {
            handle
                .reload(filter)
                .map_err(|e| Error::internal(format!("log filter reload failed: {e}")))?;
        }
        self.settings.filter.clone_from(&new.filter);
        self.settings.file.filter.clone_from(&new.file.filter);
        self.settings.console.filter.clone_from(&new.console.filter);
        self.settings.logcat.filter.clone_from(&new.logcat.filter);
        for category in &mut self.settings.files {
            if let Some(next) = new.files.iter().find(|c| c.name == category.name) {
                category.filter.clone_from(&next.filter);
            }
        }
        Ok(cold)
    }

    /// Return the bounded diagnostic export controller.
    #[cfg(feature = "log-export")]
    pub fn exporter(&self) -> Exporter {
        self.exporter.clone()
    }
}

#[cfg(feature = "log-export")]
fn close_result(writer: Result<()>, export: Result<()>) -> Result<()> {
    match (writer, export) {
        (Err(error), _) if error.class() != rivium::error::Class::Timeout => Err(error),
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result,
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        let _ = self.close(Duration::ZERO);
    }
}

/// Process-cumulative written bytes, write failures, and evicted archives.
/// These values are never reset by creating or closing an individual logger.
pub fn counters() -> [u64; 3] {
    files::counters()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_color_respects_terminal_no_color_and_dumb_terminal() {
        for (terminal, no_color, dumb, expected) in [
            (true, false, false, true),
            (false, false, false, false),
            (true, true, false, false),
            (true, false, true, false),
        ] {
            assert_eq!(ansi(Color::Auto, terminal, no_color, dumb), expected);
            assert!(ansi(Color::Always, terminal, no_color, dumb));
            assert!(!ansi(Color::Never, terminal, no_color, dumb));
        }
        assert_eq!(Settings::default().logcat.filter, "warn");
    }

    #[cfg(feature = "log-export")]
    #[test]
    fn permanent_writer_failure_survives_an_unfinished_export() {
        let temp = rivium_test::TempDir::new().unwrap();
        let mut settings = Settings::for_app("mixed");
        settings.file.max_file_size = 1024;
        let log = Logger::new("mixed", temp.path(), settings, false, vec![]).unwrap();
        tracing::dispatcher::with_default(&log.dispatch(), || {
            tracing::info!("{}", "x".repeat(2048))
        });
        log.writer.request_close();
        let expected = log
            .writer
            .wait_closed(Instant::now() + Duration::from_secs(2))
            .unwrap_err();
        let held = log.exporter.hold_release();
        let budget = Duration::from_millis(10);
        let before = Instant::now();
        let actual = log.close(budget).unwrap_err();
        assert!(before.elapsed() >= budget);
        assert_eq!(actual.to_string(), expected.to_string());
        assert_eq!(log.pending_workers(), 1);
        drop(held);
        assert_eq!(
            log.close(Duration::from_secs(2)).unwrap_err().to_string(),
            expected.to_string()
        );
        assert_eq!(log.pending_workers(), 0);
        let waiting = Error::new(rivium::error::kinds::TIMEOUT, "writer waiting");
        let failed = Error::internal("export failed");
        assert_eq!(
            close_result(Err(waiting), Err(failed))
                .unwrap_err()
                .message(),
            "export failed"
        );
    }

    #[cfg(feature = "log-export")]
    #[test]
    fn stored_failures_remain_visible_before_worker_release() {
        for failing_writer in [true, false] {
            let temp = rivium_test::TempDir::new().unwrap();
            let mut settings = Settings::for_app("pending");
            settings.file.max_file_size = 1024;
            let log = Logger::new("pending", temp.path(), settings, false, vec![]).unwrap();
            let wait = Duration::from_secs(2);
            let (held, expected) = if failing_writer {
                tracing::dispatcher::with_default(&log.dispatch(), || {
                    tracing::info!("{}", "x".repeat(2048))
                });
                let expected = log.flush(wait).unwrap_err().to_string();
                log.exporter.close();
                log.exporter.wait_closed(Instant::now() + wait).unwrap();
                (log.writer.hold_release(), expected)
            } else {
                log.writer.request_close();
                log.writer.wait_closed(Instant::now() + wait).unwrap();
                let id = log
                    .exporter
                    .start(ExportRequest {
                        from: "1970-01-01".into(),
                        to: "9999-12-31".into(),
                        sinks: vec![],
                    })
                    .unwrap();
                let until = Instant::now() + wait;
                let archive = loop {
                    match log.exporter.progress(id).unwrap() {
                        ExportState::Ready(archive) => break archive,
                        ExportState::Running => {
                            assert!(Instant::now() < until);
                            std::thread::yield_now();
                        }
                        state => panic!("unexpected export state: {state:?}"),
                    }
                };
                std::fs::remove_file(&archive.path).unwrap();
                std::fs::create_dir(&archive.path).unwrap();
                log.exporter.cancel(id);
                let ExportState::Failed(expected) = log.exporter.progress(id).unwrap() else {
                    panic!("cleanup must fail on a directory");
                };
                (log.exporter.hold_release(), expected)
            };
            let budget = Duration::from_millis(10);
            let before = Instant::now();
            let actual = log.close(budget).unwrap_err();
            assert!(before.elapsed() >= budget);
            assert_eq!(actual.to_string(), expected, "writer={failing_writer}");
            assert_eq!(log.pending_workers(), 1);
            drop(held);
            assert_eq!(log.close(wait).unwrap_err().to_string(), expected);
            assert_eq!(log.pending_workers(), 0);
        }
    }
}
