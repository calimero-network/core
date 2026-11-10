//! Records and map values as the guest sends and gets them: JSON text, converted to and
//! from the canonical CBOR the host stores by the ABI type each record field or map value
//! declares (`state.collections`: a table's `fields`, a map's `value`).
//!
//! | ABI type | JSON in | stored | JSON out |
//! | --- | --- | --- | --- |
//! | `bool` | bool | bool | bool |
//! | `u8`..`u32`, `i8`..`i32` | integer number | integer | number |
//! | `u64`, `i64` | decimal string or integer number | integer | decimal string |
//! | `f32`, `f64` | number | float (`f32` rounded to single precision) | number |
//! | `string` | string | text | string |
//! | `bytes` | base64url (unpadded) string | byte string | base64url string |
//! | `unit` | null | null | null |
//! | `BlobRef` | 64 lowercase hex digits | tag `BLOB_REF_TAG` around 32 bytes | hex |
//! | `option<T>` | null or `T` | null or `T` | null or `T` |
//! | `list<T>`, `tuple<A, ..>` | array | array | array |
//! | any other name | any JSON, numbers as integers when exact | its shape | its shape |
//!
//! `bytes` and `BlobRef` are how the Rust SDK's records and `BlobRef` store themselves
//! (a byte string, a tag); the spec's table names neither.

use std::collections::BTreeMap;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ciborium::value::Integer;
use ciborium::Value;
use serde_json::{Map, Number, Value as Json};

use crate::blobs::{blob_id_hex, parse_blob_id, BLOB_REF_TAG};
use crate::core::MAX_RUN_BYTES;

/// The deepest nesting of a guest record or map value (arrays and objects), checked on the
/// text before it is parsed.
pub const MAX_JSON_DEPTH: usize = 64;
/// The most JSON items (array elements and object members) a guest record or map value may
/// hold, checked on the text before it is parsed: the parsed tree costs some 50 bytes of
/// host heap per item, so this caps one call's tree at a few MiB whatever the input.
pub const MAX_JSON_ITEMS: usize = 65_536;

/// The type of a record field or a map value, from its ABI name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    /// `u8`..`u64`, `i8`..`i64`.
    Int {
        signed: bool,
        bits: u32,
    },
    /// `f32` or `f64`.
    Float {
        bits: u32,
    },
    /// `string`.
    Text,
    /// `bytes`.
    Bytes,
    /// `unit`.
    Unit,
    /// `BlobRef`.
    BlobRef,
    Option(Box<ValueType>),
    List(Box<ValueType>),
    Tuple(Vec<ValueType>),
    /// A struct, an enum or any other name: converted by the JSON's own shape.
    Other(String),
}

impl ValueType {
    /// The type an ABI name writes (`types::abi_type` in `calimero-sdk-macros`); nested at
    /// most [`MAX_JSON_DEPTH`] deep.
    pub fn parse(name: &str) -> Result<Self, String> {
        Self::parse_at(name, 0)
    }

    fn parse_at(name: &str, depth: usize) -> Result<Self, String> {
        if depth > MAX_JSON_DEPTH {
            return Err(format!("a type nested deeper than {MAX_JSON_DEPTH}"));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err("an empty type name".into());
        }
        let generic = |head: &str| name.strip_prefix(head)?.strip_suffix('>');
        if let Some(inner) = generic("option<") {
            return Ok(Self::Option(Box::new(Self::parse_at(inner, depth + 1)?)));
        }
        if let Some(inner) = generic("list<") {
            return Ok(Self::List(Box::new(Self::parse_at(inner, depth + 1)?)));
        }
        if let Some(inner) = generic("tuple<") {
            return split_top(inner)
                .into_iter()
                .map(|part| Self::parse_at(part, depth + 1))
                .collect::<Result<_, _>>()
                .map(Self::Tuple);
        }
        let int = |signed, bits| Self::Int { signed, bits };
        Ok(match name {
            "bool" => Self::Bool,
            "u8" => int(false, 8),
            "u16" => int(false, 16),
            "u32" => int(false, 32),
            "u64" => int(false, 64),
            "i8" => int(true, 8),
            "i16" => int(true, 16),
            "i32" => int(true, 32),
            "i64" => int(true, 64),
            "f32" => Self::Float { bits: 32 },
            "f64" => Self::Float { bits: 64 },
            "string" => Self::Text,
            "bytes" => Self::Bytes,
            "unit" => Self::Unit,
            "BlobRef" => Self::BlobRef,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => f.write_str("bool"),
            Self::Int { signed, bits } => write!(f, "{}{bits}", if *signed { 'i' } else { 'u' }),
            Self::Float { bits } => write!(f, "f{bits}"),
            Self::Text => f.write_str("string"),
            Self::Bytes => f.write_str("bytes"),
            Self::Unit => f.write_str("unit"),
            Self::BlobRef => f.write_str("BlobRef"),
            Self::Option(inner) => write!(f, "option<{inner}>"),
            Self::List(inner) => write!(f, "list<{inner}>"),
            Self::Tuple(items) => {
                f.write_str("tuple<")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str(">")
            }
            Self::Other(name) => f.write_str(name),
        }
    }
}

/// `a, option<b>, tuple<c, d>` split at its top-level commas.
fn split_top(list: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in list.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&list[start..]);
    parts
}

