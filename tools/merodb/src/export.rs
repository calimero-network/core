use borsh::BorshDeserialize;
use calimero_store::types::ContextDagDelta as StoreContextDagDelta;
use calimero_wasm_abi::schema::Manifest;
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;

use crate::deserializer;
use crate::types::{parse_key, parse_value, Column};

/// Export data from RocksDB to JSON
pub fn export_data(
    db: &DBWithThreadMode<SingleThreaded>,
    columns: &[Column],
    abi_manifest: Option<&Manifest>,
) -> Result<Value> {
    let mut data = serde_json::Map::new();

    for column in columns {
        let cf_name = column.as_str();
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| eyre::eyre!("Column family '{cf_name}' not found"))?;

        let mut entries = Vec::new();
        let iter = db.iterator_cf(&cf, IteratorMode::Start);

        for item in iter {
            let (key, value) = item
                .wrap_err_with(|| format!("Failed to read entry from column family '{cf_name}'"))?;

            let key_json = parse_key(*column, &key)
                .wrap_err_with(|| format!("Failed to parse key in column '{cf_name}'"))?;

            let value_json = parse_value_with_abi(*column, &value, abi_manifest)
                .wrap_err_with(|| format!("Failed to parse value in column '{cf_name}'"))?;

            entries.push(json!({
                "key": key_json,
                "value": value_json
            }));
        }

        drop(data.insert(
            cf_name.to_owned(),
            json!({
                "count": entries.len(),
                "entries": entries
            }),
        ));
    }

    Ok(json!({
        "database": "Calimero RocksDB Export",
        "exported_columns": columns.iter().map(Column::as_str).collect::<Vec<_>>(),
        "data": data
    }))
}

fn extract_state_entry_parts(value: &[u8]) -> Option<(&[u8], &[u8], String)> {
    let total = value.len();
    if total < 36 {
        return None;
    }

    // Iterate possible path lengths (from shortest to longest)
    for path_len in 0..=(total - 32 - 4) {
        let path_start = total - path_len;
        if path_start < 4 {
            break;
        }
        let len_pos = path_start - 4;
        if len_pos < 32 {
            break;
        }
        let len_bytes = &value[len_pos..path_start];
        let len = u32::from_le_bytes(len_bytes.try_into().ok()?) as usize;
        if len != path_len {
            continue;
        }
        let id_start = len_pos - 32;
        if id_start > total {
            continue;
        }
        let path_bytes = &value[path_start..];
        if let Ok(path) = std::str::from_utf8(path_bytes) {
            let item_bytes = &value[..id_start];
            let element_id = &value[id_start..len_pos];
            return Some((item_bytes, element_id, path.to_owned()));
        }
    }

    None
}

fn decode_key_prefix(bytes: &[u8]) -> Option<(Value, usize, String)> {
    let mut cursor = Cursor::new(bytes);

    if let Ok(value) = String::deserialize_reader(&mut cursor) {
        return Some((
            json!(value),
            cursor.position() as usize,
            "string".to_owned(),
        ));
    }

    cursor.set_position(0);
    if let Ok(value) = u64::deserialize_reader(&mut cursor) {
        return Some((json!(value), cursor.position() as usize, "u64".to_owned()));
    }

    cursor.set_position(0);
    if let Ok(value) = i64::deserialize_reader(&mut cursor) {
        return Some((json!(value), cursor.position() as usize, "i64".to_owned()));
    }

    cursor.set_position(0);
    if let Ok(value) = u32::deserialize_reader(&mut cursor) {
        return Some((json!(value), cursor.position() as usize, "u32".to_owned()));
    }

    cursor.set_position(0);
    if let Ok(value) = i32::deserialize_reader(&mut cursor) {
        return Some((json!(value), cursor.position() as usize, "i32".to_owned()));
    }

    cursor.set_position(0);
    if let Ok(value) = bool::deserialize_reader(&mut cursor) {
        return Some((json!(value), cursor.position() as usize, "bool".to_owned()));
    }

    cursor.set_position(0);
    if let Ok(value) = Vec::<u8>::deserialize_reader(&mut cursor) {
        return Some((
            json!({
                "bytes_hex": hex::encode(&value),
                "length": value.len()
            }),
            cursor.position() as usize,
            "bytes".to_owned(),
        ));
    }

    None
}

fn try_deserialize_with_manifest(
    value_bytes: &[u8],
    manifest: &Manifest,
) -> Option<(String, Value)> {
    for (type_name, _) in &manifest.types {
        if let Ok(parsed) = deserializer::deserialize_with_abi(value_bytes, manifest, type_name) {
            return Some((type_name.clone(), parsed));
        }
    }
    None
}

#[derive(BorshDeserialize)]
struct RawUpdatedAt(pub u64);

#[derive(BorshDeserialize)]
struct RawMetadata {
    pub created_at: u64,
    pub updated_at: RawUpdatedAt,
}

#[derive(BorshDeserialize)]
struct RawChildInfo {
    pub id: [u8; 32],
    pub merkle_hash: [u8; 32],
    pub metadata: RawMetadata,
}

#[derive(BorshDeserialize)]
struct RawEntityIndex {
    pub id: [u8; 32],
    pub parent_id: Option<[u8; 32]>,
    pub children: BTreeMap<String, Vec<RawChildInfo>>,
    pub full_hash: [u8; 32],
    pub own_hash: [u8; 32],
    pub metadata: RawMetadata,
    pub deleted_at: Option<u64>,
}

