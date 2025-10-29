use std::io::{Cursor, Read};

use borsh::BorshDeserialize;
use calimero_wasm_abi::schema::{CollectionType, Manifest, ScalarType, TypeDef, TypeRef};
use eyre::{Result, WrapErr};
use serde_json::{json, Value};

/// Parse Borsh-encoded data generically without type information
///
/// This attempts to deserialize common Borsh patterns by trying to parse
/// all values sequentially until the data is exhausted.
pub fn parse_borsh_generic(data: &[u8]) -> Value {
    let mut cursor = Cursor::new(data);
    let mut values = Vec::new();

    // Parse all values until we run out of data
    while cursor.position() < data.len() as u64 {
        if let Ok(val) = parse_borsh_value(&mut cursor) {
            values.push(val);
        } else {
            // If we can't parse more, include remaining as hex
            #[expect(
                clippy::cast_possible_truncation,
                reason = "position is always less than data.len()"
            )]
            let pos = cursor.position() as usize;
            if pos < data.len() {
                values.push(json!({
                    "remaining_hex": hex::encode(&data[pos..]),
                    "size": data.len().saturating_sub(pos)
                }));
            }
            break;
        }
    }

    // If only one value, return it directly; otherwise return as array
    if values.len() == 1 {
        values.into_iter().next().unwrap()
    } else {
        json!(values)
    }
}

/// Parse a single Borsh value from the cursor
fn parse_borsh_value(cursor: &mut Cursor<&[u8]>) -> Result<Value> {
    let pos = cursor.position();

    // Try as string first (most common in app state)
    if let Ok(s) = String::deserialize_reader(cursor) {
        if is_reasonable_string(&s) {
            return Ok(json!(s));
        }
    }

    // Try as u64 (timestamps, counters)
    cursor.set_position(pos);
    if let Ok(val) = u64::deserialize_reader(cursor) {
        // Check if it looks like a timestamp (reasonable range)
        if val > 1_600_000_000_000_000_000 && val < 2_000_000_000_000_000_000 {
            return Ok(json!({
                "timestamp_ns": val,
                "timestamp_readable": format_timestamp_ns(val)
            }));
        }
        if val < 1_000_000_000 {
            return Ok(json!(val));
        }
    }

    // Try as 32-byte ID/hash
    cursor.set_position(pos);
    let remaining = (cursor.get_ref().len() as u64).saturating_sub(pos);
    if remaining >= 32 {
        let mut bytes = [0_u8; 32];
        if cursor.read_exact(&mut bytes).is_ok() {
            // Check if it looks like an ID (not all zeros)
            if bytes.iter().any(|&b| b != 0) {
                return Ok(json!({
                    "id_or_hash": hex::encode(bytes)
                }));
            }
        }
    }

    // Try as bool
    cursor.set_position(pos);
    if let Ok(val) = bool::deserialize_reader(cursor) {
        return Ok(json!(val));
    }

    // Try as u32
    cursor.set_position(pos);
    if let Ok(val) = u32::deserialize_reader(cursor) {
        return Ok(json!(val));
    }

    // Nothing matched - fail
    eyre::bail!("Could not parse value at position {pos}")
}

/// Check if a string looks reasonable (printable ASCII/UTF-8, reasonable length)
fn is_reasonable_string(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 1000
        && (s
            .chars()
            .all(|c| c.is_ascii_graphic() || c.is_whitespace() || c.is_ascii_alphanumeric()))
}

/// Format a nanosecond timestamp as human-readable
fn format_timestamp_ns(ns: u64) -> String {
    use core::time::Duration;

    let duration = Duration::from_nanos(ns);

    // Simple formatting - just show seconds since epoch for now
    format!("{} seconds since epoch", duration.as_secs())
}

/// Deserialize Borsh-encoded bytes into JSON using the ABI schema
pub fn deserialize_with_abi(data: &[u8], manifest: &Manifest, type_name: &str) -> Result<Value> {
    let type_def = manifest
        .types
        .get(type_name)
        .ok_or_else(|| eyre::eyre!("Type '{type_name}' not found in ABI schema"))?;

    let mut cursor = Cursor::new(data);
    let value = deserialize_type_def(&mut cursor, type_def, manifest)?;
    if cursor.position() != data.len() as u64 {
        eyre::bail!(
            "Type '{type_name}' did not consume all bytes (consumed {}, total {})",
            cursor.position(),
            data.len()
        );
    }
    Ok(value)
}