// --- guest JSON in ------------------------------------------------------------------

/// The guest's JSON text, parsed: refused over [`MAX_RUN_BYTES`] (it could never be stored)
/// and, on the text before anything is parsed, nested past [`MAX_JSON_DEPTH`] or holding
/// more than [`MAX_JSON_ITEMS`] items, so a small input never becomes a large tree.
fn read_guest_json(text: &str) -> Result<Json, String> {
    if text.len() > MAX_RUN_BYTES {
        return Err(format!(
            "a value of {} bytes, over {MAX_RUN_BYTES}",
            text.len()
        ));
    }
    shape(text)?;
    serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))
}

/// Depth and item count of JSON text, read without allocating: brackets and commas
/// outside strings.
fn shape(text: &str) -> Result<(), String> {
    let (mut depth, mut items) = (0usize, 1usize);
    let (mut in_string, mut escaped) = (false, false);
    for b in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return Err(format!("JSON nested deeper than {MAX_JSON_DEPTH}"));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            b',' => {
                items += 1;
                if items > MAX_JSON_ITEMS {
                    return Err(format!("more than {MAX_JSON_ITEMS} JSON items"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A guest record: a JSON object with exactly the declared `fields`, each of its type, as a
/// CBOR map (not yet canonical).
pub(crate) fn record_in(fields: &BTreeMap<String, ValueType>, text: &str) -> Result<Value, String> {
    let Json::Object(object) = read_guest_json(text)? else {
        return Err("a record is a JSON object".into());
    };
    if let Some(extra) = object.keys().find(|k| !fields.contains_key(*k)) {
        return Err(format!("the record declares no field `{extra}`"));
    }
    let mut entries = Vec::with_capacity(fields.len());
    for (name, ty) in fields {
        let value = object
            .get(name)
            .ok_or_else(|| format!("field `{name}` is missing"))?;
        let value = to_cbor(ty, value).map_err(|e| format!("field `{name}`: {e}"))?;
        entries.push((Value::Text(name.clone()), value));
    }
    Ok(Value::Map(entries))
}

/// A guest map value of type `ty` as CBOR (not yet canonical).
pub(crate) fn value_in(ty: &ValueType, text: &str) -> Result<Value, String> {
    to_cbor(ty, &read_guest_json(text)?)
}

fn to_cbor(ty: &ValueType, json: &Json) -> Result<Value, String> {
    Ok(match (ty, json) {
        (ValueType::Bool, Json::Bool(b)) => Value::Bool(*b),
        (ValueType::Int { signed, bits }, _) => Value::Integer(int_in(*signed, *bits, json)?),
        (ValueType::Float { bits }, Json::Number(n)) => Value::Float(float_in(*bits, n)?),
        (ValueType::Text, Json::String(s)) => Value::Text(s.clone()),
        (ValueType::Bytes, Json::String(s)) => Value::Bytes(
            URL_SAFE_NO_PAD
                .decode(s)
                .map_err(|_| "bytes are base64url without padding".to_owned())?,
        ),
        (ValueType::Unit | ValueType::Option(_), Json::Null) => Value::Null,
        (ValueType::BlobRef, Json::String(s)) => {
            let id = parse_blob_id(s).ok_or("a BlobRef is 64 lowercase hex digits")?;
            Value::Tag(BLOB_REF_TAG, Box::new(Value::Bytes(id.to_vec())))
        }
        (ValueType::Option(inner), _) => to_cbor(inner, json)?,
        (ValueType::List(inner), Json::Array(items)) => Value::Array(
            items
                .iter()
                .map(|item| to_cbor(inner, item))
                .collect::<Result<_, _>>()?,
        ),
        (ValueType::Tuple(types), Json::Array(items)) if types.len() == items.len() => {
            Value::Array(
                types
                    .iter()
                    .zip(items)
                    .map(|(ty, item)| to_cbor(ty, item))
                    .collect::<Result<_, _>>()?,
            )
        }
        (ValueType::Other(_), _) => untyped_in(json)?,
        (ty, _) => return Err(format!("{} is not a {ty}", what(json))),
    })
}

fn what(json: &Json) -> &'static str {
    match json {
        Json::Null => "null",
        Json::Bool(_) => "a bool",
        Json::Number(_) => "a number",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

/// An integer of `bits` from a number, or for 64 bits also a canonical decimal string
/// (D20: no `+`, no leading zeros, no `-0`).
fn int_in(signed: bool, bits: u32, json: &Json) -> Result<Integer, String> {
    let n: i128 = match json {
        Json::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
            .ok_or_else(|| format!("{n} is not an integer"))?,
        Json::String(s) if bits == 64 => decimal(s)?,
        other => {
            let want = if bits == 64 {
                "a decimal string or an integer"
            } else {
                "an integer"
            };
            return Err(format!("{} is not {want}", what(other)));
        }
    };
    let (min, max) = if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    };
    if !(min..=max).contains(&n) {
        return Err(format!(
            "{n} is outside {}{bits}",
            if signed { 'i' } else { 'u' }
        ));
    }
    Integer::try_from(n).map_err(|e| e.to_string())
}

fn decimal(text: &str) -> Result<i128, String> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let canonical = !digits.is_empty()
        && digits.len() <= 20
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && text != "-0";
    if !canonical {
        return Err(format!("{text:?} is not a canonical decimal"));
    }
    text.parse().map_err(|e| format!("{text:?}: {e}"))
}

fn float_in(bits: u32, n: &Number) -> Result<f64, String> {
    let f = n.as_f64().ok_or_else(|| format!("{n} is not a number"))?;
    // An `f32` is stored as the SDK stores it: single precision, written shortest.
    #[allow(clippy::cast_possible_truncation)]
    let f = if bits == 32 { f64::from(f as f32) } else { f };
    if !f.is_finite() {
        return Err(format!("{n} is outside f{bits}"));
    }
    Ok(f)
}

/// JSON with no declared type: numbers as integers when exact, objects as text-keyed maps.
fn untyped_in(json: &Json) -> Result<Value, String> {
    Ok(match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => Value::Integer(u.into()),
            (_, Some(i)) => Value::Integer(i.into()),
            _ => Value::Float(float_in(64, n)?),
        },
        Json::String(s) => Value::Text(s.clone()),
        Json::Array(items) => Value::Array(items.iter().map(untyped_in).collect::<Result<_, _>>()?),
        Json::Object(object) => Value::Map(
            object
                .iter()
                .map(|(k, v)| Ok((Value::Text(k.clone()), untyped_in(v)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

// --- stored CBOR out ----------------------------------------------------------------

/// A stored record as the guest gets it: JSON text with every stored field by its declared
/// type; a field the schema does not declare, or one not of its type, is refused.
pub(crate) fn record_out(
    fields: &BTreeMap<String, ValueType>,
    record: &Value,
) -> Result<String, String> {
    let Value::Map(entries) = record else {
        return Err("a stored record is not a map".into());
    };
    let mut object = Map::new();
    for (name, value) in entries {
        let name = name.as_text().ok_or("a stored field name is not text")?;
        let ty = fields
            .get(name)
            .ok_or_else(|| format!("a stored field `{name}` the record does not declare"))?;
        let value = to_json(ty, value).map_err(|e| format!("stored field `{name}`: {e}"))?;
        let _prev = object.insert(name.to_owned(), value);
    }
    Ok(Json::Object(object).to_string())
}

/// A stored map value of type `ty` as JSON text.
pub(crate) fn value_out(ty: &ValueType, value: &Value) -> Result<String, String> {
    Ok(to_json(ty, value)?.to_string())
}

fn to_json(ty: &ValueType, value: &Value) -> Result<Json, String> {
    Ok(match (ty, value) {
        (ValueType::Bool, Value::Bool(b)) => Json::Bool(*b),
        (ValueType::Int { bits: 64, .. }, Value::Integer(i)) => {
            Json::String(i128::from(*i).to_string())
        }
        (ValueType::Int { .. }, Value::Integer(i)) => int_out(*i)?,
        (ValueType::Float { .. }, Value::Float(f)) => float_out(*f)?,
        (ValueType::Text, Value::Text(s)) => Json::String(s.clone()),
        (ValueType::Bytes, Value::Bytes(b)) => Json::String(URL_SAFE_NO_PAD.encode(b)),
        (ValueType::Unit | ValueType::Option(_), Value::Null) => Json::Null,
        (ValueType::BlobRef, Value::Tag(BLOB_REF_TAG, _)) => blob_out(value)?,
        (ValueType::Option(inner), _) => to_json(inner, value)?,
        (ValueType::List(inner), Value::Array(items)) => Json::Array(
            items
                .iter()
                .map(|item| to_json(inner, item))
                .collect::<Result<_, _>>()?,
        ),
        (ValueType::Tuple(types), Value::Array(items)) if types.len() == items.len() => {
            Json::Array(
                types
                    .iter()
                    .zip(items)
                    .map(|(ty, item)| to_json(ty, item))
                    .collect::<Result<_, _>>()?,
            )
        }
        (ValueType::Other(_), _) => untyped_out(value)?,
        (ty, _) => return Err(format!("not a {ty}")),
    })
}

fn int_out(i: Integer) -> Result<Json, String> {
    let n = i128::from(i);
    if let Ok(i) = i64::try_from(n) {
        return Ok(Json::from(i));
    }
    u64::try_from(n)
        .map(Json::from)
        .map_err(|_| format!("{n} is outside s64 and u64"))
}

fn float_out(f: f64) -> Result<Json, String> {
    Number::from_f64(f)
        .map(Json::Number)
        .ok_or_else(|| format!("{f} has no JSON number"))
}

fn blob_out(value: &Value) -> Result<Json, String> {
    match value {
        Value::Tag(BLOB_REF_TAG, inner) => match inner.as_ref() {
            Value::Bytes(b) => <[u8; 32]>::try_from(b.as_slice())
                .map(|id| Json::String(blob_id_hex(&id)))
                .map_err(|_| "a BlobRef holds 32 bytes".to_owned()),
            _ => Err("a BlobRef holds a byte string".into()),
        },
        _ => Err("not a BlobRef".into()),
    }
}

/// A stored value with no declared type, by its own shape: byte strings as base64url,
/// integer map keys as decimal strings, a `BlobRef` as its hex id, other tags dropped.
fn untyped_out(value: &Value) -> Result<Json, String> {
    Ok(match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Integer(i) => int_out(*i)?,
        Value::Float(f) => float_out(*f)?,
        Value::Text(s) => Json::String(s.clone()),
        Value::Bytes(b) => Json::String(URL_SAFE_NO_PAD.encode(b)),
        Value::Array(items) => {
            Json::Array(items.iter().map(untyped_out).collect::<Result<_, _>>()?)
        }
        Value::Map(entries) => {
            let mut object = Map::new();
            for (k, v) in entries {
                let key = match k {
                    Value::Text(s) => s.clone(),
                    Value::Integer(i) => i128::from(*i).to_string(),
                    _ => return Err("a stored map key that is not text or an integer".into()),
                };
                let _prev = object.insert(key, untyped_out(v)?);
            }
            Json::Object(object)
        }
        Value::Tag(BLOB_REF_TAG, _) => blob_out(value)?,
        Value::Tag(_, inner) => untyped_out(inner)?,
        _ => return Err("a stored value with no JSON form".into()),
    })
}
