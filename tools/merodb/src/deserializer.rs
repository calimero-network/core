use std::io::{Cursor, Read};

use borsh::BorshDeserialize;
use calimero_wasm_abi::schema::{CollectionType, Manifest, ScalarType, TypeDef, TypeRef};
use eyre::{Result, WrapErr};
use serde_json::{json, Value};

/// Deserialize a complete value using the ABI.
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

/// Deserialize a value from the provided cursor, advancing it.
pub fn deserialize_type_ref_from_cursor(
    cursor: &mut Cursor<&[u8]>,
    type_ref: &TypeRef,
    manifest: &Manifest,
) -> Result<Value> {
    deserialize_type_ref(cursor, type_ref, manifest)
}

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
                let bytes = Vec::<u8>::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize bytes")?;
                Ok(json!(hex::encode(bytes)))
            }
        }
        TypeDef::Alias { target } => deserialize_type_ref(cursor, target, manifest),
    }
}

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

fn deserialize_collection(
    cursor: &mut Cursor<&[u8]>,
    collection: &CollectionType,
    manifest: &Manifest,
) -> Result<Value> {
    match collection {
        CollectionType::List { items } => {
            let len =
                u32::deserialize_reader(cursor).wrap_err("Failed to deserialize list length")?;

            let mut array = Vec::new();
            for _ in 0..len {
                let value = deserialize_type_ref(cursor, items, manifest)?;
                array.push(value);
            }
            Ok(json!(array))
        }
        CollectionType::Map { key, value } => {
            let len =
                u32::deserialize_reader(cursor).wrap_err("Failed to deserialize map length")?;

            let mut map = serde_json::Map::new();
            for _ in 0..len {
                let key_value = deserialize_type_ref(cursor, key, manifest)?;
                let val_value = deserialize_type_ref(cursor, value, manifest)?;

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
