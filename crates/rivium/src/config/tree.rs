//! The merged configuration tree: every value with its source.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use toml::Value;

use super::Source;

/// One step of a key: a table key or an array index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Seg {
    Key(String),
    Index(usize),
}

/// A key as shown to people, such as `log.files[0].name`; keys that are not bare are quoted.
pub(super) fn show(path: &[Seg]) -> String {
    let mut shown = String::new();
    for seg in path {
        push(&mut shown, seg);
    }
    shown
}

pub(super) fn push(shown: &mut String, seg: &Seg) {
    match seg {
        Seg::Index(index) => drop(write!(shown, "[{index}]")),
        Seg::Key(key) => {
            if !shown.is_empty() {
                shown.push('.');
            }
            let bare = !key.is_empty()
                && (key.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            match bare {
                true => shown.push_str(key),
                false => drop(write!(shown, "{key:?}")),
            }
        }
    }
}

/// The steps of a key that [`show`] wrote without quotes, such as `log.files[0].name`.
pub(super) fn parse(key: &str) -> Vec<Seg> {
    let mut path = Vec::new();
    for part in key.split('.') {
        let mut pieces = part.split('[');
        path.extend(pieces.next().map(|name| Seg::Key(name.to_string())));
        let indexes = pieces.filter_map(|index| index.strip_suffix(']')?.parse().ok());
        path.extend(indexes.map(Seg::Index));
    }
    path
}

/// A value and where it came from. Text from environment variables and `--set` keeps its type
/// open until the field it lands in asks for one.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Table(BTreeMap<String, Node>, Source),
    Array(Vec<Node>, Source),
    /// Any other TOML value.
    Value(Value, Source),
    Text(String, Source),
}

impl Node {
    pub(super) fn from_toml(value: Value, source: &Source) -> Node {
        match value {
            Value::Table(table) => {
                let entries = table
                    .into_iter()
                    .map(|(k, v)| (k, Node::from_toml(v, source)));
                Node::Table(entries.collect(), source.clone())
            }
            Value::Array(items) => {
                let items = items.into_iter().map(|v| Node::from_toml(v, source));
                Node::Array(items.collect(), source.clone())
            }
            value => Node::Value(value, source.clone()),
        }
    }

    /// The node as a TOML value; text becomes a string.
    pub(super) fn into_toml(self) -> Value {
        match self {
            Node::Table(entries, _) => Value::Table(
                entries
                    .into_iter()
                    .map(|(k, n)| (k, n.into_toml()))
                    .collect(),
            ),
            Node::Array(items, _) => Value::Array(items.into_iter().map(Node::into_toml).collect()),
            Node::Value(value, _) => value,
            Node::Text(text, _) => Value::String(text),
        }
    }

    pub(super) fn source(&self) -> &Source {
        match self {
            Node::Table(_, source)
            | Node::Array(_, source)
            | Node::Value(_, source)
            | Node::Text(_, source) => source,
        }
    }

    /// Lays `other` over this node: tables merge key by key, anything else is replaced.
    pub(super) fn merge(&mut self, other: Node) {
        match (self, other) {
            (Node::Table(mine, _), Node::Table(theirs, _)) => {
                for (key, node) in theirs {
                    match mine.get_mut(&key) {
                        Some(slot) => slot.merge(node),
                        None => drop(mine.insert(key, node)),
                    }
                }
            }
            (slot, other) => *slot = other,
        }
    }

    /// Sets the value of a dotted key, making tables on the way of anything that is not one.
    pub(super) fn set(&mut self, keys: &[&str], leaf: Node) {
        let Some((first, rest)) = keys.split_first() else {
            return *self = leaf;
        };
        if !matches!(self, Node::Table(..)) {
            *self = Node::Table(BTreeMap::new(), leaf.source().clone());
        }
        if let Node::Table(entries, _) = self {
            let empty = || Node::Table(BTreeMap::new(), leaf.source().clone());
            entries
                .entry((*first).to_string())
                .or_insert_with(empty)
                .set(rest, leaf);
        }
    }

    /// The node at `path`, if there is one.
    pub(super) fn get(&self, path: &[Seg]) -> Option<&Node> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        match (self, first) {
            (Node::Table(entries, _), Seg::Key(key)) => entries.get(key)?.get(rest),
            (Node::Array(items, _), Seg::Index(index)) => items.get(*index)?.get(rest),
            _ => None,
        }
    }

    /// How many steps of `path` lead to a node.
    pub(super) fn depth(&self, path: &[Seg]) -> usize {
        (0..=path.len())
            .rev()
            .find(|depth| self.get(&path[..*depth]).is_some())
            .unwrap_or(0)
    }

    /// The source of the deepest node on `path`.
    pub(super) fn source_at(&self, path: &[Seg]) -> &Source {
        let depth = self.depth(path);
        self.get(&path[..depth]).unwrap_or(self).source()
    }

    /// Puts `with` at `path`, or removes the node there when `with` is `None`; returns whether
    /// the tree changed. The root cannot be removed.
    pub(super) fn replace(&mut self, path: &[Seg], with: Option<Node>) -> bool {
        let Some((last, parent)) = path.split_last() else {
            let Some(with) = with else { return false };
            let changed = *self != with;
            *self = with;
            return changed;
        };
        let Some(parent) = self.get_mut(parent) else {
            return false;
        };
        match (parent, last, with) {
            (Node::Table(entries, _), Seg::Key(key), Some(with)) => {
                entries.insert(key.clone(), with.clone()).as_ref() != Some(&with)
            }
            (Node::Table(entries, _), Seg::Key(key), None) => entries.remove(key).is_some(),
            (Node::Array(items, _), Seg::Index(index), with) if *index < items.len() => {
                match with {
                    Some(with) => items[*index] = with,
                    None => drop(items.remove(*index)),
                }
                true
            }
            _ => false,
        }
    }

    fn get_mut(&mut self, path: &[Seg]) -> Option<&mut Node> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        match (self, first) {
            (Node::Table(entries, _), Seg::Key(key)) => entries.get_mut(key)?.get_mut(rest),
            (Node::Array(items, _), Seg::Index(index)) => items.get_mut(*index)?.get_mut(rest),
            _ => None,
        }
    }
}
