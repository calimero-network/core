use borsh::BorshDeserialize;
use calimero_store::types::ContextDagDelta as StoreContextDagDelta;
use calimero_wasm_abi::schema::{CollectionType, Manifest, TypeDef, TypeRef};
use core::convert::TryFrom;
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};
use std::io::{Cursor, Read};

use crate::deserializer;
use crate::types::{parse_key, parse_value, Column};

/// Export data from RocksDB to JSON
pub fn export_data(
    db: &DBWithThreadMode<SingleThreaded>,
    columns: &[Column],
    manifest: &Manifest,
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

            let value_json = parse_value_with_abi(*column, &value, manifest)
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

fn map_fields(manifest: &Manifest) -> Vec<MapField> {
    let mut fields = Vec::new();
    let Some(root_name) = manifest.state_root.as_ref() else {
        return fields;
    };

    let Some(TypeDef::Record {
        fields: record_fields,
    }) = manifest.types.get(root_name)
    else {
        return fields;
    };

    for field in record_fields {
        if let TypeRef::Collection(CollectionType::Map { key, value }) = &field.type_ {
            fields.push(MapField {
                name: field.name.clone(),
                key_type: *key.clone(),
                value_type: *value.clone(),
            });
        }
    }

    fields
}

fn delta_hlc_snapshot(delta: &StoreContextDagDelta) -> (u64, Value) {
    let timestamp = delta.hlc.inner();
    let raw_time = timestamp.get_time().as_u64();
    let id_hex = format!("{:032x}", u128::from(*timestamp.get_id()));
    let physical_seconds = (raw_time >> 32_i32) as u32;
    let logical_counter = (raw_time & 0xF) as u32;

    let hlc_json = json!({
        "raw": delta.hlc.to_string(),
        "time_ntp64": raw_time,
        "physical_time_secs": physical_seconds,
        "logical_counter": logical_counter,
        "id_hex": id_hex,
    });

    (raw_time, hlc_json)
}

#[derive(Clone)]
struct MapField {
    name: String,
    key_type: TypeRef,
    value_type: TypeRef,
}

fn type_ref_label(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Reference { ref_ } => ref_.clone(),
        TypeRef::Scalar(s) => format!("scalar::{s:?}"),
        TypeRef::Collection(c) => format!("collection::{c:?}"),
    }
}

fn decode_map_entry(bytes: &[u8], field: &MapField, manifest: &Manifest) -> Result<Value> {
    let mut cursor = Cursor::new(bytes);

    let key_value = deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.key_type, manifest)?;
    let key_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let key_raw = bytes[..key_end].to_vec();

    let value_value = deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.value_type, manifest)?;
    let value_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let value_raw = bytes[key_end..value_end].to_vec();

    let mut element_id = [0_u8; 32];
    cursor
        .read_exact(&mut element_id)
        .wrap_err("Failed to read entry element id")?;
    // Note: path field was removed from Element struct in commit 301886bb
    // The serialized format is now: (K, V, element_id) without the path field

    if cursor.position() != bytes.len() as u64 {
        eyre::bail!("Entry bytes not fully consumed");
    }

    Ok(json!({
        "type": "Entry",
        "field": field.name.clone(),
        "element": {
            "id": String::from_utf8_lossy(&element_id)
        },
        "key": {
            "parsed": key_value,
            "raw": String::from_utf8_lossy(&key_raw),
            "type": type_ref_label(&field.key_type)
        },
        "value": {
            "parsed": value_value,
            "raw": String::from_utf8_lossy(&value_raw),
            "type": type_ref_label(&field.value_type)
        }
    }))
}

