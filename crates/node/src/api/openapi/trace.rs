//! Which structs a type reads from a JSON value, and where: a deserializer over a
//! `serde_json::Value` that answers as `Value` does and notes the field names each
//! `deserialize_struct` (or struct variant) asks for, with the path of the object it reads
//! them from. The checks compare those names with the shape's at the same path, at any depth.
//!
//! A struct read through a buffer (an internally tagged or untagged enum, or one flattened
//! into another) asks for its fields of the buffer, not of this deserializer, and is not
//! noted.

use std::cell::RefCell;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde_json::{Error, Map, Value};

/// One step from a value to a value inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// A property of an object.
    Key(String),
    /// An item of a list.
    Item(usize),
}

/// A struct the type read: where, and the field names it asked for.
#[derive(Clone, Debug)]
pub(super) struct Read {
    pub path: Vec<Step>,
    pub fields: &'static [&'static str],
}

/// The structs `T` reads from `v`, outermost first; an error when `T` does not read `v`.
pub(super) fn structs<T: de::DeserializeOwned>(v: &Value) -> Result<Vec<Read>, String> {
    let log = RefCell::new(Vec::new());
    let _t: T = T::deserialize(Rec {
        value: v.clone(),
        path: Vec::new(),
        log: &log,
    })
    .map_err(|e| e.to_string())?;
    Ok(log.into_inner())
}

/// Shows a path as `.a[0].b`.
pub(super) fn show(path: &[Step]) -> String {
    let mut out = String::new();
    for step in path {
        match step {
            Step::Key(k) => {
                out.push('.');
                out.push_str(k);
            }
            Step::Item(i) => out.push_str(&format!("[{i}]")),
        }
    }
    if out.is_empty() {
        out.push('.');
    }
    out
}

struct Rec<'a> {
    value: Value,
    path: Vec<Step>,
    log: &'a RefCell<Vec<Read>>,
}

impl Rec<'_> {
    fn compound<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.value {
            Value::Object(map) => visitor.visit_map(Props {
                rest: map.into_iter(),
                next: None,
                path: self.path,
                log: self.log,
            }),
            Value::Array(items) => visitor.visit_seq(Items {
                rest: items.into_iter().enumerate(),
                path: self.path,
                log: self.log,
            }),
            other => other.deserialize_any(visitor),
        }
    }
}

macro_rules! as_value {
    ($($method:ident)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
                self.value.$method(visitor)
            }
        )*
    };
}

impl<'de> Deserializer<'de> for Rec<'_> {
    type Error = Error;

    as_value! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64
        deserialize_i128 deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64
        deserialize_u128 deserialize_f32 deserialize_f64 deserialize_char deserialize_str
        deserialize_string deserialize_bytes deserialize_byte_buf deserialize_unit
        deserialize_identifier deserialize_ignored_any
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.compound(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.value.is_null() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.value.deserialize_unit_struct(name, visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.value {
            Value::Array(_) => self.compound(visitor),
            other => other.deserialize_seq(visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.value {
            Value::Object(_) => self.compound(visitor),
            other => other.deserialize_map(visitor),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.log.borrow_mut().push(Read {
            path: self.path.clone(),
            fields,
        });
        match self.value {
            Value::Object(_) | Value::Array(_) => self.compound(visitor),
            other => other.deserialize_struct(name, fields, visitor),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.value {
            Value::Object(map) if map.len() == 1 => {
                let (variant, value) = map.into_iter().next().expect("one entry");
                let mut path = self.path;
                path.push(Step::Key(variant.clone()));
                visitor.visit_enum(Variant {
                    variant,
                    value: Rec {
                        value,
                        path,
                        log: self.log,
                    },
                })
            }
            other => other.deserialize_enum(name, variants, visitor),
        }
    }
}

struct Props<'a> {
    rest: <Map<String, Value> as IntoIterator>::IntoIter,
    next: Option<(String, Value)>,
    path: Vec<Step>,
    log: &'a RefCell<Vec<Read>>,
}

impl<'de> MapAccess<'de> for Props<'_> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        let Some((k, v)) = self.rest.next() else {
            return Ok(None);
        };
        let key = seed.deserialize(Value::String(k.clone()))?;
        self.next = Some((k, v));
        Ok(Some(key))
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, Error> {
        let (k, v) = self
            .next
            .take()
            .ok_or_else(|| de::Error::custom("a value before its key"))?;
        let mut path = self.path.clone();
        path.push(Step::Key(k));
        seed.deserialize(Rec {
            value: v,
            path,
            log: self.log,
        })
    }
}

struct Items<'a> {
    rest: std::iter::Enumerate<std::vec::IntoIter<Value>>,
    path: Vec<Step>,
    log: &'a RefCell<Vec<Read>>,
}

impl<'de> SeqAccess<'de> for Items<'_> {
    type Error = Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Error> {
        let Some((i, v)) = self.rest.next() else {
            return Ok(None);
        };
        let mut path = self.path.clone();
        path.push(Step::Item(i));
        seed.deserialize(Rec {
            value: v,
            path,
            log: self.log,
        })
        .map(Some)
    }
}

struct Variant<'a> {
    variant: String,
    value: Rec<'a>,
}

impl<'de, 'a> EnumAccess<'de> for Variant<'a> {
    type Error = Error;
    type Variant = Rec<'a>;

    fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Rec<'a>), Error> {
        let variant: de::value::StringDeserializer<Error> = self.variant.into_deserializer();
        Ok((seed.deserialize(variant)?, self.value))
    }
}

impl<'de> VariantAccess<'de> for Rec<'_> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        de::Deserialize::deserialize(self)
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_struct("", fields, visitor)
    }
}
