//! Explicit configuration layers and immutable, revisioned reader snapshots.

mod de;

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use rivium::{
    Error,
    error::{ErrorType, kinds},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::watch;
use toml::Value;

/// The input responsible for a configuration value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    /// Application defaults.
    Default,
    /// A TOML document at this path.
    File(PathBuf),
    /// An explicitly supplied environment variable.
    Environment(String),
    /// An explicitly supplied dotted-key assignment.
    Override(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::File(path) => write!(f, "file {}", path.display()),
            Self::Environment(name) => write!(f, "env {name}"),
            Self::Override(key) => write!(f, "--set {key}"),
        }
    }
}

/// One configuration diagnostic. It may contain sensitive input details.
#[derive(Clone, Debug)]
pub struct Problem {
    /// Dotted configuration field path.
    pub key: String,
    /// The input responsible for this field.
    pub source: Source,
    /// Diagnostic detail intended for authorized callers.
    pub message: String,
    /// Machine-readable error classification.
    pub kind: ErrorType,
}

/// Configuration diagnostics collected during loading or decoding.
#[derive(Clone, Debug)]
pub struct Report(pub Vec<Problem>);

impl Report {
    fn one(key: impl Into<String>, source: Source, message: impl fmt::Display) -> Self {
        Self(vec![Problem {
            key: key.into(),
            source,
            message: message.to_string(),
            kind: kinds::INVALID_INPUT,
        }])
    }

    fn too_large(source: Source, limit: usize) -> Self {
        let mut report = Self::one("", source, format!("document exceeds {limit} bytes"));
        report.0[0].kind = kinds::PAYLOAD_TOO_LARGE;
        report
    }

    /// The overall classification; document size errors retain their own kind.
    pub fn kind(&self) -> ErrorType {
        self.0
            .iter()
            .find(|problem| problem.kind == kinds::PAYLOAD_TOO_LARGE)
            .map_or(kinds::INVALID_INPUT, |problem| problem.kind)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, problem) in self.0.iter().enumerate() {
            if index != 0 {
                writeln!(f)?;
            }
            write!(
                f,
                "invalid configuration: {} ({}): {}",
                problem.key,
                problem.source,
                problem.message.replace(['\r', '\n'], " ")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Report {}

/// Bounds applied before a configuration document is parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadLimits {
    /// Maximum number of UTF-8 bytes in the TOML input.
    pub max_document_bytes: usize,
}

impl Default for LoadLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 1024 * 1024,
        }
    }
}

/// A merged document, retaining source information and typed text assignments.
#[derive(Clone, Debug)]
pub struct Document {
    /// Merged TOML representation.
    pub value: Value,
    /// Source at each supplied branch or leaf.
    pub sources: BTreeMap<String, Source>,
    text: BTreeMap<String, String>,
    defaults: Value,
}

impl Document {
    /// Find a field's source, falling back to the closest enclosing assignment.
    pub fn source(&self, path: &str) -> Source {
        let mut current = path;
        loop {
            if let Some(source) = self.sources.get(current) {
                return source.clone();
            }
            match current.rfind(['.', '[']) {
                Some(position) => current = &current[..position],
                None => return Source::Default,
            }
        }
    }

