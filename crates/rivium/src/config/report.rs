//! Configuration problems and where values come from.

use std::fmt;
use std::path::PathBuf;

/// Where a value came from, shown as `default`, `file <path>`, `env <NAME>`, `cli --set <key>`
/// or `host --set <key>`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// The default value.
    Default,
    /// The configuration file at this path.
    File(PathBuf),
    /// This environment variable.
    Env(String),
    /// A `--set` option on the command line, for this key.
    Cli(String),
    /// A `--set` argument from the embedded host, for this key.
    Host(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::File(path) => write!(f, "file {}", printable(&path.display().to_string())),
            Source::Env(name) => write!(f, "env {}", printable(name)),
            Source::Cli(key) => write!(f, "cli --set {}", printable(key)),
            Source::Host(key) => write!(f, "host --set {}", printable(key)),
        }
    }
}

/// One problem: with a key, or about a whole input such as a file that is not valid TOML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    key: Option<String>,
    source: Source,
    reason: String,
}

impl Problem {
    pub(crate) fn new(key: Option<String>, source: Source, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Problem {
            key,
            source,
            reason,
        }
    }

    /// The key, such as `log.file.max_file_size`; `None` when the whole input is at fault.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Where the value came from.
    #[must_use]
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// What is wrong.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// One line: `invalid configuration: <key> (<source>): <reason>`, or
/// `invalid configuration: <source>: <reason>` for a whole input.
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = printable(&self.reason);
        match &self.key {
            Some(key) => write!(
                f,
                "invalid configuration: {} ({}): {reason}",
                printable(key),
                self.source
            ),
            None => write!(f, "invalid configuration: {}: {reason}", self.source),
        }
    }
}

/// Every problem found while loading the configuration: whole inputs first, then by key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    problems: Vec<Problem>,
}

impl Report {
    pub(crate) fn new(mut problems: Vec<Problem>) -> Self {
        problems.sort_by(|a, b| a.key.cmp(&b.key));
        problems.dedup();
        Report { problems }
    }

    /// The problems, in order.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }
}

/// One problem per line.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, problem) in self.problems.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

/// Text shown on one line: control characters are escaped, so no input can start a line.
pub(crate) fn printable(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        match c.is_control() {
            true => shown.extend(c.escape_default()),
            false => shown.push(c),
        }
    }
    shown
}
