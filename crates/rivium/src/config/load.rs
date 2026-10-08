//! Loading: the layers in order of priority, then deserialization that reports every problem.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use toml::Value;

use super::tree::{Node, Seg, parse, show};
use super::value::{Expected, NodeDe};
use super::{Problem, Report, Source};
use crate::lifecycle::LifecycleSettings;
use crate::log::LogSettings;

/// The top-level sections Rivium owns; every other top-level key belongs to the service.
const RESERVED: [&str; 2] = ["log", "lifecycle"];

/// Rounds of "report, put the default back, try again" before giving up.
const ROUNDS: usize = 64;

/// The sections Rivium owns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reserved {
    pub(crate) log: LogSettings,
    pub(crate) lifecycle: LifecycleSettings,
}

/// The configuration file layer.
#[derive(Clone, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the hosts read files; they are not written yet")
)]
pub(crate) enum FileLayer {
    /// No file, as for `default-config`.
    None,
    /// A file on disk. A missing file is a problem when it was named explicitly; otherwise the
    /// service runs with the defaults.
    Path { path: PathBuf, explicit: bool },
    /// Text that stands for the file at `path`: a candidate configuration being checked.
    Text { path: PathBuf, text: String },
}

/// What the configuration is loaded from, lowest priority first: defaults, the file, the
/// environment (`RUST_LOG`, then `<PREFIX>_<A>__<B>`), then `--set`.
#[derive(Clone, Debug)]
pub(crate) struct Inputs<'a> {
    /// The service name; the environment prefix is its upper snake case.
    pub(crate) name: &'a str,
    pub(crate) file: FileLayer,
    /// Environment variables; `None` for the embedded host, which reads none.
    pub(crate) env: Option<&'a [(OsString, OsString)]>,
    /// `--set` overrides as (key, value), in order.
    pub(crate) sets: &'a [(String, String)],
    /// Whether the overrides come from the embedded host rather than the command line.
    pub(crate) host: bool,
}

/// A loaded configuration.
#[derive(Clone, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the hosts read these; they are not written yet")
)]
pub(crate) struct Loaded<C> {
    pub(crate) config: C,
    pub(crate) reserved: Reserved,
    /// The file that was read, if any.
    pub(crate) file: Option<PathBuf>,
    /// The default file location, when no file was there.
    pub(crate) missing_file: Option<PathBuf>,
    /// How many keys the environment and `--set` set.
    pub(crate) overrides: usize,
    tree: Node,
}

/// The environment variable prefix of a service: `snmp-agent` → `SNMP_AGENT`.
pub(crate) fn env_prefix(name: &str) -> String {
    name.to_ascii_uppercase().replace('-', "_")
}

/// Loads the configuration of a service whose own settings are `C`.
///
/// # Errors
///
/// Every problem found: files that cannot be read or parsed, unknown keys, values of the wrong
/// type or out of range, and settings that do not fit together.
pub(crate) fn load<C: Serialize + DeserializeOwned + Default>(
    inputs: &Inputs<'_>,
) -> Result<Loaded<C>, Report> {
    let mut problems = Vec::new();
    let defaults = defaults::<C>(inputs.name, &mut problems);
    if !problems.is_empty() {
        return Err(Report::new(problems));
    }
    let mut tree = defaults.clone();
    let (file, missing_file) = file_layer(&inputs.file, &mut tree, &mut problems);
    let mut overrides = 0;
    let mut set = |tree: &mut Node, key: &str, value: String, source: Source| {
        let keys: Vec<&str> = key.split('.').collect();
        match keys.iter().any(|key| key.is_empty()) {
            true => problems.push(Problem::new(
                Some(key.to_string()),
                source,
                "not a valid key",
            )),
            false => {
                tree.set(&keys, Node::Text(value, source));
                overrides += 1;
            }
        }
    };
    let mut not_utf8 = Vec::new();
    for (key, name, value) in env_layer(inputs.name, inputs.env.unwrap_or_default()) {
        match value.into_string() {
            Ok(value) => set(&mut tree, &key, value, Source::Env(name)),
            Err(_) => not_utf8.push(Problem::new(Some(key), Source::Env(name), "not UTF-8 text")),
        }
    }
    for (key, value) in inputs.sets {
        let source = match inputs.host {
            true => Source::Host(key.clone()),
            false => Source::Cli(key.clone()),
        };
        set(&mut tree, key, value.clone(), source);
    }
    problems.extend(not_utf8);

    let config = part::<C>(&mut tree, &defaults, false, &mut problems);
    let reserved = part::<Reserved>(&mut tree, &defaults, true, &mut problems);
    for (key, reason) in reserved
        .as_ref()
        .map(|r| r.log.problems(inputs.name))
        .unwrap_or_default()
    {
        let source = tree.source_at(&parse(&key)).clone();
        problems.push(Problem::new(Some(key), source, reason));
    }
    match (config, reserved) {
        (Some(config), Some(reserved)) if problems.is_empty() => Ok(Loaded {
            config,
            reserved,
            file,
            missing_file,
            overrides,
            tree,
        }),
        _ => Err(Report::new(problems)),
    }
}

