//! Installing logging: one layer per output, each with a filter that later runs reload.

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, reload};

use super::BoxedLayer;
use super::files::{Files, FilesConfig, SinkSender};
use super::settings::{Format, LogSettings};

/// What logging is installed with; the host resolves the directory against the root.
pub(crate) struct LogInputs {
    pub(crate) name: String,
    pub(crate) settings: LogSettings,
    pub(crate) dir: PathBuf,
    /// Whether to write standard output: the process host does, the embedded host does not.
    pub(crate) console: bool,
    /// Whether to write Android's log: the embedded host does, on Android.
    #[cfg_attr(
        not(target_os = "android"),
        expect(dead_code, reason = "read on Android only")
    )]
    pub(crate) logcat: bool,
    /// Layers the service adds, kept from the first installation on.
    pub(crate) layers: Vec<BoxedLayer>,
}

/// Why logging could not be installed.
#[derive(Debug)]
pub(crate) enum InstallError {
    /// These settings differ from the ones logging was installed with, and cannot change while
    /// the process runs: a configuration problem.
    Changed(Vec<&'static str>),
    /// The log directory or a log file cannot be created or written, or another subscriber is
    /// already installed.
    Io(io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Changed(keys) => {
                write!(
                    f,
                    "{} cannot change while the process runs",
                    keys.join(" and ")
                )
            }
            InstallError::Io(error) => write!(f, "cannot write the log files: {error}"),
        }
    }
}

/// The filter handle of each output, with the output's place in [`filters`].
type Filters = Vec<(usize, reload::Handle<EnvFilter, Registry>)>;

/// Logging as installed in this process.
pub(super) struct Installed {
    settings: LogSettings,
    dir: PathBuf,
    pub(super) files: Files,
    filters: Filters,
}

/// The process's logging, installed once.
pub(super) static INSTALLED: Mutex<Option<Installed>> = Mutex::new(None);

/// Installs logging the first time; afterwards only reloads the filters. The directory and the
/// set of files cannot change; other settings that differ keep their installed values until the
/// process restarts, with a warning.
///
/// # Errors
///
/// See [`InstallError`].
pub(crate) fn install(inputs: LogInputs) -> Result<(), InstallError> {
    let mut installed = INSTALLED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(installed) = installed.as_ref() {
        return installed.update(&inputs);
    }
    let (files, mut layers, filters) = build(&inputs, BoxMakeWriter::new(io::stdout))?;
    layers.extend(inputs.layers);
    // Records of the `log` crate become events too; reloading a filter also updates the `log`
    // crate's level. When another subscriber is installed, this drops the layers and with them
    // the senders, so the threads of the log files end.
    let subscriber = Registry::default().with(layers);
    subscriber
        .try_init()
        .map_err(|e| InstallError::Io(io::Error::other(e)))?;
    let (settings, dir) = (inputs.settings, inputs.dir);
    *installed = Some(Installed {
        settings,
        dir,
        files,
        filters,
    });
    Ok(())
}

impl Installed {
    fn update(&self, inputs: &LogInputs) -> Result<(), InstallError> {
        let (now, then) = (&inputs.settings, &self.settings);
        let changed: Vec<&'static str> = [
            ("log.file.dir", inputs.dir != self.dir),
            (
                "log.files",
                sinks(&inputs.name, now) != sinks(&inputs.name, then),
            ),
        ]
        .into_iter()
        .filter_map(|(key, differs)| differs.then_some(key))
        .collect();
        if !changed.is_empty() {
            return Err(InstallError::Changed(changed));
        }
        let new = filters(now);
        for (index, handle) in &self.filters {
            let _ = handle.reload(EnvFilter::new(&new[*index]));
        }
        let kept: Vec<&str> = [
            (
                "log.file.max_file_size",
                now.file.max_file_size != then.file.max_file_size,
            ),
            (
                "log.file.max_total_size",
                now.file.max_total_size != then.file.max_total_size,
            ),
            (
                "log.console.format",
                now.console.format != then.console.format,
            ),
        ]
        .into_iter()
        .filter_map(|(key, differs)| differs.then_some(key))
        .collect();
        if !kept.is_empty() {
            tracing::warn!(keys = ?kept, "log settings changed; they take effect when the process restarts");
        }
        Ok(())
    }
}

/// The base names of the log files: the main file, named after the service, then categories.
fn sinks(name: &str, settings: &LogSettings) -> Vec<String> {
    let categories = settings.files.iter().map(|category| category.name.clone());
    std::iter::once(name.to_string())
        .chain(categories)
        .collect()
}

/// The filter of each output: the main file, each category, the console, then logcat. An output
/// without its own filter uses `log.filter`; the console's empty filter means `log.filter` on a
/// terminal and `warn` otherwise.
pub(super) fn filters(settings: &LogSettings) -> Vec<String> {
    let or_default = |own: &str| {
        if own.is_empty() {
            settings.filter.clone()
        } else {
            own.to_string()
        }
    };
    let console = match settings.console.filter.as_str() {
        "" if !io::stdout().is_terminal() => "warn".to_string(),
        own => or_default(own),
    };
    let categories = settings
        .files
        .iter()
        .map(|category| category.filter.clone());
    std::iter::once(or_default(&settings.file.filter))
        .chain(categories)
        .chain([console, settings.logcat.filter.clone()])
        .collect()
}

/// Starts the log files and builds the output layers with their filter handles.
pub(super) fn build(
    inputs: &LogInputs,
    console: BoxMakeWriter,
) -> Result<(Files, Vec<BoxedLayer>, Filters), InstallError> {
    let settings = &inputs.settings;
    let config = FilesConfig {
        dir: inputs.dir.clone(),
        sinks: sinks(&inputs.name, settings),
        max_file_size: settings.file.max_file_size,
        max_total_size: settings.file.max_total_size,
        export_reserve: 0,
    };
    let files = Files::start(config, Arc::new(SystemTime::now)).map_err(InstallError::Io)?;
    let filters = filters(settings);
    let mut handles = Vec::new();
    let mut filtered = |layer: BoxedLayer, index: usize| {
        let (filter, handle) = reload::Layer::new(EnvFilter::new(&filters[index]));
        handles.push((index, handle));
        layer.with_filter(filter).boxed()
    };
    let sinks = settings.files.len() + 1;
    let mut layers: Vec<BoxedLayer> = (0..sinks)
        .map(|sink| {
            let layer = fmt::layer()
                .with_ansi(false)
                .with_writer(files.sender(sink));
            filtered(layer.boxed(), sink)
        })
        .collect();
    if inputs.console {
        let text = fmt::layer().with_ansi(false).with_writer(console);
        let layer = match settings.console.format {
            Format::Text => text.boxed(),
            Format::Json => text.json().flatten_event(true).boxed(),
        };
        layers.push(filtered(layer, sinks));
    }
    #[cfg(target_os = "android")]
    if inputs.logcat {
        let logcat = super::logcat::Logcat::new(&inputs.name);
        let layer = fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(logcat);
        layers.push(filtered(layer.boxed(), sinks + 1));
    }
    Ok((files, layers, handles))
}

impl<'a> MakeWriter<'a> for SinkSender {
    type Writer = &'a SinkSender;

    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

/// Each formatted event arrives in one write and becomes one queued line.
impl io::Write for &SinkSender {
    fn write(&mut self, line: &[u8]) -> io::Result<usize> {
        match self.send(line.to_vec()) {
            true => Ok(line.len()),
            false => Err(io::Error::other("the log writer has stopped")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
