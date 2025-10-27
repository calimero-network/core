use std::io::{Cursor, Read};

use borsh::BorshDeserialize;
use calimero_wasm_abi::schema::{CollectionType, Manifest, ScalarType, TypeDef, TypeRef};
use eyre::{Result, WrapErr};
use serde_json::{json, Value};

/// Deserialize Borsh-encoded bytes into JSON using the ABI schema
pub fn deserialize_with_abi(data: &[u8], manifest: &Manifest, type_name: &str) -> Result<Value> {
    let type_def = manifest
        .types
        .get(type_name)
        .ok_or_else(|| eyre::eyre!("Type '{type_name}' not found in ABI schema"))?;

    let mut cursor = Cursor::new(data);
    deserialize_type_def(&mut cursor, type_def, manifest)
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
/// This looks for common state type names in the manifest
pub fn deserialize_root_state(data: &[u8], manifest: &Manifest) -> Result<Value> {
    // Try to find the root state type
    // Common patterns: the type used in the first method's return type,
    // or a type that looks like application state

    // For now, try to find a type that's used in init methods or has "State" in the name
    let state_type = manifest
        .types
        .keys()
        .find(|name| {
            name.contains("State")
                || manifest.methods.iter().any(|m| {
                    m.name == "init"
                        && m.returns.as_ref().is_some_and(
                            |r| matches!(r, TypeRef::Reference { ref_ } if ref_ == *name),
                        )
                })
        })
        .ok_or_else(|| eyre::eyre!("Could not determine root state type from ABI"))?;

    deserialize_with_abi(data, manifest, state_type)
}
