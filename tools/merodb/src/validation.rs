pub mod cli;

use core::str;

use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};

use crate::types::{parse_value, Column};

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

/// Validate the database integrity by performing comprehensive checks on all columns.
///
/// This function validates the entire database by iterating through all column families
/// and checking each key-value pair for structural and data integrity issues.
///
/// # Validation Process
///
/// For each column family in the database, the validator performs the following checks:
///
/// 1. **Key Size Validation**
///    - Verifies that each key matches the expected byte length for its column type
///    - Different columns have different key size requirements:
///      - 32 bytes for Meta, Config, Blobs, Application (single ID)
///      - 64 bytes for Identity, Delta, State (compound keys)
///      - 83 bytes for Alias (kind + scope + name)
///      - 48 or 64 bytes for Generic (variable size)
///    - Reports `InvalidKeySize` errors for keys that don't match the expected size
///
/// 2. **Key Structure Validation**
///    - Validates the internal structure and components of keys:
///      - **Simple ID columns** (Meta, Config, Blobs, Application): Checks keys are not all zeros
///      - **Compound key columns** (Identity, Delta, State): Validates both 32-byte components are non-zero
///      - **Identity column**: Additionally validates that the public key portion is valid UTF-8
///      - **Alias column**: Validates kind byte (1-3), scope and name are non-zero
///      - **Generic column**: Validates key is not all zeros
///    - Reports `UnexpectedData` errors for structurally invalid keys (e.g., all-zero IDs)
///
/// 3. **Value Deserialization**
///    - Attempts to parse each value using the column-specific `parse_value()` function
///    - Validates that values can be properly deserialized according to their schema
///    - Reports `DeserializationError` if parsing fails or returns error metadata
///
/// # Return Value
///
/// Returns a JSON object with the following structure:
/// ```json
/// {
///   "validation_result": {
///     "status": "passed" | "failed",
///     "total_entries": <number>,
///     "valid_entries": <number>,
///     "invalid_entries": <number>
///   },
///   "column_results": {
///     "<column_name>": {
///       "total_entries": <number>,
///       "valid_entries": <number>,
///       "invalid_entries": <number>,
///       "error_count": <number>
///     },
///     ...
///   },
///   "errors": [
///     {
///       "column": "<column_name>",
///       "key": "<hex_representation>",
///       "error_type": "invalid_key_size" | "deserialization_error" | "unexpected_data",
///       "message": "<detailed_error_message>"
///     },
///     ...
///   ]
/// }
/// ```
///
/// # Usage in Migration Guards
///
/// This function is used by the `requires_validation` guard to ensure database integrity
/// before executing potentially destructive migration steps. If validation fails (status is
/// "failed" or invalid_entries > 0), the migration step is blocked from executing.
///
/// # Errors
///
/// Returns an error if:
/// - A column family cannot be accessed
/// - Iterator fails while reading entries
/// - Any other I/O error occurs during validation
pub fn validate_database(db: &DBWithThreadMode<SingleThreaded>) -> Result<Value> {
    let mut overall_result = ValidationResult::default();
    let mut column_results = serde_json::Map::new();

    for column in Column::all() {
        let result = validate_column(db, *column)?;

        overall_result.total_entries = overall_result
            .total_entries
            .saturating_add(result.total_entries);
        overall_result.valid_entries = overall_result
            .valid_entries
            .saturating_add(result.valid_entries);
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

#[expect(
    clippy::too_many_lines,
    reason = "Comprehensive validation requires detailed checks"
)]
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
        let (key, value) =
            item.wrap_err_with(|| format!("Failed to read entry from column family '{cf_name}'"))?;

        result.total_entries = result.total_entries.saturating_add(1);

        // Validate key size
        let expected_key_size = column.key_size();
        let size_valid = if matches!(column, Column::Generic) {
            // Generic column accepts 48 or 64 byte keys
            matches!(key.len(), 48 | 64)
        } else {
            key.len() == expected_key_size
        };

        if !size_valid {
            result.invalid_entries = result.invalid_entries.saturating_add(1);
            let message = if matches!(column, Column::Generic) {
                format!(
                    "Expected key size 48 or 64 bytes (Generic column), found {} bytes",
                    key.len()
                )
            } else {
                format!(
                    "Expected key size {expected_key_size} bytes, found {} bytes",
                    key.len()
                )
            };
            result.errors.push(ValidationError {
                column: cf_name.to_owned(),
                key_hex: hex::encode(&key),
                error_type: ErrorType::InvalidKeySize,
                message,
            });
            continue;
        }

        // Validate key structure
        // Check that key components are valid (non-zero for IDs, proper structure)
        let key_validation_error = match column {
            Column::Meta | Column::Config | Column::Blobs | Column::Application => {
                // Keys should be 32-byte IDs - check they're not all zeros
                key.iter()
                    .all(|&b| b == 0)
                    .then_some("Key is all zeros (invalid ID)")
            }
            Column::Identity | Column::Delta | Column::State => {
                // Keys have two 32-byte components - validate both parts
                let (first_part, second_part) = key.split_at(32);

                first_part
                    .iter()
                    .all(|&b| b == 0)
                    .then_some("First component (context_id) is all zeros")
                    .or_else(|| {
                        second_part
                            .iter()
                            .all(|&b| b == 0)
                            .then_some("Second component is all zeros")
                    })
                    .or_else(|| {
                        // For Identity column, validate UTF-8 in public key portion
                        (matches!(column, Column::Identity) && str::from_utf8(second_part).is_err())
                            .then_some("Public key portion contains invalid UTF-8")
                    })
            }
            Column::Alias => {
                // Alias keys: 1 byte kind + 32 byte scope + 50 byte name
                // Key length is 83 bytes (validated in size check above)
                let kind = key[0];
                (!matches!(kind, 1..=3))
                    .then_some("Invalid alias kind byte (must be 1, 2, or 3)")
                    .or_else(|| {
                        key[1..33]
                            .iter()
                            .all(|&b| b == 0)
                            .then_some("Scope is all zeros")
                    })
                    .or_else(|| {
                        key[33..83]
                            .iter()
                            .all(|&b| b == 0)
                            .then_some("Name is all zeros")
                    })
            }
            Column::Generic => {
                // Generic column has variable key size (48 or 64 bytes)
                // Size validation already happened, just check it's not all zeros
                key.iter().all(|&b| b == 0).then_some("Key is all zeros")
            }
        };

        if let Some(error_msg) = key_validation_error {
            result.invalid_entries = result.invalid_entries.saturating_add(1);
            result.errors.push(ValidationError {
                column: cf_name.to_owned(),
                key_hex: hex::encode(&key),
                error_type: ErrorType::UnexpectedData,
                message: error_msg.to_owned(),
            });
            continue;
        }

        // Try to parse the value
        match parse_value(column, &value) {
            Ok(value_json) => {
                if value_json.get("error").is_some() {
                    result.invalid_entries = result.invalid_entries.saturating_add(1);
                    result.errors.push(ValidationError {
                        column: cf_name.to_owned(),
                        key_hex: String::from_utf8_lossy(&key).to_string(),
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
                    key_hex: String::from_utf8_lossy(&key).to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rocksdb::{Options, WriteBatch};
    use tempfile::TempDir;

    /// Helper to create a test database with column families.
    ///
    /// Note: We use our own helper instead of `migration::test_utils::DbFixture` because:
    /// - Validation requires `DBWithThreadMode<SingleThreaded>` for read-only operations
    /// - Migration tests use regular `DB` (multi-threaded) for write operations
    /// - Our tests don't need Calimero-specific types or Borsh serialization
    fn create_test_db() -> (TempDir, DBWithThreadMode<SingleThreaded>) {
        let temp_dir = TempDir::new().unwrap();
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let column_names: Vec<_> = Column::all().iter().map(|c| c.as_str()).collect();

        let db = DBWithThreadMode::open_cf(&opts, temp_dir.path(), &column_names).unwrap();

        (temp_dir, db)
    }

    #[test]
    fn test_validation_passes_with_valid_entries() {
        let (_temp_dir, db) = create_test_db();

        // Use State column which accepts any data
        let cf = db.cf_handle(Column::State.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        let mut valid_key = vec![1u8; 32]; // Context ID: non-zero
        valid_key.extend_from_slice(&[2u8; 32]); // State key: non-zero
        let valid_value = b"test_value"; // State values are not validated structurally
        batch.put_cf(&cf, &valid_key, valid_value);

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::State).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.valid_entries, 1);
        assert_eq!(result.invalid_entries, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validation_detects_invalid_key_size() {
        let (_temp_dir, db) = create_test_db();

        // Add entry with wrong key size to Meta column
        let cf = db.cf_handle(Column::Meta.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        let invalid_key = vec![1u8; 16]; // Wrong size: 16 instead of 32
        let valid_value = b"test_value";
        batch.put_cf(&cf, &invalid_key, valid_value);

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Meta).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.valid_entries, 0);
        assert_eq!(result.invalid_entries, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(
            result.errors[0].error_type,
            ErrorType::InvalidKeySize
        ));
    }

    #[test]
    fn test_validation_detects_all_zero_keys() {
        let (_temp_dir, db) = create_test_db();

        // Add entry with all-zero key to Meta column
        let cf = db.cf_handle(Column::Meta.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        let zero_key = vec![0u8; 32]; // All zeros
        let valid_value = b"test_value";
        batch.put_cf(&cf, &zero_key, valid_value);

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Meta).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.valid_entries, 0);
        assert_eq!(result.invalid_entries, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(
            result.errors[0].error_type,
            ErrorType::UnexpectedData
        ));
        assert!(result.errors[0].message.contains("all zeros"));
    }

    #[test]
    fn test_validation_detects_zero_compound_key_components() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Identity.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Test case 1: First component (context_id) is all zeros
        let mut key1 = vec![0u8; 32]; // First 32 bytes: all zeros
        key1.extend_from_slice(b"valid_public_key_here_1234567890"); // Second 32 bytes: valid
        batch.put_cf(&cf, &key1, b"value1");

        // Test case 2: Second component is all zeros
        let mut key2 = vec![1u8; 32]; // First 32 bytes: valid
        key2.extend_from_slice(&[0u8; 32]); // Second 32 bytes: all zeros
        batch.put_cf(&cf, &key2, b"value2");

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Identity).unwrap();

        assert_eq!(result.total_entries, 2);
        assert_eq!(result.valid_entries, 0);
        assert_eq!(result.invalid_entries, 2);
        assert_eq!(result.errors.len(), 2);

        // Check error messages
        let messages: Vec<_> = result.errors.iter().map(|e| &e.message).collect();
        assert!(messages
            .iter()
            .any(|m| m.contains("First component (context_id) is all zeros")));
        assert!(messages
            .iter()
            .any(|m| m.contains("Second component is all zeros")));
    }

    #[test]
    fn test_validation_detects_invalid_utf8_in_identity_keys() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Identity.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Create key with invalid UTF-8 in public key portion
        let mut key = vec![1u8; 32]; // Valid context_id
        key.extend_from_slice(&[0xFF, 0xFE, 0xFD, 0xFC]); // Invalid UTF-8 bytes
        key.extend_from_slice(&[1u8; 28]); // Fill to 64 bytes total

        batch.put_cf(&cf, &key, b"value");
        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Identity).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.invalid_entries, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0]
            .message
            .contains("Public key portion contains invalid UTF-8"));
    }

    #[test]
    fn test_validation_detects_invalid_alias_kind_byte() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Alias.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Create alias key with invalid kind byte (4, but valid range is 1-3)
        let mut key = vec![4u8]; // Invalid kind
        key.extend_from_slice(&[1u8; 32]); // Scope
        key.extend_from_slice(&[b'n'; 50]); // Name

        batch.put_cf(&cf, &key, b"value");
        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Alias).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.invalid_entries, 1);
        assert!(result.errors[0]
            .message
            .contains("Invalid alias kind byte (must be 1, 2, or 3)"));
    }

    #[test]
    fn test_validation_detects_alias_zero_scope() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Alias.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Create alias key with zero scope
        let mut key = vec![1u8]; // Valid kind
        key.extend_from_slice(&[0u8; 32]); // Scope: all zeros
        key.extend_from_slice(&[b'n'; 50]); // Name

        batch.put_cf(&cf, &key, b"value");
        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Alias).unwrap();

        assert_eq!(result.invalid_entries, 1);
        assert!(result.errors[0].message.contains("Scope is all zeros"));
    }

    #[test]
    fn test_validation_detects_alias_zero_name() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Alias.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Create alias key with zero name
        let mut key = vec![1u8]; // Valid kind
        key.extend_from_slice(&[1u8; 32]); // Scope
        key.extend_from_slice(&[0u8; 50]); // Name: all zeros

        batch.put_cf(&cf, &key, b"value");
        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Alias).unwrap();

        assert_eq!(result.invalid_entries, 1);
        assert!(result.errors[0].message.contains("Name is all zeros"));
    }

    #[test]
    fn test_validation_accepts_generic_variable_sizes() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Generic.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Add both valid sizes for Generic column
        let key_48 = vec![1u8; 48]; // 48 bytes
        let key_64 = vec![1u8; 64]; // 64 bytes

        batch.put_cf(&cf, &key_48, b"value1");
        batch.put_cf(&cf, &key_64, b"value2");

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Generic).unwrap();

        assert_eq!(result.total_entries, 2);
        assert_eq!(result.valid_entries, 2);
        assert_eq!(result.invalid_entries, 0);
    }

    #[test]
    fn test_validation_rejects_generic_invalid_size() {
        let (_temp_dir, db) = create_test_db();

        let cf = db.cf_handle(Column::Generic.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        // Add invalid size for Generic column (not 48 or 64)
        let key_50 = vec![1u8; 50]; // Invalid size
        batch.put_cf(&cf, &key_50, b"value");

        db.write(batch).unwrap();

        // Validate
        let result = validate_column(&db, Column::Generic).unwrap();

        assert_eq!(result.total_entries, 1);
        assert_eq!(result.invalid_entries, 1);
        assert!(result.errors[0]
            .message
            .contains("Expected key size 48 or 64 bytes"));
    }

    #[test]
    fn test_validate_database_aggregates_results() {
        let (_temp_dir, db) = create_test_db();

        // Add valid entry to State (accepts any value)
        let cf_state = db.cf_handle(Column::State.as_str()).unwrap();
        let mut batch = WriteBatch::default();
        let mut valid_key = vec![1u8; 32];
        valid_key.extend_from_slice(&[2u8; 32]);
        batch.put_cf(&cf_state, &valid_key, b"valid");

        // Add invalid entry to Delta (all zeros in first component)
        let cf_delta = db.cf_handle(Column::Delta.as_str()).unwrap();
        let mut invalid_key = vec![0u8; 32]; // First component: all zeros (invalid)
        invalid_key.extend_from_slice(&[1u8; 32]); // Second component: valid
        batch.put_cf(&cf_delta, &invalid_key, b"invalid");

        db.write(batch).unwrap();

        // Validate entire database
        let result_json = validate_database(&db).unwrap();

        let status = result_json["validation_result"]["status"].as_str().unwrap();
        let total = result_json["validation_result"]["total_entries"]
            .as_u64()
            .unwrap();
        let valid = result_json["validation_result"]["valid_entries"]
            .as_u64()
            .unwrap();
        let invalid = result_json["validation_result"]["invalid_entries"]
            .as_u64()
            .unwrap();

        assert_eq!(status, "failed");
        assert_eq!(total, 2);
        assert_eq!(valid, 1);
        assert_eq!(invalid, 1);

        // Check that errors array contains the invalid entry
        let errors = result_json["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["column"].as_str().unwrap(), "Delta");
    }

    #[test]
    fn test_validate_database_passes_with_all_valid() {
        let (_temp_dir, db) = create_test_db();

        // Add only valid entries to State column (accepts any value)
        let cf = db.cf_handle(Column::State.as_str()).unwrap();
        let mut batch = WriteBatch::default();

        let mut key1 = vec![1u8; 32];
        key1.extend_from_slice(&[2u8; 32]);
        batch.put_cf(&cf, &key1, b"valid1");

        let mut key2 = vec![2u8; 32];
        key2.extend_from_slice(&[3u8; 32]);
        batch.put_cf(&cf, &key2, b"valid2");

        db.write(batch).unwrap();

        // Validate entire database
        let result_json = validate_database(&db).unwrap();

        let status = result_json["validation_result"]["status"].as_str().unwrap();
        let invalid = result_json["validation_result"]["invalid_entries"]
            .as_u64()
            .unwrap();

        assert_eq!(status, "passed");
        assert_eq!(invalid, 0);
    }
}
