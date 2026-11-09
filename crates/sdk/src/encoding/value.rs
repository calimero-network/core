//! Deterministic CBOR values (RFC 8949 core deterministic encoding).
//!
//! The value is serialized with ciborium, read back as a generic tree, canonicalised
//! (map keys sorted by their encoded bytes, recursively) and written again. ciborium writes
//! the shortest integer and float forms and only definite lengths.

use ciborium::Value;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::StorageError;

/// Encode `value` as deterministic CBOR; NaN is refused.
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, StorageError> {
    let tree = Value::serialized(value).map_err(|e| StorageError::Encode(e.to_string()))?;
    let tree = canonical(tree)?;
    let mut out = Vec::new();
    ciborium::into_writer(&tree, &mut out).map_err(|e| StorageError::Encode(e.to_string()))?;
    Ok(out)
}

/// Decode a CBOR value.
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StorageError> {
    ciborium::from_reader(bytes).map_err(|e| StorageError::Decode(e.to_string()))
}

fn canonical(value: Value) -> Result<Value, StorageError> {
    Ok(match value {
        Value::Float(f) if f.is_nan() => return Err(StorageError::Nan),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(canonical).collect::<Result<_, _>>()?)
        }
        Value::Tag(tag, inner) => Value::Tag(tag, Box::new(canonical(*inner)?)),
        Value::Map(entries) => {
            let mut keyed = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = canonical(k)?;
                let mut encoded = Vec::new();
                ciborium::into_writer(&k, &mut encoded)
                    .map_err(|e| StorageError::Encode(e.to_string()))?;
                keyed.push((encoded, k, canonical(v)?));
            }
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Map(keyed.into_iter().map(|(_, k, v)| (k, v)).collect())
        }
        other => other,
    })
}

/// A shortest-form CBOR integer, as `storage.add` keeps it.
pub fn int_to_vec(n: i128) -> Result<Vec<u8>, StorageError> {
    let int = ciborium::value::Integer::try_from(n)
        .map_err(|_| StorageError::Encode("integer out of range".into()))?;
    to_vec(&Value::Integer(int))
}

/// Read a CBOR integer written by [`int_to_vec`] or by the host's `storage.add`.
pub fn int_from_slice(bytes: &[u8]) -> Result<i128, StorageError> {
    let v: Value = from_slice(bytes)?;
    match v {
        Value::Integer(i) => Ok(i128::from(i)),
        _ => Err(StorageError::Decode("not an integer".into())),
    }
}