/// The default configuration of a service: its own defaults, then `[log]` and `[lifecycle]`,
/// with the `--set` overrides applied, as TOML.
///
/// # Errors
///
/// When an override is not valid.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the process host prints it; they are not written yet"
    )
)]
pub(crate) fn default_config<C: Serialize + DeserializeOwned + Default>(
    name: &str,
    sets: &[(String, String)],
) -> Result<String, Report> {
    let inputs = Inputs {
        name,
        file: FileLayer::None,
        env: None,
        sets,
        host: false,
    };
    let loaded = load::<C>(&inputs)?;
    let unwritable = |error: toml::ser::Error| {
        let reason = format!("cannot be written as TOML: {error}");
        Report::new(vec![Problem::new(None, Source::Default, reason)])
    };
    let service = toml::to_string(&loaded.config).map_err(unwritable)?;
    let reserved = toml::to_string(&loaded.reserved).map_err(unwritable)?;
    Ok(match service.is_empty() {
        true => reserved,
        false => format!("{service}\n{reserved}"),
    })
}

impl<C: Serialize> Loaded<C> {
    /// Every key in effect, one line each: `<key> = <value> (<source>)`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "check-config prints it; they are not written yet")
    )]
    pub(crate) fn listing(&self) -> Vec<String> {
        let mut table = toml::Table::try_from(&self.config).unwrap_or_default();
        table.extend(toml::Table::try_from(&self.reserved).unwrap_or_default());
        let mut lines = Vec::new();
        let mut path = Vec::new();
        list(&Value::Table(table), &mut path, &mut |path, value| {
            let source = self.tree.source_at(path);
            lines.push(format!("{} = {value} ({source})", show(path)));
        });
        lines
    }
}

fn list(value: &Value, path: &mut Vec<Seg>, line: &mut impl FnMut(&[Seg], &Value)) {
    match value {
        Value::Table(table) if !table.is_empty() => {
            for (key, value) in table {
                path.push(Seg::Key(key.clone()));
                list(value, path, line);
                path.pop();
            }
        }
        value => line(path, value),
    }
}

/// The defaults: the service's, then Rivium's sections, which the service must not define.
fn defaults<C: Serialize + Default>(name: &str, problems: &mut Vec<Problem>) -> Node {
    let mut table = match toml::Table::try_from(C::default()) {
        Ok(table) => table,
        Err(error) => {
            let reason = format!("the service's defaults are not a TOML table: {error}");
            problems.push(Problem::new(None, Source::Default, reason));
            toml::Table::new()
        }
    };
    for section in RESERVED
        .iter()
        .filter(|section| table.contains_key(**section))
    {
        let reason = "is a section Rivium owns; the service's configuration must not define it";
        problems.push(Problem::new(
            Some((*section).to_string()),
            Source::Default,
            reason,
        ));
    }
    let reserved = Reserved {
        log: LogSettings::new(name),
        lifecycle: LifecycleSettings::default(),
    };
    table.extend(toml::Table::try_from(reserved).unwrap_or_default());
    Node::from_toml(Value::Table(table), &Source::Default)
}

/// Lays the file over the tree; returns the file read and the default location left empty.
fn file_layer(
    layer: &FileLayer,
    tree: &mut Node,
    problems: &mut Vec<Problem>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let (path, text) = match layer {
        FileLayer::None => return (None, None),
        FileLayer::Text { path, text } => (path, Ok(text.clone())),
        FileLayer::Path { path, explicit } => match std::fs::read_to_string(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => {
                return (None, Some(path.clone()));
            }
            text => (path, text),
        },
    };
    let source = Source::File(path.clone());
    let parsed = text
        .map_err(|error| format!("cannot be read: {error}"))
        .and_then(|text| {
            toml::from_str::<toml::Table>(&text).map_err(|error| {
                let at = error.span().map_or(0, |span| span.start).min(text.len());
                let line = text[..at].matches('\n').count() + 1;
                let column = text[..at]
                    .rsplit('\n')
                    .next()
                    .map_or(0, |part| part.chars().count())
                    + 1;
                format!(
                    "line {line}, column {column}: {}",
                    error.message().trim_end()
                )
            })
        });
    match parsed {
        Ok(table) => tree.merge(Node::from_toml(Value::Table(table), &source)),
        Err(reason) => problems.push(Problem::new(None, source, reason)),
    }
    (Some(path.clone()), None)
}

