pub mod cli;

use borsh::BorshDeserialize;
use calimero_store::types::ContextDagDelta as StoreContextDagDelta;
use calimero_wasm_abi::schema::{CollectionType, Field, Manifest, TypeDef, TypeRef};
use core::convert::TryFrom;
use core::ops::Deref;
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

    let key_value =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.key_type, manifest)?;
    let key_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let key_raw = bytes[..key_end].to_vec();

    let value_value =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.value_type, manifest)?;
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

fn decode_state_entry(bytes: &[u8], manifest: &Manifest) -> Option<Value> {
    // Try to decode as EntityIndex first (these are smaller, metadata-only)
    if let Ok(index) = borsh::from_slice::<EntityIndex>(bytes) {
        return Some(json!({
            "type": "EntityIndex",
            "id": String::from_utf8_lossy(index.id.as_bytes()),
            "parent_id": index.parent_id.map(|id| String::from_utf8_lossy(id.as_bytes()).to_string()),
            "children_count": index.children.as_ref().map_or(0, Vec::len),
            "full_hash": String::from_utf8_lossy(&index.full_hash),
            "own_hash": String::from_utf8_lossy(&index.own_hash),
            "created_at": index.metadata.created_at,
            "updated_at": *index.metadata.updated_at,
            "deleted_at": index.deleted_at
        }));
    }

    // Check if it's just a raw ID (32 bytes)
    if bytes.len() == 32 {
        if let Ok(id) = borsh::from_slice::<Id>(bytes) {
            return Some(json!({
                "type": "RawId",
                "id": String::from_utf8_lossy(id.as_bytes()),
                "note": "Direct ID storage (possibly root collection reference or internal metadata)"
            }));
        }
    }

    // Get all fields from the state root
    let root_name = manifest.state_root.as_ref()?;
    let Some(TypeDef::Record {
        fields: record_fields,
    }) = manifest.types.get(root_name)
    else {
        return None;
    };

    // Try to decode as map entry (Entry<(K, V)>)
    for field in record_fields {
        if let TypeRef::Collection(CollectionType::Map { key, value }) = &field.type_ {
            let map_field = MapField {
                name: field.name.clone(),
                key_type: (**key).clone(),
                value_type: (**value).clone(),
            };
            if let Ok(decoded) = decode_map_entry(bytes, &map_field, manifest) {
                return Some(decoded);
            }
        }
    }

    // Try to decode as scalar entry (Entry<T> where T is a scalar/enum/reference)
    for field in record_fields {
        // Skip map fields (already tried above)
        if matches!(
            &field.type_,
            TypeRef::Collection(CollectionType::Map { .. })
        ) {
            continue;
        }

        if let Ok(decoded) = decode_scalar_entry(bytes, field, manifest) {
            return Some(decoded);
        }
    }

    None
}

fn decode_scalar_entry(bytes: &[u8], field: &Field, manifest: &Manifest) -> Result<Value> {
    let mut cursor = Cursor::new(bytes);

    // Deserialize the value (not a tuple, just the value itself)
    let value_parsed =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.type_, manifest)?;
    let value_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let value_raw = bytes[..value_end].to_vec();

    // Read the Element ID
    let mut element_id = [0_u8; 32];
    cursor
        .read_exact(&mut element_id)
        .wrap_err("Failed to read entry element id")?;

    if cursor.position() != bytes.len() as u64 {
        eyre::bail!("Entry bytes not fully consumed");
    }

    Ok(json!({
        "type": "ScalarEntry",
        "field": field.name.clone(),
        "element": {
            "id": String::from_utf8_lossy(&element_id)
        },
        "value": {
            "parsed": value_parsed,
            "raw": String::from_utf8_lossy(&value_raw),
            "type": type_ref_label(&field.type_)
        }
    }))
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
    const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

#[derive(borsh::BorshDeserialize)]
#[expect(
    dead_code,
    reason = "Fields required for Borsh deserialization structure"
)]
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