fn decode_state_entry(value: &[u8], manifest: &Manifest) -> Option<Value> {
    // Try to decode as EntityIndex first (these are smaller, metadata-only)
    // EntityIndex structure:
    // - id: Id (32 bytes)
    // - parent_id: Option<Id> (1 byte discriminant + maybe 32 bytes)
    // - children: Option<Vec<ChildInfo>>
    // - full_hash: [u8; 32]
    // - own_hash: [u8; 32]
    // - metadata: Metadata
    // - deleted_at: Option<u64>

    // Try to decode as EntityIndex - this will tell us if it's an Index entry
    if let Ok(index) = borsh::from_slice::<EntityIndex>(value) {
        return Some(json!({
            "type": "EntityIndex",
            "id": String::from_utf8_lossy(index.id.as_bytes()),
            "parent_id": index.parent_id.map(|id| String::from_utf8_lossy(id.as_bytes()).to_string()),
            "children_count": index.children.as_ref().map(|c| c.len()).unwrap_or(0),
            "full_hash": String::from_utf8_lossy(&index.full_hash),
            "own_hash": String::from_utf8_lossy(&index.own_hash),
            "created_at": index.metadata.created_at,
            "updated_at": *index.metadata.updated_at,
            "deleted_at": index.deleted_at
        }));
    }

    // Try to decode as map entry (Entry<(K, V)>)
    let fields = map_fields(manifest);
    if fields.is_empty() {
        return None;
    }

    for field in fields {
        if let Ok(decoded) = decode_map_entry(value, &field, manifest) {
            return Some(decoded);
        }
    }

    None
}

// EntityIndex structure for decoding
#[derive(borsh::BorshDeserialize)]
struct EntityIndex {
    id: Id,
    parent_id: Option<Id>,
    children: Option<Vec<ChildInfo>>,
    full_hash: [u8; 32],
    own_hash: [u8; 32],
    metadata: Metadata,
    deleted_at: Option<u64>,
}

#[derive(borsh::BorshDeserialize)]
struct Id {
    bytes: [u8; 32],
}

impl Id {
    fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

#[derive(borsh::BorshDeserialize)]
struct ChildInfo {
    id: Id,
    merkle_hash: [u8; 32],
    metadata: Metadata,
}

#[derive(borsh::BorshDeserialize)]
struct Metadata {
    created_at: u64,
    updated_at: UpdatedAt,
}

#[derive(borsh::BorshDeserialize)]
struct UpdatedAt(u64);

impl std::ops::Deref for UpdatedAt {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Parse a value with the ABI manifest present
fn parse_value_with_abi(column: Column, value: &[u8], manifest: &Manifest) -> Result<Value> {
    match column {
        Column::State => {
            if let Some(decoded) = decode_state_entry(value, manifest) {
                return Ok(decoded);
            }

            Ok(json!({
                "raw": String::from_utf8_lossy(value),
                "size": value.len(),
                "note": "Unable to decode with ABI"
            }))
        }
        Column::Generic => {
            if let Ok(delta) = StoreContextDagDelta::try_from_slice(value) {
                if let Some(root) = manifest.state_root.as_ref() {
                    if let Ok(parsed) =
                        deserializer::deserialize_with_abi(&delta.actions, manifest, root)
                    {
                        let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
                        return Ok(json!({
                            "type": "context_dag_delta",
                            "delta_id": String::from_utf8_lossy(&delta.delta_id),
                            "parents": delta.parents.iter().map(|p| String::from_utf8_lossy(p).to_string()).collect::<Vec<_>>(),
                            "actions": {
                                "parsed": parsed,
                                "raw": String::from_utf8_lossy(&delta.actions)
                            },
                            "timestamp": timestamp_raw,
                            "hlc": hlc_json,
                            "applied": delta.applied
                        }));
                    }
                }

                let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
                return Ok(json!({
                    "type": "context_dag_delta",
                    "delta_id": String::from_utf8_lossy(&delta.delta_id),
                    "parents": delta.parents.iter().map(|p| String::from_utf8_lossy(p).to_string()).collect::<Vec<_>>(),
                    "actions": {
                        "raw": String::from_utf8_lossy(&delta.actions),
                        "note": "Unable to decode actions with ABI"
                    },
                    "timestamp": timestamp_raw,
                    "hlc": hlc_json,
                    "applied": delta.applied
                }));
            }

            Ok(json!({
                "raw": String::from_utf8_lossy(value),
                "size": value.len(),
                "note": "Unable to decode with ABI"
            }))
        }
        _ => {
            // For other columns, use default parsing
            parse_value(column, value)
        }
    }
}
