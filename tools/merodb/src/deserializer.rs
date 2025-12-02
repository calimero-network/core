use std::io::{Cursor, Read};

use borsh::BorshDeserialize;
use calimero_wasm_abi::schema::{
    CollectionType, CrdtCollectionType, Manifest, ScalarType, TypeDef, TypeRef,
};
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
                Ok(json!(String::from_utf8_lossy(&bytes)))
            } else {
                let bytes = Vec::<u8>::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize bytes")?;
                Ok(json!(String::from_utf8_lossy(&bytes)))
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
        TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } => deserialize_collection_with_crdt(cursor, collection, crdt_type, inner_type, manifest),
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
                Ok(json!(String::from_utf8_lossy(&bytes)))
            } else {
                let bytes = Vec::<u8>::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize bytes")?;
                Ok(json!(String::from_utf8_lossy(&bytes)))
            }
        }
        ScalarType::Unit => Ok(json!(null)),
    }
}

fn deserialize_collection_with_crdt(
    cursor: &mut Cursor<&[u8]>,
    collection: &CollectionType,
    crdt_type: &Option<CrdtCollectionType>,
    inner_type: &Option<Box<TypeRef>>,
    manifest: &Manifest,
) -> Result<Value> {
    // Handle CRDT types with their special serialization formats
    if let Some(crdt) = crdt_type {
        return match crdt {
            CrdtCollectionType::LwwRegister => {
                // LwwRegister<T> serializes as (value: T, timestamp: HybridTimestamp, node_id: [u8; 32])
                // Use inner_type to deserialize the value correctly
                let value_type = inner_type
                    .as_ref()
                    .map(|t| t.as_ref())
                    .ok_or_else(|| eyre::eyre!("LwwRegister missing inner_type in schema"))?;

                let value = deserialize_type_ref(cursor, value_type, manifest)?;
                let timestamp = u64::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize LwwRegister timestamp")?;
                let mut node_id = [0_u8; 32];
                cursor
                    .read_exact(&mut node_id)
                    .wrap_err("Failed to deserialize LwwRegister node_id")?;

                Ok(json!({
                    "value": value,
                    "timestamp": timestamp,
                    "node_id": hex::encode(node_id),
                    "crdt_type": "LwwRegister"
                }))
            }
            CrdtCollectionType::Counter => {
                // Counter serializes as (positive: UnorderedMap<String, u64>, negative?: UnorderedMap<String, u64>)
                // UnorderedMap<String, u64> serializes as: length (u32), then for each entry: key (String), value (u64), element_id ([u8; 32])

                // Deserialize positive map
                let pos_len = u32::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize Counter positive map length")?;

                let mut positive_entries = serde_json::Map::new();
                let mut positive_total: u64 = 0;

                for _ in 0..pos_len {
                    let key = String::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize Counter positive map key")?;
                    let value = u64::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize Counter positive map value")?;
                    let mut element_id = [0_u8; 32];
                    cursor
                        .read_exact(&mut element_id)
                        .wrap_err("Failed to read Counter positive map element_id")?;

                    positive_total += value;
                    drop(positive_entries.insert(
                        key.clone(),
                        json!({
                            "value": value,
                            "element_id": hex::encode(element_id)
                        }),
                    ));
                }

                // Try to deserialize negative map (might not exist for GCounter)
                let mut negative_entries = serde_json::Map::new();
                let mut negative_total: u64 = 0;
                let has_negative = {
                    let saved_pos = cursor.position();
                    match u32::deserialize_reader(cursor) {
                        Ok(neg_len) => {
                            for _ in 0..neg_len {
                                let key = String::deserialize_reader(cursor)
                                    .wrap_err("Failed to deserialize Counter negative map key")?;
                                let value = u64::deserialize_reader(cursor)
                                    .wrap_err("Failed to deserialize Counter negative map value")?;
                                let mut element_id = [0_u8; 32];
                                cursor
                                    .read_exact(&mut element_id)
                                    .wrap_err("Failed to read Counter negative map element_id")?;

                                negative_total += value;
                                drop(negative_entries.insert(
                                    key.clone(),
                                    json!({
                                        "value": value,
                                        "element_id": hex::encode(element_id)
                                    }),
                                ));
                            }
                            true
                        }
                        Err(_) => {
                            // No negative map (GCounter) - restore cursor position
                            cursor.set_position(saved_pos);
                            false
                        }
                    }
                };

                // Calculate total value
                let total_value = if has_negative {
                    // PN-Counter: positive - negative (can be negative)
                    positive_total as i64 - negative_total as i64
                } else {
                    // GCounter: just positive (always non-negative)
                    positive_total as i64
                };

                Ok(json!({
                    "value": total_value,
                    "positive": {
                        "entries": positive_entries,
                        "total": positive_total
                    },
                    "negative": if has_negative {
                        json!({
                            "entries": negative_entries,
                            "total": negative_total
                        })
                    } else {
                        json!(null)
                    },
                    "crdt_type": if has_negative { "PNCounter" } else { "GCounter" }
                }))
            }
            CrdtCollectionType::Vector => {
                // Vector<T> has CRDT metadata but serializes similarly to Vec<T>
                // Deserialize as a list but note it's a Vector
                match collection {
                    CollectionType::List { items } => {
                        let len = u32::deserialize_reader(cursor)
                            .wrap_err("Failed to deserialize Vector length")?;
                        let mut array = Vec::new();
                        for _ in 0..len {
                            let value = deserialize_type_ref(cursor, items, manifest)?;
                            array.push(value);
                        }
                        Ok(json!({
                            "items": array,
                            "crdt_type": "Vector"
                        }))
                    }
                    _ => {
                        eyre::bail!("Vector CRDT type must be a List collection");
                    }
                }
            }
            CrdtCollectionType::UnorderedMap => {
                // UnorderedMap<K, V> has element IDs and CRDT metadata
                // Deserialize as a map but note it's an UnorderedMap
                match collection {
                    CollectionType::Map { key, value } => {
                        let len = u32::deserialize_reader(cursor)
                            .wrap_err("Failed to deserialize UnorderedMap length")?;
                        let mut map = serde_json::Map::new();
                        for _ in 0..len {
                            let key_value = deserialize_type_ref(cursor, key, manifest)?;
                            let val_value = deserialize_type_ref(cursor, value, manifest)?;

                            // UnorderedMap entries have element_id after key-value pair
                            let mut element_id = [0_u8; 32];
                            cursor
                                .read_exact(&mut element_id)
                                .wrap_err("Failed to read UnorderedMap element_id")?;

                            let key_str = match key_value {
                                Value::String(s) => s,
                                other => other.to_string(),
                            };

                            drop(map.insert(
                                key_str,
                                json!({
                                    "value": val_value,
                                    "element_id": hex::encode(element_id)
                                }),
                            ));
                        }
                        Ok(json!({
                            "entries": map,
                            "crdt_type": "UnorderedMap"
                        }))
                    }
                    _ => {
                        eyre::bail!("UnorderedMap CRDT type must be a Map collection");
                    }
                }
            }
            CrdtCollectionType::UnorderedSet => {
                // UnorderedSet<T> serializes similarly to Vec<T> but with CRDT metadata
                match collection {
                    CollectionType::List { items } => {
                        let len = u32::deserialize_reader(cursor)
                            .wrap_err("Failed to deserialize UnorderedSet length")?;
                        let mut array = Vec::new();
                        for _ in 0..len {
                            let value = deserialize_type_ref(cursor, items, manifest)?;
                            array.push(value);
                        }
                        Ok(json!({
                            "items": array,
                            "crdt_type": "UnorderedSet"
                        }))
                    }
                    _ => {
                        eyre::bail!("UnorderedSet CRDT type must be a List collection");
                    }
                }
            }
            CrdtCollectionType::ReplicatedGrowableArray => {
                // RGA serializes as UnorderedMap<CharKey, RgaChar>
                // CharKey serializes as CharId { timestamp: HybridTimestamp, seq: u32 }
                // HybridTimestamp serializes as time: u64, id: u128
                // RgaChar serializes as { content: u32, left: CharId }
                // UnorderedMap serializes as: length (u32), then for each entry: key, value, element_id ([u8; 32])

                // Deserialize map length
                let len = u32::deserialize_reader(cursor)
                    .wrap_err("Failed to deserialize RGA map length")?;

                // Deserialize all characters
                let mut chars: Vec<(CharIdData, RgaCharData, String)> = Vec::new();

                for _ in 0..len {
                    // Deserialize CharId (key)
                    let time = u64::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA CharId timestamp")?;
                    let id = u128::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA CharId id")?;
                    let seq = u32::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA CharId seq")?;

                    let char_id = CharIdData { time, id, seq };

                    // Deserialize RgaChar (value)
                    let content = u32::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA character content")?;

                    let left_time = u64::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA left timestamp")?;
                    let left_id = u128::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA left id")?;
                    let left_seq = u32::deserialize_reader(cursor)
                        .wrap_err("Failed to deserialize RGA left seq")?;

                    let left = CharIdData {
                        time: left_time,
                        id: left_id,
                        seq: left_seq,
                    };

                    let rga_char = RgaCharData { content, left };

                    // Deserialize element_id
                    let mut element_id = [0_u8; 32];
                    cursor
                        .read_exact(&mut element_id)
                        .wrap_err("Failed to read RGA element_id")?;

                    chars.push((char_id, rga_char, hex::encode(element_id)));
                }

                // Reconstruct text by following left-neighbor links
                let text = reconstruct_rga_text(&chars);

                // Build entries map for detailed view
                let mut entries = serde_json::Map::new();
                for (char_id, rga_char, element_id) in &chars {
                    let char_value = char::from_u32(rga_char.content).unwrap_or('\u{FFFD}');
                    let char_id_str = format!("{}#{}", char_id.time, char_id.seq);
                    drop(entries.insert(
                        char_id_str,
                        json!({
                            "char": char_value,
                            "content": rga_char.content,
                            "left": format!("{}#{}", rga_char.left.time, rga_char.left.seq),
                            "element_id": element_id
                        }),
                    ));
                }

                Ok(json!({
                    "text": text,
                    "length": text.chars().count(),
                    "entries": entries,
                    "crdt_type": "ReplicatedGrowableArray"
                }))
            }
        };
    }

    // Standard collection deserialization (no CRDT)
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

// Helper structs for RGA deserialization
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharIdData {
    pub time: u64,
    pub id: u128,
    pub seq: u32,
}

impl PartialEq<&CharIdData> for CharIdData {
    fn eq(&self, other: &&CharIdData) -> bool {
        self == *other
    }
}

#[derive(Clone, Debug)]
pub struct RgaCharData {
    pub content: u32,
    pub left: CharIdData,
}

// Reconstruct RGA text by following left-neighbor links
pub fn reconstruct_rga_text(chars: &[(CharIdData, RgaCharData, String)]) -> String {
    if chars.is_empty() {
        return String::new();
    }

    // Root CharId has time=0, id=1 (DEFAULT_ID), seq=0 (from HybridTimestamp::default())
    // This is the sentinel that represents the beginning of the document
    let root = CharIdData {
        time: 0,
        id: 1,
        seq: 0,
    };

    // Build map of CharId -> RgaChar
    let mut char_map: std::collections::HashMap<CharIdData, (RgaCharData, char)> =
        std::collections::HashMap::new();

    for (char_id, rga_char, _) in chars {
        let char_value = char::from_u32(rga_char.content).unwrap_or('\u{FFFD}');
        char_map.insert(char_id.clone(), (rga_char.clone(), char_value));
    }

    // Build ordered list by following left-neighbor links
    let mut ordered = Vec::new();
    let mut current_left = root;

    // Keep iterating until we've placed all characters
    while ordered.len() < chars.len() {
        // Find all characters that come after current_left
        let mut candidates: Vec<_> = char_map
            .iter()
            .filter(|(_, (c, _))| c.left == current_left)
            .filter(|(id, _)| !ordered.iter().any(|(placed_id, _)| *placed_id == **id))
            .collect();

        if candidates.is_empty() {
            // No more characters for this left - find next unplaced char
            // This handles concurrent insertions that created gaps
            if let Some((next_id, (_, char_value))) = char_map
                .iter()
                .find(|(id, _)| !ordered.iter().any(|(placed_id, _)| placed_id == *id))
            {
                ordered.push((next_id.clone(), *char_value));
                current_left = next_id.clone();
            } else {
                break;
            }
        } else {
            // Sort by CharId in REVERSE order (latest timestamp first)
            // This ensures sequential mid-document insertions are placed correctly
            candidates.sort_by_key(|(id, _)| std::cmp::Reverse((*id).clone()));

            // Take the character with highest CharId (latest timestamp)
            let (next_id, (_, char_value)) = candidates[0];
            ordered.push((next_id.clone(), *char_value));
            current_left = next_id.clone();
        }
    }

    // Convert to string
    ordered.iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use borsh::BorshSerialize;

    use super::*;

    /// Helper to create a simple manifest with a given type definition
    fn create_manifest_with_type(type_name: &str, type_def: TypeDef) -> Manifest {
        let mut manifest = Manifest::new();
        drop(manifest.types.insert(type_name.to_owned(), type_def));
        manifest
    }

    #[test]
    fn test_deserialize_scalar_types() -> Result<()> {
        // Test bool
        let manifest = create_manifest_with_type(
            "BoolType",
            TypeDef::Alias {
                target: TypeRef::bool(),
            },
        );
        let data = borsh::to_vec(&true)?;
        let result = deserialize_with_abi(&data, &manifest, "BoolType")?;
        assert_eq!(result, json!(true));

        // Test i32
        let manifest = create_manifest_with_type(
            "I32Type",
            TypeDef::Alias {
                target: TypeRef::i32(),
            },
        );
        let data = borsh::to_vec(&42_i32)?;
        let result = deserialize_with_abi(&data, &manifest, "I32Type")?;
        assert_eq!(result, json!(42_i32));

        // Test u64
        let manifest = create_manifest_with_type(
            "U64Type",
            TypeDef::Alias {
                target: TypeRef::u64(),
            },
        );
        let data = borsh::to_vec(&12345_u64)?;
        let result = deserialize_with_abi(&data, &manifest, "U64Type")?;
        assert_eq!(result, json!(12345_u64));

        // Test string
        let manifest = create_manifest_with_type(
            "StringType",
            TypeDef::Alias {
                target: TypeRef::string(),
            },
        );
        let data = borsh::to_vec(&"hello".to_owned())?;
        let result = deserialize_with_abi(&data, &manifest, "StringType")?;
        assert_eq!(result, json!("hello"));

        Ok(())
    }

    #[test]
    fn test_deserialize_record() -> Result<()> {
        use calimero_wasm_abi::schema::Field;

        let manifest = create_manifest_with_type(
            "Person",
            TypeDef::Record {
                fields: vec![
                    Field {
                        name: "name".to_owned(),
                        type_: TypeRef::string(),
                        nullable: None,
                    },
                    Field {
                        name: "age".to_owned(),
                        type_: TypeRef::u32(),
                        nullable: None,
                    },
                ],
            },
        );

        // Serialize person record: {name: "Alice", age: 30}
        let mut data = Vec::new();
        "Alice".to_owned().serialize(&mut data)?;
        30_u32.serialize(&mut data)?;

        let result = deserialize_with_abi(&data, &manifest, "Person")?;
        assert_eq!(
            result,
            json!({
                "name": "Alice",
                "age": 30_i32
            })
        );

        Ok(())
    }

    #[test]
    fn test_deserialize_variant_without_payload() -> Result<()> {
        use calimero_wasm_abi::schema::Variant;

        let manifest = create_manifest_with_type(
            "Status",
            TypeDef::Variant {
                variants: vec![
                    Variant {
                        name: "Active".to_owned(),
                        code: None,
                        payload: None,
                    },
                    Variant {
                        name: "Inactive".to_owned(),
                        code: None,
                        payload: None,
                    },
                ],
            },
        );

        // Serialize variant discriminant 0 (Active)
        let data = borsh::to_vec(&0_u32)?;
        let result = deserialize_with_abi(&data, &manifest, "Status")?;
        assert_eq!(result, json!("Active"));

        // Serialize variant discriminant 1 (Inactive)
        let data = borsh::to_vec(&1_u32)?;
        let result = deserialize_with_abi(&data, &manifest, "Status")?;
        assert_eq!(result, json!("Inactive"));

        Ok(())
    }

    #[test]
    fn test_deserialize_variant_with_payload() -> Result<()> {
        use calimero_wasm_abi::schema::Variant;

        let manifest = create_manifest_with_type(
            "Result",
            TypeDef::Variant {
                variants: vec![
                    Variant {
                        name: "Ok".to_owned(),
                        code: None,
                        payload: Some(TypeRef::u32()),
                    },
                    Variant {
                        name: "Err".to_owned(),
                        code: None,
                        payload: Some(TypeRef::string()),
                    },
                ],
            },
        );

        // Serialize Ok(42)
        let mut data = Vec::new();
        0_u32.serialize(&mut data)?; // discriminant
        42_u32.serialize(&mut data)?; // payload
        let result = deserialize_with_abi(&data, &manifest, "Result")?;
        assert_eq!(
            result,
            json!({
                "variant": "Ok",
                "payload": 42_i32
            })
        );

        // Serialize Err("failed")
        let mut data = Vec::new();
        1_u32.serialize(&mut data)?; // discriminant
        "failed".to_owned().serialize(&mut data)?; // payload
        let result = deserialize_with_abi(&data, &manifest, "Result")?;
        assert_eq!(
            result,
            json!({
                "variant": "Err",
                "payload": "failed"
            })
        );

        Ok(())
    }

    #[test]
    fn test_deserialize_list() -> Result<()> {
        let manifest = create_manifest_with_type(
            "Numbers",
            TypeDef::Alias {
                target: TypeRef::list(TypeRef::u32()),
            },
        );

        // Serialize list: [1, 2, 3]
        let numbers = vec![1_u32, 2_u32, 3_u32];
        let data = borsh::to_vec(&numbers)?;

        let result = deserialize_with_abi(&data, &manifest, "Numbers")?;
        assert_eq!(result, json!([1, 2, 3]));

        Ok(())
    }

    #[test]
    fn test_deserialize_map() -> Result<()> {
        let manifest = create_manifest_with_type(
            "StringToU32Map",
            TypeDef::Alias {
                target: TypeRef::map(TypeRef::u32()),
            },
        );

        // Serialize map: {"a": 1, "b": 2}
        let mut data = Vec::new();
        2_u32.serialize(&mut data)?; // length
        "a".to_owned().serialize(&mut data)?;
        1_u32.serialize(&mut data)?;
        "b".to_owned().serialize(&mut data)?;
        2_u32.serialize(&mut data)?;

        let result = deserialize_with_abi(&data, &manifest, "StringToU32Map")?;
        assert_eq!(
            result,
            json!({
                "a": 1,
                "b": 2
            })
        );

        Ok(())
    }

    #[test]
    fn test_deserialize_nested_record() -> Result<()> {
        use calimero_wasm_abi::schema::Field;

        let mut manifest = Manifest::new();

        // Define Address type
        drop(manifest.types.insert(
            "Address".to_owned(),
            TypeDef::Record {
                fields: vec![
                    Field {
                        name: "street".to_owned(),
                        type_: TypeRef::string(),
                        nullable: None,
                    },
                    Field {
                        name: "city".to_owned(),
                        type_: TypeRef::string(),
                        nullable: None,
                    },
                ],
            },
        ));

        // Define Person type with nested Address
        drop(manifest.types.insert(
            "Person".to_owned(),
            TypeDef::Record {
                fields: vec![
                    Field {
                        name: "name".to_owned(),
                        type_: TypeRef::string(),
                        nullable: None,
                    },
                    Field {
                        name: "address".to_owned(),
                        type_: TypeRef::reference("Address"),
                        nullable: None,
                    },
                ],
            },
        ));

        // Serialize nested record
        let mut data = Vec::new();
        "Alice".to_owned().serialize(&mut data)?;
        "123 Main St".to_owned().serialize(&mut data)?;
        "Springfield".to_owned().serialize(&mut data)?;

        let result = deserialize_with_abi(&data, &manifest, "Person")?;
        assert_eq!(
            result,
            json!({
                "name": "Alice",
                "address": {
                    "street": "123 Main St",
                    "city": "Springfield"
                }
            })
        );

        Ok(())
    }

    #[test]
    fn test_deserialize_bytes() -> Result<()> {
        // Variable-length bytes
        let manifest = create_manifest_with_type(
            "DynamicBytes",
            TypeDef::Alias {
                target: TypeRef::bytes(),
            },
        );
        let bytes = vec![0x01_u8, 0x02_u8, 0x03_u8];
        let data = borsh::to_vec(&bytes)?;
        let result = deserialize_with_abi(&data, &manifest, "DynamicBytes")?;
        assert_eq!(result, json!("\u{1}\u{2}\u{3}"));

        // Fixed-size bytes
        let manifest = create_manifest_with_type(
            "FixedBytes",
            TypeDef::Bytes {
                size: Some(4),
                encoding: None,
            },
        );
        let data = vec![0x41_u8, 0x42_u8, 0x43_u8, 0x44_u8]; // "ABCD"
        let result = deserialize_with_abi(&data, &manifest, "FixedBytes")?;
        assert_eq!(result, json!("ABCD"));

        Ok(())
    }

    #[test]
    fn test_deserialize_error_type_not_found() {
        let manifest = Manifest::new();
        let data = vec![0x01_u8, 0x02_u8];

        let result = deserialize_with_abi(&data, &manifest, "NonExistentType");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Type 'NonExistentType' not found"));
    }

    #[test]
    fn test_deserialize_error_invalid_variant_discriminant() {
        use calimero_wasm_abi::schema::Variant;

        let manifest = create_manifest_with_type(
            "Status",
            TypeDef::Variant {
                variants: vec![Variant {
                    name: "Active".to_owned(),
                    code: None,
                    payload: None,
                }],
            },
        );

        // Invalid discriminant (out of range)
        let data = borsh::to_vec(&5_u32).unwrap();
        let result = deserialize_with_abi(&data, &manifest, "Status");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid variant discriminant"));
    }

    #[test]
    fn test_deserialize_error_incomplete_data() {
        use calimero_wasm_abi::schema::Field;

        let manifest = create_manifest_with_type(
            "Person",
            TypeDef::Record {
                fields: vec![
                    Field {
                        name: "name".to_owned(),
                        type_: TypeRef::string(),
                        nullable: None,
                    },
                    Field {
                        name: "age".to_owned(),
                        type_: TypeRef::u32(),
                        nullable: None,
                    },
                ],
            },
        );

        // Only serialize the name, missing age
        let mut data = Vec::new();
        "Alice".to_owned().serialize(&mut data).unwrap();

        let result = deserialize_with_abi(&data, &manifest, "Person");
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_error_extra_data() {
        let manifest = create_manifest_with_type(
            "U32Type",
            TypeDef::Alias {
                target: TypeRef::u32(),
            },
        );

        // Serialize a u32 but with extra bytes
        let mut data = borsh::to_vec(&42_u32).unwrap();
        data.push(0xFF); // Extra byte

        let result = deserialize_with_abi(&data, &manifest, "U32Type");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("did not consume all bytes"));
    }

    #[test]
    fn test_deserialize_unit_type() -> Result<()> {
        let manifest = create_manifest_with_type(
            "UnitType",
            TypeDef::Alias {
                target: TypeRef::unit(),
            },
        );

        let data = Vec::new(); // Unit serializes to empty
        let result = deserialize_with_abi(&data, &manifest, "UnitType")?;
        assert_eq!(result, json!(null));

        Ok(())
    }

    #[test]
    fn test_deserialize_complex_nested_structure() -> Result<()> {
        use calimero_wasm_abi::schema::{Field, Variant};

        let mut manifest = Manifest::new();

        // Define a variant type for status
        drop(manifest.types.insert(
            "Status".to_owned(),
            TypeDef::Variant {
                variants: vec![
                    Variant {
                        name: "Pending".to_owned(),
                        code: None,
                        payload: None,
                    },
                    Variant {
                        name: "Completed".to_owned(),
                        code: None,
                        payload: Some(TypeRef::u32()),
                    },
                ],
            },
        ));

        // Define a record type with nested types
        drop(manifest.types.insert(
            "Task".to_owned(),
            TypeDef::Record {
                fields: vec![
                    Field {
                        name: "id".to_owned(),
                        type_: TypeRef::u32(),
                        nullable: None,
                    },
                    Field {
                        name: "tags".to_owned(),
                        type_: TypeRef::list(TypeRef::string()),
                        nullable: None,
                    },
                    Field {
                        name: "status".to_owned(),
                        type_: TypeRef::reference("Status"),
                        nullable: None,
                    },
                ],
            },
        ));

        // Serialize: Task { id: 1, tags: ["urgent", "bug"], status: Completed(100) }
        let mut data = Vec::new();
        1_u32.serialize(&mut data)?; // id
        vec!["urgent".to_owned(), "bug".to_owned()].serialize(&mut data)?; // tags
        1_u32.serialize(&mut data)?; // status discriminant (Completed)
        100_u32.serialize(&mut data)?; // status payload

        let result = deserialize_with_abi(&data, &manifest, "Task")?;
        assert_eq!(
            result,
            json!({
                "id": 1,
                "tags": ["urgent", "bug"],
                "status": {
                    "variant": "Completed",
                    "payload": 100
                }
            })
        );

        Ok(())
    }

    #[test]
    fn test_deserialize_empty_list() -> Result<()> {
        let manifest = create_manifest_with_type(
            "EmptyList",
            TypeDef::Alias {
                target: TypeRef::list(TypeRef::string()),
            },
        );

        let empty_list: Vec<String> = vec![];
        let data = borsh::to_vec(&empty_list)?;

        let result = deserialize_with_abi(&data, &manifest, "EmptyList")?;
        assert_eq!(result, json!([]));

        Ok(())
    }

    #[test]
    fn test_deserialize_empty_map() -> Result<()> {
        let manifest = create_manifest_with_type(
            "EmptyMap",
            TypeDef::Alias {
                target: TypeRef::map(TypeRef::u32()),
            },
        );

        // Serialize empty map
        let mut data = Vec::new();
        0_u32.serialize(&mut data)?; // length = 0

        let result = deserialize_with_abi(&data, &manifest, "EmptyMap")?;
        assert_eq!(result, json!({}));

        Ok(())
    }
}