/// Deserialize a type definition from a cursor
fn deserialize_type_def(
    cursor: &mut Cursor<&[u8]>,
    type_def: &TypeDef,
    manifest: &Manifest,
) -> Result<Value> {
    match type_def {
        TypeDef::Record { fields } => {
            let mut obj = serde_json::Map::new();
            for field in fields {
                let value = deserialize_type_ref(cursor, &field.type_, manifest)?;
                drop(obj.insert(field.name.clone(), value));
            }
            Ok(json!(obj))
        }
        TypeDef::Variant { variants } => {
            // Borsh encodes variants as u32 discriminant + optional payload
            let discriminant = u32::deserialize_reader(cursor)
                .wrap_err("Failed to deserialize variant discriminant")?;

            let variant = variants
                .get(discriminant as usize)
                .ok_or_else(|| eyre::eyre!("Invalid variant discriminant: {discriminant}"))?;

            if let Some(payload_type) = &variant.payload {
                let payload = deserialize_type_ref(cursor, payload_type, manifest)?;
                Ok(json!({
                    "variant": variant.name,
                    "payload": payload
                }))
            } else {
                Ok(json!(variant.name))
            }
        }
        TypeDef::Bytes { size, .. } => {
            if let Some(size) = size {
                let mut bytes = vec![0_u8; *size];
                cursor
                    .read_exact(&mut bytes)
                    .wrap_err("Failed to read fixed-size bytes")?;
                Ok(json!(hex::encode(bytes)))
            } else {
                // Variable-length bytes (Vec<u8>)
                let bytes = Vec::<u8>::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize bytes")?;
                Ok(json!(hex::encode(bytes)))
            }
        }
        TypeDef::Alias { target } => deserialize_type_ref(cursor, target, manifest),
    }
}

/// Deserialize a type reference from a cursor
fn deserialize_type_ref(
    cursor: &mut Cursor<&[u8]>,
    type_ref: &TypeRef,
    manifest: &Manifest,
) -> Result<Value> {
    match type_ref {
        TypeRef::Reference { ref_ } => {
            let type_definition = manifest
                .types
                .get(ref_)
                .ok_or_else(|| eyre::eyre!("Type '{ref_}' not found in ABI schema"))?;
            deserialize_type_def(cursor, type_definition, manifest)
        }
        TypeRef::Scalar(scalar) => deserialize_scalar(cursor, scalar),
        TypeRef::Collection(collection) => deserialize_collection(cursor, collection, manifest),
    }
}

/// Deserialize a scalar type from a cursor
fn deserialize_scalar(cursor: &mut Cursor<&[u8]>, scalar_type: &ScalarType) -> Result<Value> {
    match scalar_type {
        ScalarType::Bool => {
            let value = bool::deserialize_reader(cursor).wrap_err("Failed to deserialize bool")?;
            Ok(json!(value))
        }
        ScalarType::I32 => {
            let value = i32::deserialize_reader(cursor).wrap_err("Failed to deserialize i32")?;
            Ok(json!(value))
        }
        ScalarType::I64 => {
            let value = i64::deserialize_reader(cursor).wrap_err("Failed to deserialize i64")?;
            Ok(json!(value))
        }
        ScalarType::U32 => {
            let value = u32::deserialize_reader(cursor).wrap_err("Failed to deserialize u32")?;
            Ok(json!(value))
        }
        ScalarType::U64 => {
            let value = u64::deserialize_reader(cursor).wrap_err("Failed to deserialize u64")?;
            Ok(json!(value))
        }
        ScalarType::F32 => {
            let value = f32::deserialize_reader(cursor).wrap_err("Failed to deserialize f32")?;
            Ok(json!(value))
        }
        ScalarType::F64 => {
            let value = f64::deserialize_reader(cursor).wrap_err("Failed to deserialize f64")?;
            Ok(json!(value))
        }
        ScalarType::String => {
            let value =
                String::deserialize_reader(cursor).wrap_err("Failed to deserialize string")?;
            Ok(json!(value))
        }
        ScalarType::Bytes { size, .. } => {
            if let Some(size) = size {
                let mut bytes = vec![0_u8; *size];
                cursor
                    .read_exact(&mut bytes)
                    .wrap_err("Failed to read fixed-size bytes")?;
                Ok(json!(hex::encode(bytes)))
            } else {
                let bytes = Vec::<u8>::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize bytes")?;
                Ok(json!(hex::encode(bytes)))
            }
        }
        ScalarType::Unit => Ok(json!(null)),
    }
}