    /// Deserialize all fields and reject unknown keys.
    ///
    /// Diagnostics cover independent invalid fields where a default or omission
    /// allows decoding to continue. A custom deserializer may stop at its first
    /// invalid invariant; no arbitrary business validation is inferred.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, Report> {
        self.decode_at(self.value.clone(), self.defaults.clone(), "")
    }

    /// Decode one named section, rejecting unknown keys within that section.
    pub fn section<T: DeserializeOwned>(&self, name: &str) -> Result<T, Report> {
        let value = lookup(&self.value, name)
            .cloned()
            .unwrap_or_else(empty_table);
        let defaults = lookup(&self.defaults, name)
            .cloned()
            .unwrap_or_else(empty_table);
        self.decode_at(value, defaults, name)
    }

    fn decode_at<T: DeserializeOwned>(
        &self,
        mut value: Value,
        defaults: Value,
        prefix: &str,
    ) -> Result<T, Report> {
        let mut problems = Vec::new();
        let mut text = self.text.clone();
        let mut repaired = std::collections::BTreeSet::new();
        loop {
            let mut ignored = Vec::new();
            let mut tracker = serde_path_to_error::Track::new();
            let decoder = de::Input {
                value: value.clone(),
                path: prefix.into(),
                text: &text,
            };
            let decoder = serde_path_to_error::Deserializer::new(decoder, &mut tracker);
            let decoded = serde_ignored::deserialize::<_, _, T>(decoder, |path| {
                ignored.push(path.to_string())
            });
            for path in ignored {
                let key = dotted(prefix, &path);
                if !problems
                    .iter()
                    .any(|p: &Problem| p.key == key && p.message == "unknown key")
                {
                    problems.push(Problem {
                        source: self.source(&key),
                        key,
                        message: "unknown key".into(),
                        kind: kinds::INVALID_INPUT,
                    });
                }
            }
            match decoded {
                Ok(value) if problems.is_empty() => return Ok(value),
                Ok(_) => return Err(Report(problems)),
                Err(error) => {
                    let local = tracker.path().to_string();
                    let local = local.trim_start_matches('.');
                    let key = dotted(prefix, local);
                    if !repaired.insert(key.clone()) {
                        return Err(Report(problems));
                    }
                    problems.push(Problem {
                        source: self.source(&key),
                        key: key.clone(),
                        message: error.to_string(),
                        kind: kinds::INVALID_INPUT,
                    });
                    clear_branch(&mut text, &key);
                    if !substitute(&mut value, &defaults, local) {
                        return Err(Report(problems));
                    }
                }
            }
        }
    }

    /// Render values and their origins. The result may contain secrets.
    pub fn listing(&self) -> String {
        let mut entries = Vec::new();
        list_leaves(&self.value, "", &mut |key, value| {
            entries.push(format!("{key} = {value} ({})", self.source(key)));
        });
        entries.join("\n")
    }
}

/// Build a document from defaults, file, explicit environment and assignments.
///
/// Environment variables are never read implicitly, and cwd is never changed.
#[derive(Clone, Debug)]
pub struct Loader {
    defaults: Value,
    file: Option<PathBuf>,
    environment: Vec<(String, String)>,
    prefix: String,
    overrides: Vec<String>,
    limits: LoadLimits,
}

impl Loader {
    /// Serialize the application's defaults into a TOML document.
    pub fn new(defaults: &impl Serialize) -> Result<Self, Report> {
        let defaults =
            Value::try_from(defaults).map_err(|error| Report::one("", Source::Default, error))?;
        if !defaults.is_table() {
            return Err(Report::one("", Source::Default, "defaults must be a table"));
        }
        Ok(Self {
            defaults,
            file: None,
            environment: Vec::new(),
            prefix: String::new(),
            overrides: Vec::new(),
            limits: LoadLimits::default(),
        })
    }

    /// Set an optional file; a missing file leaves defaults in effect.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Supply environment inputs. `PREFIX_A__B` addresses `a.b`.
    ///
    /// A single `_` remains part of a field name. Empty prefixes do not expose
    /// the whole process environment. Inputs are sorted for deterministic merges.
    pub fn environment(
        mut self,
        prefix: impl Into<String>,
        values: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.prefix = prefix.into();
        self.environment = values.into_iter().collect();
        self
    }

    /// Add ordered dotted-key assignments; later assignments take precedence.
    pub fn overrides(mut self, values: impl IntoIterator<Item = String>) -> Self {
        self.overrides.extend(values);
        self
    }

