use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};

use crate::types::{parse_key, parse_value, Column};

#[derive(Debug, Default)]
pub struct ValidationResult {
    pub total_entries: usize,
    pub valid_entries: usize,
    pub invalid_entries: usize,
    pub errors: Vec<ValidationError>,
}

#[derive(Debug)]
pub struct ValidationError {
    pub column: String,
    pub key_hex: String,
    pub error_type: ErrorType,
    pub message: String,
}

#[derive(Debug)]
pub enum ErrorType {
    InvalidKeySize,
    DeserializationError,
    UnexpectedData,
}

impl ErrorType {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidKeySize => "invalid_key_size",
            Self::DeserializationError => "deserialization_error",
            Self::UnexpectedData => "unexpected_data",
        }
    }
}

/// Validate the database integrity
pub fn validate_database(db: &DBWithThreadMode<SingleThreaded>) -> Result<Value> {
    let mut overall_result = ValidationResult::default();
    let mut column_results = serde_json::Map::new();

    for column in Column::all() {
        let result = validate_column(db, *column)?;

        overall_result.total_entries =
            overall_result.total_entries.saturating_add(result.total_entries);
        overall_result.valid_entries =
            overall_result.valid_entries.saturating_add(result.valid_entries);
        overall_result.invalid_entries = overall_result
            .invalid_entries
            .saturating_add(result.invalid_entries);
        overall_result.errors.extend(result.errors);

        drop(column_results.insert(
            column.as_str().to_owned(),
            json!({
                "total_entries": result.total_entries,
                "valid_entries": result.valid_entries,
                "invalid_entries": result.invalid_entries,
                "error_count": result.invalid_entries
            }),
        ));
    }

    let errors_json: Vec<Value> = overall_result
        .errors
        .iter()
        .map(|e| {
            json!({
                "column": e.column,
                "key": e.key_hex,
                "error_type": e.error_type.as_str(),
                "message": e.message
            })
        })
        .collect();

    Ok(json!({
        "validation_result": {
            "status": if overall_result.invalid_entries == 0 { "passed" } else { "failed" },
            "total_entries": overall_result.total_entries,
            "valid_entries": overall_result.valid_entries,
            "invalid_entries": overall_result.invalid_entries
        },
        "column_results": column_results,
        "errors": errors_json
    }))
}

fn validate_column(
    db: &DBWithThreadMode<SingleThreaded>,
    column: Column,
) -> Result<ValidationResult> {
    let mut result = ValidationResult::default();
    let cf_name = column.as_str();

    let cf = db
        .cf_handle(cf_name)
        .ok_or_else(|| eyre::eyre!("Column family '{}' not found", cf_name))?;

    let iter = db.iterator_cf(&cf, IteratorMode::Start);

    for item in iter {
        let (key, value) = item.wrap_err_with(|| {
            format!("Failed to read entry from column family '{cf_name}'")
        })?;

        result.total_entries = result.total_entries.saturating_add(1);

        // Validate key size
        let expected_key_size = column.key_size();
        if key.len() != expected_key_size {
            result.invalid_entries = result.invalid_entries.saturating_add(1);
            result.errors.push(ValidationError {
                column: cf_name.to_owned(),
                key_hex: hex::encode(&key),
                error_type: ErrorType::InvalidKeySize,
                message: format!(
                    "Expected key size {expected_key_size} bytes, found {} bytes",
                    key.len()
                ),
            });
            continue;
        }

        // Try to parse the key
        match parse_key(column, &key) {
            Ok(key_json) => {
                if key_json.get("error").is_some() {
                    result.invalid_entries = result.invalid_entries.saturating_add(1);
                    result.errors.push(ValidationError {
                        column: cf_name.to_owned(),
                        key_hex: hex::encode(&key),
                        error_type: ErrorType::UnexpectedData,
                        message: format!("Key parsing reported error: {key_json}"),
                    });
                    continue;
                }
            }
            Err(e) => {
                result.invalid_entries = result.invalid_entries.saturating_add(1);
                result.errors.push(ValidationError {
                    column: cf_name.to_owned(),
                    key_hex: hex::encode(&key),
                    error_type: ErrorType::DeserializationError,
                    message: format!("Failed to parse key: {e}"),
                });
                continue;
            }
        }

        // Try to parse the value
        match parse_value(column, &value) {
            Ok(value_json) => {
                if value_json.get("error").is_some() {
                    result.invalid_entries = result.invalid_entries.saturating_add(1);
                    result.errors.push(ValidationError {
                        column: cf_name.to_owned(),
                        key_hex: hex::encode(&key),
                        error_type: ErrorType::DeserializationError,
                        message: format!("Value parsing reported error: {value_json}"),
                    });
                    continue;
                }
            }
            Err(e) => {
                result.invalid_entries = result.invalid_entries.saturating_add(1);
                result.errors.push(ValidationError {
                    column: cf_name.to_owned(),
                    key_hex: hex::encode(&key),
                    error_type: ErrorType::DeserializationError,
                    message: format!("Failed to parse value: {e}"),
                });
                continue;
            }
        }

        result.valid_entries = result.valid_entries.saturating_add(1);
    }

    Ok(result)
}
