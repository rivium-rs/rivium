//! The `[log]` section of the configuration.

use std::path::PathBuf;

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
}

/// `[log.console]`: standard output, written by the process host only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Console {
    /// Empty: `log.filter` when standard output is a terminal, `warn` otherwise.
    #[serde(deserialize_with = "optional_filter")]
    pub(crate) filter: String,
    pub(crate) format: Format,
}

/// The format of console lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Text,
    Json,
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
            },
            file: File {
                dir: PathBuf::from("logs").join(name),
                filter: String::new(),
                max_file_size: 16 * MIB,
                max_total_size: 256 * MIB,
            },
            files: Vec::new(),
            logcat: Logcat {
                filter: "warn".into(),
            },
        }
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
        // The active files and the archive being compressed must fit in the log files' share.
        let sinks = 1 + self.files.len() as u64;
        let least = (sinks + 2).saturating_mul(self.file.max_file_size);
        if self.file.max_total_size < least {
            let reason = format!(
                "must be at least {} for {sinks} log file(s): ({sinks} + 2) × log.file.max_file_size",
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