    /// Set a document input limit for direct Loader consumers.
    pub fn limits(mut self, limits: LoadLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Read at most the configured byte limit plus one and then merge layers.
    pub fn load(self) -> Result<Document, Report> {
        let text = if let Some(path) = &self.file {
            match File::open(path) {
                Ok(file) => {
                    let limit = self.limits.max_document_bytes;
                    let maximum = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
                    let mut bytes = Vec::new();
                    file.take(maximum)
                        .read_to_end(&mut bytes)
                        .map_err(|error| Report::one("", Source::File(path.clone()), error))?;
                    if bytes.len() > limit {
                        return Err(Report::too_large(Source::File(path.clone()), limit));
                    }
                    Some(
                        String::from_utf8(bytes)
                            .map_err(|error| Report::one("", Source::File(path.clone()), error))?,
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(Report::one("", Source::File(path.clone()), error)),
            }
        } else {
            None
        };
        self.load_text(text.as_deref())
    }

    /// Merge a supplied document in place of reading the configured file.
    pub fn load_text(mut self, text: Option<&str>) -> Result<Document, Report> {
        let mut document = Document {
            value: self.defaults.clone(),
            defaults: self.defaults,
            sources: BTreeMap::new(),
            text: BTreeMap::new(),
        };
        record_sources(&document.value, "", &Source::Default, &mut document.sources);
        if let Some(text) = text {
            let source = Source::File(self.file.unwrap_or_else(|| PathBuf::from("<text>")));
            if text.len() > self.limits.max_document_bytes {
                return Err(Report::too_large(source, self.limits.max_document_bytes));
            }
            let parsed: Value =
                toml::from_str(text).map_err(|error| Report::one("", source.clone(), error))?;
            merge_file(
                &mut document.value,
                parsed,
                "",
                &source,
                &mut document.sources,
            );
        }
        self.environment.sort_by(|left, right| left.0.cmp(&right.0));
        let marker = format!("{}_", self.prefix);
        // The prefixed filter has higher precedence than the conventional input.
        if let Some((_, value)) = self.environment.iter().find(|(name, _)| name == "RUST_LOG") {
            assign(
                &mut document,
                "log.filter",
                value,
                Source::Environment("RUST_LOG".into()),
            )?;
        }
        if !self.prefix.is_empty() {
            for (name, value) in &self.environment {
                if let Some(suffix) = name.strip_prefix(&marker) {
                    let key = suffix.to_ascii_lowercase().replace("__", ".");
                    assign(
                        &mut document,
                        &key,
                        value,
                        Source::Environment(name.clone()),
                    )?;
                }
            }
        }
        for assignment in self.overrides {
            let (key, value) = assignment.split_once('=').ok_or_else(|| {
                Report::one(
                    "",
                    Source::Override(assignment.clone()),
                    "expected key=value",
                )
            })?;
            assign(
                &mut document,
                key.trim(),
                value.trim(),
                Source::Override(key.trim().into()),
            )?;
        }
        Ok(document)
    }
}

fn empty_table() -> Value {
    Value::Table(Default::default())
}

fn dotted(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.into()
    } else if child.is_empty() {
        parent.into()
    } else {
        format!("{parent}.{child}")
    }
}

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |value, part| value.as_table()?.get(part))
}

fn clear_branch<T>(entries: &mut BTreeMap<String, T>, key: &str) {
    let prefix = format!("{key}.");
    entries.retain(|path, _| path != key && !path.starts_with(&prefix));
}

fn record_sources(
    value: &Value,
    path: &str,
    source: &Source,
    sources: &mut BTreeMap<String, Source>,
) {
    if !path.is_empty() {
        sources.insert(path.into(), source.clone());
    }
    if let Value::Table(table) = value {
        for (key, value) in table {
            record_sources(value, &dotted(path, key), source, sources);
        }
    }
}

fn merge_file(
    target: &mut Value,
    incoming: Value,
    path: &str,
    source: &Source,
    sources: &mut BTreeMap<String, Source>,
) {
    if let (Value::Table(target), Value::Table(incoming)) = (&mut *target, &incoming) {
        for (key, value) in incoming {
            merge_file(
                target.entry(key.clone()).or_insert_with(empty_table),
                value.clone(),
                &dotted(path, key),
                source,
                sources,
            );
        }
    } else {
        clear_branch(sources, path);
        *target = incoming;
        record_sources(target, path, source, sources);
    }
}

fn assign(document: &mut Document, path: &str, text: &str, source: Source) -> Result<(), Report> {
    let parts: Vec<_> = path.split('.').collect();
    if parts.iter().any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }) {
        return Err(Report::one(
            path,
            source,
            "expected a nonempty dotted field path",
        ));
    }
    let parsed = toml::from_str::<Value>(&format!("value = {text}"))
        .ok()
        .and_then(|mut table| {
            let table = table.as_table_mut()?;
            (table.len() == 1).then(|| table.remove("value")).flatten()
        })
        .unwrap_or_else(|| Value::String(text.into()));
    let mut target = &mut document.value;
    for (index, part) in parts.iter().enumerate() {
        if !target.is_table() {
            *target = empty_table();
            let ancestor = parts[..index].join(".");
            clear_branch(&mut document.text, &ancestor);
            clear_branch(&mut document.sources, &ancestor);
            document.sources.insert(ancestor, source.clone());
        }
        let table = target.as_table_mut().expect("table set above");
        if index + 1 == parts.len() {
            table.insert((*part).into(), parsed.clone());
            break;
        }
        target = table.entry(*part).or_insert_with(empty_table);
    }
    clear_branch(&mut document.text, path);
    clear_branch(&mut document.sources, path);
    document.text.insert(path.into(), text.into());
    record_sources(&parsed, path, &source, &mut document.sources);
    Ok(())
}