/// Deserialize a collection type from a cursor
fn deserialize_collection(
    cursor: &mut Cursor<&[u8]>,
    collection: &CollectionType,
    manifest: &Manifest,
) -> Result<Value> {
    match collection {
        CollectionType::List { items } => {
            // Borsh encodes Vec as u32 length + elements
            let len =
                u32::deserialize_reader(cursor).wrap_err("Failed to deserialize list length")?;

            let mut array = Vec::new();
            for _ in 0..len {
                let value = deserialize_type_ref(cursor, items, manifest)?;
                array.push(value);
            }
            Ok(json!(array))
        }
        CollectionType::Map {
            key: key_type,
            value: value_type,
        } => {
            // Borsh encodes BTreeMap as u32 length + (key, value) pairs
            let len =
                u32::deserialize_reader(cursor).wrap_err("Failed to deserialize map length")?;

            // For JSON, we need string keys
            let mut map = serde_json::Map::new();
            for _ in 0..len {
                let key_value = deserialize_type_ref(cursor, key_type, manifest)?;
                let val_value = deserialize_type_ref(cursor, value_type, manifest)?;

                // Convert key to string if it isn't already
                let key_str = match key_value {
                    Value::String(s) => s,
                    other => other.to_string(),
                };

                drop(map.insert(key_str, val_value));
            }
            Ok(json!(map))
        }
        CollectionType::Record { fields } => {
            let mut obj = serde_json::Map::new();
            for field in fields {
                let value = deserialize_type_ref(cursor, &field.type_, manifest)?;
                drop(obj.insert(field.name.clone(), value));
            }
            Ok(json!(obj))
        }
    }
}

/// Deserialize the root state type
///
/// This looks for the application state type in the manifest using several strategies
pub fn deserialize_root_state(data: &[u8], manifest: &Manifest) -> Result<Value> {
    // Strategy 1: Look for types that implement AppState trait or have State suffix
    // Common patterns in Calimero SDK apps:
    // - Type names ending with "State" (e.g., "KvStore", "Marketplace", etc. don't always end with State)
    // - Types that are used as &mut self in #[app::logic] methods

    // Strategy 2: Find the type that appears most frequently as &mut self in methods
    // In Calimero apps, the state struct is the impl target for all logic methods

    // Strategy 3: Look for struct types (not enums/variants) that are used in methods
    let state_type_candidates: Vec<_> = manifest
        .types
        .iter()
        .filter(|(_, typedef)| {
            // Only consider Record types (structs), not Variants (enums)
            matches!(typedef, TypeDef::Record { .. })
        })
        .map(|(name, _)| name.clone())
        .collect();

    // If there's only one struct type, that's likely the state
    if state_type_candidates.len() == 1 {
        return deserialize_with_abi(data, manifest, &state_type_candidates[0]);
    }

    // Try to find based on method parameters/returns
    // In Calimero SDK apps, methods don't typically return the state type
    // but the state is the struct on which methods are implemented

    // Look for types with "State" in the name as a fallback
    let state_type = state_type_candidates
        .iter()
        .find(|name| name.contains("State"))
        .or_else(|| {
            // If no "State" suffix, try the first struct type
            state_type_candidates.first()
        })
        .ok_or_else(|| {
            eyre::eyre!(
                "Could not determine root state type from ABI. Found {} struct types: {:?}",
                state_type_candidates.len(),
                state_type_candidates
            )
        })?;

    deserialize_with_abi(data, manifest, state_type)
}
