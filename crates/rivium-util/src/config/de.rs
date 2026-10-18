//! Serde adapter keeping text assignments typed by their destination fields.

use serde::{
    Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use std::collections::BTreeMap;
use toml::Value;

pub(super) struct Input<'a> {
    pub(super) value: Value,
    pub(super) path: String,
    pub(super) text: &'a BTreeMap<String, String>,
}

macro_rules! typed_text {
    ($($method:ident => $visit:ident ($value:ty)),+ $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                if let Some(raw) = self.text.get(&self.path) {
                    let parsed = raw.parse::<$value>().map_err(de::Error::custom)?;
                    visitor.$visit(parsed)
                } else {
                    self.value.$method(visitor)
                }
            }
        )+
    };
}

impl<'de> Deserializer<'de> for Input<'_> {
    type Error = toml::de::Error;

    typed_text! {
        deserialize_bool => visit_bool(bool),
        deserialize_i8 => visit_i8(i8),
        deserialize_i16 => visit_i16(i16),
        deserialize_i32 => visit_i32(i32),
        deserialize_i64 => visit_i64(i64),
        deserialize_i128 => visit_i128(i128),
        deserialize_u8 => visit_u8(u8),
        deserialize_u16 => visit_u16(u16),
        deserialize_u32 => visit_u32(u32),
        deserialize_u64 => visit_u64(u64),
        deserialize_u128 => visit_u128(u128),
        deserialize_f32 => visit_f32(f32),
        deserialize_f64 => visit_f64(f64),
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::Table(table) => visitor.visit_map(Fields {
                values: table.into_iter(),
                pending: None,
                parent: self.path,
                text: self.text,
            }),
            Value::Array(values) => visitor.visit_seq(Items {
                values: values.into_iter().enumerate(),
                parent: self.path,
                text: self.text,
            }),
            value => value.deserialize_any(visitor),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if !self.value.is_str()
            && let Some(text) = self.text.get(&self.path)
        {
            return visitor.visit_string(text.clone());
        }
        self.value.deserialize_str(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.value.deserialize_enum(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        char bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct Fields<'a> {
    values: toml::map::IntoIter<String, Value>,
    pending: Option<(String, Value)>,
    parent: String,
    text: &'a BTreeMap<String, String>,
}

impl<'de> MapAccess<'de> for Fields<'_> {
    type Error = toml::de::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.values.next() else {
            return Ok(None);
        };
        self.pending = Some((key.clone(), value));
        seed.deserialize(de::value::StringDeserializer::<Self::Error>::new(key))
            .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let Some((key, value)) = self.pending.take() else {
            return Err(de::Error::custom("value requested without a field"));
        };
        seed.deserialize(Input {
            value,
            path: super::dotted(&self.parent, &key),
            text: self.text,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct Items<'a> {
    values: std::iter::Enumerate<std::vec::IntoIter<Value>>,
    parent: String,
    text: &'a BTreeMap<String, String>,
}

impl<'de> SeqAccess<'de> for Items<'_> {
    type Error = toml::de::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        match self.values.next() {
            Some((index, value)) => seed
                .deserialize(Input {
                    value,
                    path: format!("{}[{index}]", self.parent),
                    text: self.text,
                })
                .map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}