fn try_parse_entity_index(value: &[u8]) -> Option<Value> {
    let index = RawEntityIndex::try_from_slice(value).ok()?;

    let mut children_map = serde_json::Map::new();
    for (collection, child_infos) in index.children {
        let entries = child_infos
            .into_iter()
            .map(|child| {
                json!({
                    "id": hex::encode(child.id),
                    "merkle_hash": hex::encode(child.merkle_hash),
                    "metadata": {
                        "created_at": child.metadata.created_at,
                        "updated_at": child.metadata.updated_at.0
                    }
                })
            })
            .collect::<Vec<_>>();
        let _previous = children_map.insert(collection, Value::Array(entries));
    }

    let parent_id = index.parent_id.map(hex::encode);

    Some(json!({
        "raw_size": value.len(),
        "note": "Storage entity index metadata",
        "entity_index": {
            "id": hex::encode(index.id),
            "parent_id": parent_id,
            "own_hash": hex::encode(index.own_hash),
            "full_hash": hex::encode(index.full_hash),
            "metadata": {
                "created_at": index.metadata.created_at,
                "updated_at": index.metadata.updated_at.0
            },
            "deleted_at": index.deleted_at,
            "children": children_map
        }
    }))
}

/// Parse a value with optional ABI-guided deserialization
fn parse_value_with_abi(
    column: Column,
    value: &[u8],
    abi_manifest: Option<&Manifest>,
) -> Result<Value> {
    match column {
        Column::State => {
            if let Some((item_bytes, element_id, path)) = extract_state_entry_parts(value) {
                if item_bytes.is_empty() {
                    return Ok(json!({
                        "raw_size": value.len(),
                        "element": {
                            "id": hex::encode(element_id),
                            "path": path
                        },
                        "note": "Collection metadata (no payload)"
                    }));
                }

                if let Some((key_json, consumed, key_type)) = decode_key_prefix(item_bytes) {
                    let key_bytes = &item_bytes[..consumed];
                    let value_bytes = &item_bytes[consumed..];

                    let (value_parsed, value_note, abi_type) = abi_manifest.map_or_else(|| (
                            deserializer::parse_borsh_generic(value_bytes),
                            "No ABI provided; using generic Borsh deserialization".to_owned(),
                            None,
                        ), |manifest| if let Some((type_name, parsed)) =
                            try_deserialize_with_manifest(value_bytes, manifest)
                        {
                            (
                                parsed,
                                format!("ABI-guided deserialization using type '{type_name}'"),
                                Some(type_name),
                            )
                        } else {
                            (
                                deserializer::parse_borsh_generic(value_bytes),
                                "ABI did not match; using generic Borsh deserialization".to_owned(),
                                None,
                            )
                        });

                    return Ok(json!({
                        "raw_size": value.len(),
                        "element": {
                            "id": hex::encode(element_id),
                            "path": path
                        },
                        "key": {
                            "parsed": key_json,
                            "raw_hex": hex::encode(key_bytes),
                            "type": key_type
                        },
                        "value": {
                            "parsed": value_parsed,
                            "raw_hex": hex::encode(value_bytes),
                            "size": value_bytes.len(),
                            "abi_type": abi_type,
                            "note": value_note
                        }
                    }));
                }
            }

            if let Some(entity_index) = try_parse_entity_index(value) {
                return Ok(entity_index);
            }

            if let Some(manifest) = abi_manifest {
                match deserializer::deserialize_root_state(value, manifest) {
                    Ok(deserialized) => {
                        Ok(json!({
                            "parsed": deserialized,
                            "raw_size": value.len(),
                            "note": "ABI-guided deserialization with field names"
                        }))
                    }
                    Err(e) => {
                        let parsed = deserializer::parse_borsh_generic(value);
                        Ok(json!({
                            "parsed": parsed,
                            "raw_size": value.len(),
                            "note": format!("ABI deserialization failed ({e}), using generic fallback")
                        }))
                    }
                }
            } else {
                let parsed = deserializer::parse_borsh_generic(value);
                Ok(json!({
                    "parsed": parsed,
                    "raw_size": value.len(),
                    "note": "Generic Borsh deserialization - actual types may differ"
                }))
            }
        }

        Column::Generic => abi_manifest.map_or_else(
            || {
                // No ABI, use default parsing
                parse_value(column, value)
            },
            |manifest| {
                // Generic column can contain ContextDagDelta entries
                // Try to parse as ContextDagDelta and deserialize its actions with ABI
                StoreContextDagDelta::try_from_slice(value).map_or_else(
                    |_| parse_value(column, value),
                    |delta| {
                        // Try to deserialize the actions data with ABI
                        match deserializer::deserialize_root_state(&delta.actions, manifest) {
                            Ok(deserialized) => Ok(json!({
                                "type": "context_dag_delta",
                                "delta_id": hex::encode(delta.delta_id),
                                "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                                "actions": {
                                    "deserialized": deserialized,
                                    "raw_size": delta.actions.len()
                                },
                                "timestamp": delta.timestamp,
                                "applied": delta.applied
                            })),
                            Err(e) => {
                                // Fall back to hex on error
                                Ok(json!({
                                    "type": "context_dag_delta",
                                    "delta_id": hex::encode(delta.delta_id),
                                    "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                                    "actions": {
                                        "error": format!("Failed to deserialize: {e}"),
                                        "hex": hex::encode(&delta.actions),
                                        "size": delta.actions.len()
                                    },
                                    "timestamp": delta.timestamp,
                                    "applied": delta.applied
                                }))
                            }
                        }
                    },
                )
            },
        ),
        _ => {
            // For other columns, use default parsing
            parse_value(column, value)
        }
    }
}
