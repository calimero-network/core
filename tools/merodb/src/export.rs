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
    let path =
        String::deserialize_reader(&mut cursor).wrap_err("Failed to deserialize entry path")?;

    if cursor.position() != bytes.len() as u64 {
        eyre::bail!("Entry bytes not fully consumed");
    }
    Ok(json!({
        "field": field.name.clone(),
        "element": {
            "id": hex::encode(element_id),
            "path": path
        },
        "key": {
            "parsed": key_value,
            "raw_hex": hex::encode(key_raw),
            "type": type_ref_label(&field.key_type)
        },
        "value": {
            "parsed": value_value,
            "raw_hex": hex::encode(value_raw),
            "type": type_ref_label(&field.value_type)
        }
    }))
}

fn decode_state_entry(value: &[u8], manifest: &Manifest) -> Option<Value> {
    for field in map_fields(manifest) {
        if let Ok(decoded) = decode_map_entry(value, &field, manifest) {
            return Some(decoded);
        }
    }

    None
}

/// Parse a value with the ABI manifest present
fn parse_value_with_abi(column: Column, value: &[u8], manifest: &Manifest) -> Result<Value> {
    match column {
        Column::State => {
            if let Some(decoded) = decode_state_entry(value, manifest) {
                return Ok(decoded);
            }

            Ok(json!({
                "raw_hex": hex::encode(value),
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
                        return Ok(json!({
                            "type": "context_dag_delta",
                            "delta_id": hex::encode(delta.delta_id),
                            "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                            "actions": {
                                "parsed": parsed,
                                "raw_hex": hex::encode(&delta.actions)
                            },
                            "timestamp": delta.timestamp,
                            "applied": delta.applied
                        }));
                    }
                }

                return Ok(json!({
                    "type": "context_dag_delta",
                    "delta_id": hex::encode(delta.delta_id),
                    "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                    "actions": {
                        "raw_hex": hex::encode(&delta.actions),
                        "note": "Unable to decode actions with ABI"
                    },
                    "timestamp": delta.timestamp,
                    "applied": delta.applied
                }));
            }

            Ok(json!({
                "raw_hex": hex::encode(value),
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