impl Deref for UpdatedAt {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Parse a value with the ABI manifest present
pub fn parse_value_with_abi(column: Column, value: &[u8], manifest: &Manifest) -> Result<Value> {
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
                            "delta_id": hex::encode(delta.delta_id),
                            "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
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
                    "delta_id": hex::encode(delta.delta_id),
                    "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
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

/// Extract state tree structure starting from state_root
#[cfg(feature = "gui")]
pub fn extract_state_tree(
    db: &DBWithThreadMode<SingleThreaded>,
    manifest: &Manifest,
) -> Result<Value> {
    // Get Meta column to find contexts and their root hashes
    let meta_cf = db
        .cf_handle("Meta")
        .ok_or_else(|| eyre::eyre!("Meta column family not found"))?;

    let state_cf = db
        .cf_handle("State")
        .ok_or_else(|| eyre::eyre!("State column family not found"))?;

    let mut contexts = Vec::new();

    // Iterate through Meta column to find all contexts
    let iter = db.iterator_cf(&meta_cf, IteratorMode::Start);
    for item in iter {
        let (key, value) = item.wrap_err("Failed to read Meta entry")?;

        // Try to parse the key to get context_id
        let key_json = parse_key(Column::Meta, &key)?;
        let context_id = key_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Failed to extract context_id from Meta key"))?;

        // Parse the value to get root_hash
        let value_json = parse_value(Column::Meta, &value)?;
        let root_hash_str = value_json
            .get("root_hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Failed to extract root_hash from Meta value"))?;

        // Decode context_id from hex to bytes for State column lookup
        let context_id_bytes =
            hex::decode(context_id).wrap_err("Failed to decode context_id from hex")?;

        // Find the actual root node in the State column by iterating through entries
        // and finding one where parent_id == null
        let tree = find_and_build_tree_for_context(db, state_cf, &context_id_bytes, manifest)?;

        contexts.push(json!({
            "context_id": context_id,
            "root_hash": root_hash_str,
            "tree": tree
        }));
    }

    Ok(json!({
        "contexts": contexts,
        "total_contexts": contexts.len()
    }))
}

/// Find the root node for a context and build the tree
#[cfg(feature = "gui")]
fn find_and_build_tree_for_context(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    manifest: &Manifest,
) -> Result<Value> {
    use rocksdb::IteratorMode;

    // Iterate through State column to find entries for this context
    let iter = db.iterator_cf(state_cf, IteratorMode::Start);

    for item in iter {
        let (key, value) = item.wrap_err("Failed to read State entry")?;

        // Check if this key belongs to our context (first 32 bytes match context_id)
        if key.len() == 64 && &key[0..32] == context_id {
            // Try to decode as EntityIndex to check if it's a root node
            if let Ok(index) = borsh::from_slice::<EntityIndex>(&value) {
                // Found a root node (parent_id is None)
                if index.parent_id.is_none() {
                    // Extract state_key (last 32 bytes) and convert to hex
                    let state_key_hex = hex::encode(&key[32..64]);
                    // Build the tree from this root
                    return build_tree_from_root(
                        db,
                        state_cf,
                        context_id,
                        &state_key_hex,
                        manifest,
                    );
                }
            }
        }
    }

    // No root node found for this context
    Ok(json!({
        "id": "unknown",
        "type": "missing",
        "note": "No root node (parent_id == null) found in State column for this context"
    }))
}

/// Recursively build tree structure from a given root hash
#[cfg(feature = "gui")]
fn build_tree_from_root(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    node_id: &str,
    manifest: &Manifest,
) -> Result<Value> {
    // Decode the node_id (state key) from hex string
    let state_key = hex::decode(node_id).wrap_err("Failed to decode node_id from hex")?;

    // Construct composite key: context_id (32 bytes) + state_key (32 bytes) = 64 bytes
    let mut key = Vec::with_capacity(64);
    key.extend_from_slice(context_id);
    key.extend_from_slice(&state_key);
    let value_bytes = db
        .get_cf(state_cf, key)
        .wrap_err("Failed to query State column")?;

    let Some(value_bytes) = value_bytes else {
        return Ok(json!({
            "id": node_id,
            "type": "missing",
            "note": "Node not found in State column"
        }));
    };

    // Try to decode as EntityIndex
    if let Ok(index) = borsh::from_slice::<EntityIndex>(&value_bytes) {
        let children_info: Vec<Value> = if let Some(children) = &index.children {
            let mut child_nodes = Vec::new();
            for child in children {
                // Convert child id to hex string for consistent representation
                let child_id = hex::encode(child.id.as_bytes());
                let child_tree =
                    build_tree_from_root(db, state_cf, context_id, &child_id, manifest)?;
                child_nodes.push(child_tree);
            }
            child_nodes
        } else {
            Vec::new()
        };

        return Ok(json!({
            "id": node_id,
            "type": "EntityIndex",
            "parent_id": index.parent_id.map(|id| String::from_utf8_lossy(id.as_bytes()).to_string()),
            "full_hash": hex::encode(index.full_hash),
            "own_hash": hex::encode(index.own_hash),
            "created_at": index.metadata.created_at,
            "updated_at": *index.metadata.updated_at,
            "deleted_at": index.deleted_at,
            "children": children_info,
            "children_count": children_info.len()
        }));
    }

    // Try to decode as data entry
    if let Some(decoded) = decode_state_entry(&value_bytes, manifest) {
        return Ok(json!({
            "id": node_id,
            "type": "DataEntry",
            "data": decoded
        }));
    }

    // Fallback for unknown format
    Ok(json!({
        "id": node_id,
        "type": "Unknown",
        "size": value_bytes.len(),
        "raw": String::from_utf8_lossy(&value_bytes)
    }))
}

/// Export data without ABI manifest
#[cfg(feature = "gui")]
pub fn export_data_without_abi(
    db: &DBWithThreadMode<SingleThreaded>,
    columns: &[Column],
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

            let value_json = parse_value(*column, &value)
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