/// The keys the environment sets, as (key, variable, value): `RUST_LOG` for `log.filter`, then
/// `<PREFIX>_<A>__<B>` for `a.b`, in the order of the names. Empty values count as unset;
/// prefixed names without `__`, such as `<PREFIX>_ROOT`, are not keys.
fn env_layer(name: &str, env: &[(OsString, OsString)]) -> Vec<(String, String, OsString)> {
    let start = format!("{}_", env_prefix(name));
    let set = env.iter().filter(|(_, value)| !value.is_empty());
    let mut keys: Vec<(String, String, OsString)> = set
        .filter_map(|(name, value)| {
            let name = name.to_str()?;
            let rest = name
                .strip_prefix(&start)
                .filter(|rest| rest.contains("__"))?;
            Some((
                rest.to_ascii_lowercase().replace("__", "."),
                name.to_string(),
                value.clone(),
            ))
        })
        .collect();
    keys.sort_by(|a, b| a.1.cmp(&b.1));
    let alias = env
        .iter()
        .find(|(name, value)| name == "RUST_LOG" && !value.is_empty());
    let alias = alias.map(|(_, value)| ("log.filter".into(), "RUST_LOG".into(), value.clone()));
    alias.into_iter().chain(keys).collect()
}

/// Deserializes one part of the tree: the service's keys, or Rivium's sections. Each error is
/// reported, its value is put back to the default (or removed when there is none), and the
/// part is read again, so every problem is found in one run.
fn part<T: DeserializeOwned>(
    tree: &mut Node,
    defaults: &Node,
    reserved: bool,
    problems: &mut Vec<Problem>,
) -> Option<T> {
    let mut seen = BTreeSet::new();
    for _ in 0..ROUNDS {
        let Node::Table(entries, source) = &*tree else {
            return None;
        };
        let entries = (entries.iter())
            .filter(|(key, _)| RESERVED.contains(&key.as_str()) == reserved)
            .map(|(key, node)| (key.clone(), node.clone()));
        let node = Node::Table(entries.collect(), source.clone());
        let expected = Expected::default();
        let mut ignored = Vec::new();
        let de = NodeDe {
            node,
            path: String::new(),
            expected: &expected,
        };
        let mut track = |path: serde_ignored::Path<'_>| ignored.push(ignored_path(&path));
        let de = serde_ignored::Deserializer::new(de, &mut track);
        match serde_path_to_error::deserialize(de) {
            Ok(value) => {
                let unknown = ignored
                    .iter()
                    .map(|path| unknown_key(tree, &expected, path));
                problems.extend(unknown.collect::<Vec<_>>());
                return Some(value);
            }
            Err(error) => {
                let mut path: Vec<Seg> = (error.path().iter())
                    .filter_map(|seg| match seg {
                        serde_path_to_error::Segment::Seq { index } => Some(Seg::Index(*index)),
                        serde_path_to_error::Segment::Map { key } => Some(Seg::Key(key.clone())),
                        _ => None,
                    })
                    .collect();
                let reason = error.into_inner().to_string();
                let missing = reason
                    .strip_prefix("missing field `")
                    .and_then(|r| r.strip_suffix('`'));
                path.extend(missing.map(|field| Seg::Key(field.to_string())));
                // A key that fails again was removed for want of a default and left its parent
                // incomplete: recover the parent instead, without reporting the key twice.
                let key = show(&path);
                let again = !seen.insert(key.clone());
                if !again {
                    problems.push(Problem::new(
                        Some(key),
                        tree.source_at(&path).clone(),
                        reason,
                    ));
                }
                let at = &path[..path.len() - usize::from(again && !path.is_empty())];
                if !recover(tree, defaults, at) {
                    return None;
                }
            }
        }
    }
    None
}

/// Puts the default back at `path`, or removes the value there when it has none. A path that
/// ends inside text or below a missing key recovers the deepest value on it instead.
fn recover(tree: &mut Node, defaults: &Node, path: &[Seg]) -> bool {
    if let Some(default) = defaults.get(path) {
        return tree.replace(path, Some(default.clone()));
    }
    let at = &path[..tree.depth(path)];
    !at.is_empty() && tree.replace(at, defaults.get(at).cloned())
}

fn ignored_path(path: &serde_ignored::Path<'_>) -> Vec<Seg> {
    use serde_ignored::Path;
    match path {
        Path::Root => Vec::new(),
        Path::Seq { parent, index } => [ignored_path(parent), vec![Seg::Index(*index)]].concat(),
        Path::Map { parent, key } => [ignored_path(parent), vec![Seg::Key(key.clone())]].concat(),
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => ignored_path(parent),
    }
}

/// An unknown key, with the closest expected field when one is close enough to be a typo.
fn unknown_key(tree: &Node, expected: &Expected, path: &[Seg]) -> Problem {
    let parent = show(&path[..path.len().saturating_sub(1)]);
    let close = match (path.last(), expected.borrow().get(&parent)) {
        (Some(Seg::Key(key)), Some(fields)) => (fields.iter())
            .map(|field| (distance(key, field), *field))
            .filter(|(distance, _)| *distance <= (key.chars().count() / 3).max(1))
            .min(),
        _ => None,
    };
    let reason = match close {
        Some((_, field)) => format!("unknown key; did you mean `{field}`?"),
        None => "unknown key".to_string(),
    };
    Problem::new(Some(show(path)), tree.source_at(path).clone(), reason)
}

/// The edit distance between two keys.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, a) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(a != *b))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}
