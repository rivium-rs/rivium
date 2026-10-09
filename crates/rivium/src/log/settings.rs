//! The `[log]` section of the configuration.

use std::path::PathBuf;
#[cfg(feature = "log-export")]
use std::time::Duration;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

use crate::config::de::{bytes, format_bytes, serialize_bytes};

/// `[log]`: every output has its own filter; nothing filters them all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LogSettings {
    /// `log.filter`: the filter of the outputs that do not set their own, in `RUST_LOG` syntax.
    #[serde(deserialize_with = "filter")]
    pub(crate) filter: String,
    pub(crate) console: Console,
    pub(crate) file: File,
    /// `[[log.files]]`: category files, each with its own filter.
    pub(crate) files: Vec<Category>,
    pub(crate) logcat: Logcat,
    #[cfg(feature = "log-export")]
    pub(crate) export: Export,
}

/// `[log.console]`: standard output, written by the process host only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Console {
    /// Empty: `log.filter` when standard output is a terminal, `warn` otherwise.
    #[serde(deserialize_with = "optional_filter")]
    pub(crate) filter: String,
    pub(crate) format: Format,
    /// Whether text lines are colored; JSON lines never are.
    pub(crate) color: Color,
}

/// The format of console lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Text,
    Json,
}

/// Whether console lines are colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Color {
    /// On a terminal, unless `NO_COLOR` is set or `TERM` is `dumb`.
    Auto,
    Always,
    Never,
}

/// `[log.file]`: the main log file `<name>.log` and the disk budget of the log directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct File {
    /// Relative to the root directory.
    pub(crate) dir: PathBuf,
    /// Empty: `log.filter`.
    #[serde(deserialize_with = "optional_filter")]
    pub(crate) filter: String,
    #[serde(
        serialize_with = "serialize_bytes",
        deserialize_with = "bytes::<_, MIB, GIB>"
    )]
    pub(crate) max_file_size: u64,
    #[serde(
        serialize_with = "serialize_bytes",
        deserialize_with = "bytes::<_, MIB, { u64::MAX }>"
    )]
    pub(crate) max_total_size: u64,
}

/// `[[log.files]]`: a category file `<name>.log`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Category {
    pub(crate) name: String,
    #[serde(deserialize_with = "filter")]
    pub(crate) filter: String,
}

/// `[log.export]`: log export archives, which keep a share of the log directory's budget.
#[cfg(feature = "log-export")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Export {
    /// The largest archive, and the share of `log.file.max_total_size` kept for it.
    #[serde(
        serialize_with = "serialize_bytes",
        deserialize_with = "bytes::<_, MIB, GIB>"
    )]
    pub(crate) max_size: u64,
    /// How long a finished archive can be downloaded.
    #[serde(
        serialize_with = "crate::config::de::serialize_duration",
        deserialize_with = "crate::config::de::duration::<_, 1, 86_400>"
    )]
    pub(crate) expire_after: Duration,
}

/// `[log.logcat]`: Android's log, written by the embedded host on Android only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Logcat {
    #[serde(deserialize_with = "filter")]
    pub(crate) filter: String,
}

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

impl LogSettings {
    /// The defaults of the service `name`.
    pub(crate) fn new(name: &str) -> Self {
        LogSettings {
            filter: "info".into(),
            console: Console {
                filter: String::new(),
                format: Format::Text,
                color: Color::Auto,
            },
            file: File {
                // Written with `/`, so the default configuration is the same on every platform.
                dir: PathBuf::from(format!("logs/{name}")),
                filter: String::new(),
                max_file_size: 16 * MIB,
                max_total_size: 256 * MIB,
            },
            files: Vec::new(),
            logcat: Logcat {
                filter: "warn".into(),
            },
            #[cfg(feature = "log-export")]
            export: Export {
                max_size: 64 * MIB,
                expire_after: Duration::from_secs(30 * 60),
            },
        }
    }

    /// The keys whose values differ from `installed` and cannot change while the process runs:
    /// every setting but the filters.
    pub(crate) fn unchangeable(&self, installed: &LogSettings) -> Vec<&'static str> {
        let names = |settings: &LogSettings| {
            (settings.files.iter())
                .map(|category| category.name.clone())
                .collect::<Vec<_>>()
        };
        let (now, then) = (&self.file, &installed.file);
        [
            ("log.file.dir", now.dir != then.dir),
            ("log.files", names(self) != names(installed)),
            (
                "log.file.max_file_size",
                now.max_file_size != then.max_file_size,
            ),
            (
                "log.file.max_total_size",
                now.max_total_size != then.max_total_size,
            ),
            (
                "log.console.format",
                self.console.format != installed.console.format,
            ),
            (
                "log.console.color",
                self.console.color != installed.console.color,
            ),
            #[cfg(feature = "log-export")]
            (
                "log.export.max_size",
                self.export.max_size != installed.export.max_size,
            ),
            #[cfg(feature = "log-export")]
            (
                "log.export.expire_after",
                self.export.expire_after != installed.export.expire_after,
            ),
        ]
        .into_iter()
        .filter_map(|(key, differs)| differs.then_some(key))
        .collect()
    }

    /// The share of `log.file.max_total_size` kept for log export archives.
    pub(crate) fn export_reserve(&self) -> u64 {
        #[cfg(feature = "log-export")]
        return self.export.max_size;
        #[cfg(not(feature = "log-export"))]
        0
    }

    /// Problems that no single value shows, as (key, reason); `name` is the main file's.
    pub(crate) fn problems(&self, name: &str) -> Vec<(String, String)> {
        let mut problems = Vec::new();
        if self.file.dir.as_os_str().is_empty() {
            problems.push(("log.file.dir".into(), "must not be empty".into()));
        }
        let mut names = vec![name];
        for (index, category) in self.files.iter().enumerate() {
            let key = format!("log.files[{index}].name");
            let name = category.name.as_str();
            let valid = !name.is_empty()
                && (name.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if !valid {
                problems.push((key, "must be letters, digits, `_` and `-`".into()));
            } else if names.contains(&name) {
                problems.push((key, format!("`{name}` is the name of another log file")));
            }
            names.push(name);
        }
        // The active files and the archive being compressed must fit in the log files' share,
        // besides the export archive's.
        let sinks = 1 + self.files.len() as u64;
        let least = (sinks + 2)
            .saturating_mul(self.file.max_file_size)
            .saturating_add(self.export_reserve());
        if self.file.max_total_size < least {
            let export = match self.export_reserve() {
                0 => "",
                _ => " + log.export.max_size",
            };
            let reason = format!(
                "must be at least {} for {sinks} log file(s): ({sinks} + 2) × log.file.max_file_size{export}",
                format_bytes(least)
            );
            problems.push(("log.file.max_total_size".into(), reason));
        }
        problems
    }
}

/// A filter in `RUST_LOG` syntax with at least one directive; `off` turns an output off.
fn filter<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    check(&text).map(|()| text)
}

/// Like [`filter`], or empty.
fn optional_filter<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        Ok(text)
    } else {
        check(&text).map(|()| text)
    }
}

fn check<E: de::Error>(text: &str) -> Result<(), E> {
    // `,` alone parses, as a filter without directives that turns everything off.
    if text.split(',').all(|directive| directive.trim().is_empty()) {
        return Err(E::custom("not a log filter: it has no directive"));
    }
    EnvFilter::try_new(text)
        .map(drop)
        .map_err(|error| E::custom(format!("not a log filter: {error}")))
}
