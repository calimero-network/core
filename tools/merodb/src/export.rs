use calimero_wasm_abi::schema::Manifest;
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};

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

/// Parse a value with optional ABI-guided deserialization
fn parse_value_with_abi(
    column: Column,
    value: &[u8],
    abi_manifest: Option<&Manifest>,
) -> Result<Value> {
    match column {
        Column::State => abi_manifest.map_or_else(
            || {
                // No ABI, use default parsing
                parse_value(column, value)
            },
            |manifest| {
                // Try to deserialize with ABI
                match deserializer::deserialize_root_state(value, manifest) {
                    Ok(deserialized) => Ok(json!({
                        "deserialized": deserialized,
                        "raw_size": value.len()
                    })),
                    Err(e) => {
                        // Fall back to hex on error
                        Ok(json!({
                            "error": format!("Failed to deserialize: {e}"),
                            "hex": hex::encode(value),
                            "size": value.len()
                        }))
                    }
                }
            },
        ),
        Column::Delta => abi_manifest.map_or_else(
            || {
                // No ABI, use default parsing
                parse_value(column, value)
            },
            |manifest| {
                // Delta can contain state data, but we need to handle the Delta enum first
                use borsh::BorshDeserialize;
                use calimero_store::types::ContextDelta as StoreContextDelta;

                StoreContextDelta::try_from_slice(value).map_or_else(
                    |_| parse_value(column, value),
                    |delta| match delta {
                        StoreContextDelta::Head(size) => Ok(json!({
                            "type": "head",
                            "size": size.get()
                        })),
                        StoreContextDelta::Data(bytes) => {
                            // Try to deserialize the delta data with ABI
                            match deserializer::deserialize_root_state(&bytes, manifest) {
                                Ok(deserialized) => Ok(json!({
                                    "type": "data",
                                    "deserialized": deserialized,
                                    "raw_size": bytes.len()
                                })),
                                Err(e) => {
                                    // Fall back to hex on error
                                    Ok(json!({
                                        "type": "data",
                                        "error": format!("Failed to deserialize: {e}"),
                                        "hex": hex::encode(bytes.as_ref()),
                                        "size": bytes.len()
                                    }))
                                }
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
