//! Deserializing the merged tree: TOML values as they are, text as the field it lands in asks.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::de::value::Error;
use serde::de::{self, DeserializeSeed, Deserializer as _, Error as _, IntoDeserializer, Visitor};
use toml::Value;

use super::tree::{Node, Seg, push};

/// The fields each struct expects, by key: for "did you mean" hints on unknown keys.
pub(super) type Expected = RefCell<BTreeMap<String, &'static [&'static str]>>;

/// The type a field asks for, which decides how text is read.
#[derive(Clone, Copy)]
enum Hint {
    Bool,
    Int,
    Float,
    Text,
    Inline,
}

/// A node being deserialized, with its key.
pub(super) struct NodeDe<'e> {
    pub(super) node: Node,
    pub(super) path: String,
    pub(super) expected: &'e Expected,
}

fn toml(error: toml::de::Error) -> Error {
    Error::custom(error.message())
}

impl NodeDe<'_> {
    /// The node as a TOML value, with text read as `hint` says. Text that is not of the type
    /// asked for stays a string, so the field reports it as `invalid type: string "…"`.
    fn value(self, hint: Hint) -> Result<Value, Error> {
        let Node::Text(text, _) = self.node else {
            return Ok(self.node.into_toml());
        };
        Ok(match hint {
            Hint::Bool if text == "true" || text == "false" => Value::Boolean(text == "true"),
            Hint::Int => text.parse().map_or(Value::String(text), Value::Integer),
            Hint::Float => text.parse().map_or(Value::String(text), Value::Float),
            Hint::Inline => inline(&text)?,
            Hint::Bool | Hint::Text => Value::String(text),
        })
    }

    /// Visits a table or an array element by element, so text inside keeps its open type;
    /// anything else is read as a TOML value written as text.
    fn visit<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let (path, expected) = (self.path, self.expected);
        let (table, entries): (bool, Vec<(Seg, Node)>) = match self.node {
            Node::Table(entries, _) => (
                true,
                entries.into_iter().map(|(k, n)| (Seg::Key(k), n)).collect(),
            ),
            Node::Array(items, _) => (false, (0..).map(Seg::Index).zip(items).collect()),
            node => {
                let value = NodeDe {
                    node,
                    path,
                    expected,
                }
                .value(Hint::Inline)?;
                return value.deserialize_any(visitor).map_err(toml);
            }
        };
        let access = Access {
            path,
            expected,
            entries: entries.into_iter(),
            value: None,
        };
        match table {
            true => visitor.visit_map(access),
            false => visitor.visit_seq(access),
        }
    }
}

/// Text written as a TOML value, such as `["a", "b"]` or `{ a = 1 }`.
fn inline(text: &str) -> Result<Value, Error> {
    let table: Result<toml::Table, _> = toml::from_str(&format!("value = {text}"));
    match table.map(|mut table| table.remove("value")) {
        Ok(Some(value)) => Ok(value),
        _ => Err(Error::custom(format!(
            "expected a TOML value such as [\"a\"] or {{ a = 1 }}, got {text:?}"
        ))),
    }
}

macro_rules! read_as {
    ($($hint:ident: $($method:ident)*;)*) => {$($(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            self.value(Hint::$hint)?.deserialize_any(visitor).map_err(toml)
        }
    )*)*};
}

macro_rules! visit {
    ($($method:ident($($arg:ty),*);)*) => {$(
        fn $method<V: Visitor<'de>>(self, $(_: $arg,)* visitor: V) -> Result<V::Value, Error> {
            self.visit(visitor)
        }
    )*};
}

impl<'de> de::Deserializer<'de> for NodeDe<'_> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.node {
            Node::Table(..) | Node::Array(..) => self.visit(visitor),
            _ => self
                .value(Hint::Text)?
                .deserialize_any(visitor)
                .map_err(toml),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        if !matches!(self.node, Node::Table(..)) {
            let value = self.value(Hint::Inline)?;
            return value
                .deserialize_struct(name, fields, visitor)
                .map_err(toml);
        }
        self.expected.borrow_mut().insert(self.path.clone(), fields);
        self.visit(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let hint = match &self.node {
            Node::Text(text, _) if text.trim_start().starts_with('{') => Hint::Inline,
            _ => Hint::Text,
        };
        let value = self.value(hint)?;
        value
            .deserialize_enum(name, variants, visitor)
            .map_err(toml)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_unit(visitor)
    }

    visit! {
        deserialize_map();
        deserialize_seq();
        deserialize_tuple(usize);
        deserialize_tuple_struct(&'static str, usize);
    }

    read_as! {
        Bool: deserialize_bool;
        Int: deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64
            deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64;
        Float: deserialize_f32 deserialize_f64;
        Text: deserialize_char deserialize_str deserialize_string deserialize_bytes
            deserialize_byte_buf deserialize_unit deserialize_identifier;
    }
}

/// The entries of a table, or the elements of an array, being visited.
struct Access<'e> {
    path: String,
    expected: &'e Expected,
    entries: std::vec::IntoIter<(Seg, Node)>,
    value: Option<NodeDe<'e>>,
}

impl<'e> Access<'e> {
    /// The next entry: its key, and the node with its own key.
    fn next(&mut self) -> Option<(Seg, NodeDe<'e>)> {
        let (seg, node) = self.entries.next()?;
        let mut path = self.path.clone();
        push(&mut path, &seg);
        Some((
            seg,
            NodeDe {
                node,
                path,
                expected: self.expected,
            },
        ))
    }
}

impl<'de> de::MapAccess<'de> for Access<'_> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        let Some((Seg::Key(key), value)) = self.next() else {
            return Ok(None);
        };
        self.value = Some(value);
        seed.deserialize(key.into_deserializer()).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(
            self.value
                .take()
                .ok_or_else(|| Error::custom("a value without a key"))?,
        )
    }
}

impl<'de> de::SeqAccess<'de> for Access<'_> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        self.next()
            .map(|(_, element)| seed.deserialize(element))
            .transpose()
    }
}