fn list_leaves(value: &Value, path: &str, output: &mut impl FnMut(&str, &Value)) {
    match value {
        Value::Table(table) if !table.is_empty() => {
            for (key, value) in table {
                list_leaves(value, &dotted(path, key), output);
            }
        }
        _ => output(path, value),
    }
}

fn substitute(value: &mut Value, defaults: &Value, path: &str) -> bool {
    // One invalid element invalidates its containing collection; recovering the
    // whole field still permits diagnostics for independent sibling fields.
    let path = path.split('[').next().unwrap_or(path);
    let (parent, field) = path.rsplit_once('.').unwrap_or(("", path));
    if field.is_empty() || field.contains(['[', ']']) {
        return false;
    }
    let mut target = value;
    if !parent.is_empty() {
        for part in parent.split('.') {
            let Some(child) = target.as_table_mut().and_then(|table| table.get_mut(part)) else {
                return false;
            };
            target = child;
        }
    }
    let Some(table) = target.as_table_mut() else {
        return false;
    };
    if !table.contains_key(field) {
        return false;
    }
    if let Some(default) = lookup(defaults, path) {
        table.insert(field.into(), default.clone());
    } else {
        table.remove(field);
    }
    true
}

/// Result of a host's configuration reload decision.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReloadReport {
    /// The currently published revision.
    pub generation: u64,
    /// Names of fields now exposed through reader snapshots.
    pub published: Vec<String>,
    /// Names of fields requiring a new runtime generation.
    pub requires_restart: Vec<String>,
}

/// One immutable value and its matching revision.
#[derive(Debug)]
pub struct Snapshot<T> {
    /// Monotonic revision within this store, starting at zero.
    pub generation: u64,
    /// Immutable shared configuration value.
    pub value: Arc<T>,
}

impl<T> Clone for Snapshot<T> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            value: self.value.clone(),
        }
    }
}

/// The single publisher of a configuration value.
pub struct Store<T> {
    state: Arc<RwLock<Snapshot<T>>>,
    notify: watch::Sender<u64>,
}

/// Cloneable read access; dropping the store closes update notifications.
pub struct Handle<T> {
    state: Arc<RwLock<Snapshot<T>>>,
    notify: watch::Receiver<u64>,
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            notify: self.notify.clone(),
        }
    }
}

impl<T> Store<T> {
    /// Create the initial, revision-zero configuration.
    pub fn new(value: T) -> Self {
        let (notify, _) = watch::channel(0);
        Self {
            state: Arc::new(RwLock::new(Snapshot {
                generation: 0,
                value: Arc::new(value),
            })),
            notify,
        }
    }

    /// Create a reader without exposing publication authority.
    pub fn handle(&self) -> Handle<T> {
        Handle {
            state: self.state.clone(),
            notify: self.notify.subscribe(),
        }
    }

    /// Publish a validated effective value; overflow leaves the store unchanged.
    pub fn publish(&mut self, effective: T) -> Result<u64, Error> {
        self.publish_guarded(effective, ())
    }

    /// Publish while owning a caller-provided admission guard.
    ///
    /// The snapshot changes before releasing `guard`. Notification and release
    /// of the old value happen only after releasing that guard and the store lock.
    pub fn publish_guarded<G>(&mut self, effective: T, guard: G) -> Result<u64, Error> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(generation) = state.generation.checked_add(1) else {
            drop(state);
            drop(guard);
            drop(effective);
            return Err(Error::new(
                kinds::INTERNAL,
                "configuration generation exhausted",
            ));
        };
        let previous = std::mem::replace(
            &mut *state,
            Snapshot {
                generation,
                value: Arc::new(effective),
            },
        );
        drop(state);
        drop(guard);
        self.notify.send_replace(generation);
        drop(previous);
        Ok(generation)
    }
}

impl<T> Handle<T> {
    /// Read one consistent value/revision pair without consuming a notification.
    pub fn snapshot(&self) -> Snapshot<T> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Read the most recently published value.
    pub fn current(&self) -> Arc<T> {
        self.snapshot().value
    }

    /// Read the most recently published revision; use `snapshot` when pairing it.
    pub fn generation(&self) -> u64 {
        self.snapshot().generation
    }

    /// Wait for a publication, or return `None` after the last unseen publication
    /// has been delivered and the writer has closed.
    pub async fn changed(&mut self) -> Option<Arc<T>> {
        self.notify.changed().await.ok()?;
        Some(self.current())
    }
}

/// Resolve a configuration path against an explicit root without changing cwd.
pub fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        root.join(path)
    }
}
