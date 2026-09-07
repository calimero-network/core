//! Input validation for server request types.
//!
//! This module provides comprehensive validation for all request types,
//! checking payload sizes, string lengths, and format constraints.

use thiserror::Error as ThisError;

/// Maximum size for initialization parameters
pub const MAX_INIT_PARAMS_SIZE: usize = 1024 * 1024; // 1 MB

/// Maximum length for protocol strings
pub const MAX_PROTOCOL_LENGTH: usize = 64;

/// Maximum length for package names
pub const MAX_PACKAGE_NAME_LENGTH: usize = 128;

/// Maximum length for version strings
pub const MAX_VERSION_LENGTH: usize = 64;

/// Maximum length for hash strings (hex-encoded, 32 bytes = 64 chars)
pub const MAX_HASH_LENGTH: usize = 64;

/// Maximum length for base64-encoded quote
pub const MAX_QUOTE_B64_LENGTH: usize = 64 * 1024; // 64 KB

/// Maximum length for file paths
pub const MAX_PATH_LENGTH: usize = 4096;

/// Maximum number of capabilities in a single request
pub const MAX_CAPABILITIES_COUNT: usize = 100;

/// Maximum offset for pagination
pub const MAX_PAGINATION_OFFSET: usize = 1_000_000;

/// Maximum limit for pagination
pub const MAX_PAGINATION_LIMIT: usize = 1000;

/// Maximum length for context value keys
pub const MAX_CONTEXT_KEY_LENGTH: usize = 1024;

/// Maximum valid_for_seconds value (roughly 1 year)
pub const MAX_VALID_FOR_SECONDS: u64 = 31_536_000;

/// Maximum length for method names in execution requests
pub const MAX_METHOD_NAME_LENGTH: usize = 256;

/// Maximum size for JSON arguments in execution requests (10 MB)
pub const MAX_ARGS_JSON_SIZE: usize = 10 * 1024 * 1024;

/// Validation error types
#[derive(Clone, Debug, ThisError)]
pub enum ValidationError {
    #[error("Field '{field}' exceeds maximum length of {max} characters (got {actual})")]
    StringTooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },

    #[error("Field '{field}' exceeds maximum size of {max} bytes (got {actual})")]
    PayloadTooLarge {
        field: &'static str,
        max: usize,
        actual: usize,
    },

    #[error("Field '{field}' must be exactly {expected} characters (got {actual})")]
    InvalidLength {
        field: &'static str,
        expected: usize,
        actual: usize,
    },

    #[error("Field '{field}' contains invalid hex encoding: {reason}")]
    InvalidHexEncoding { field: &'static str, reason: String },

    #[error("Field '{field}' value {actual} exceeds maximum of {max}")]
    ValueTooLarge {
        field: &'static str,
        max: u64,
        actual: u64,
    },

    #[error("Field '{field}' value {actual} is below minimum of {min}")]
    ValueTooSmall {
        field: &'static str,
        min: u64,
        actual: u64,
    },

    #[error("Field '{field}' is required but was empty")]
    EmptyField { field: &'static str },

    #[error("Field '{field}' has invalid format: {reason}")]
    InvalidFormat { field: &'static str, reason: String },
}

/// Trait for validating request types
pub trait Validate {
    /// Validate the request and return a list of validation errors.
    /// Returns an empty Vec if validation passes.
    fn validate(&self) -> Vec<ValidationError>;

    /// Validate and return the first error if any.
    fn validate_first(&self) -> Result<(), ValidationError> {
        self.validate().into_iter().next().map_or(Ok(()), Err)
    }
}

/// Helper functions for common validations
pub mod helpers {
    use super::*;

    /// Validate string length
    pub fn validate_string_length(
        value: &str,
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        if value.len() > max {
            Some(ValidationError::StringTooLong {
                field,
                max,
                actual: value.len(),
            })
        } else {
            None
        }
    }

    /// Validate that a string carries a value at all.
    pub fn validate_non_empty(value: &str, field: &'static str) -> Option<ValidationError> {
        value.is_empty().then(|| ValidationError::InvalidFormat {
            field,
            reason: "must not be empty".to_owned(),
        })
    }

    /// Validate byte slice size
    pub fn validate_bytes_size(
        value: &[u8],
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        if value.len() > max {
            Some(ValidationError::PayloadTooLarge {
                field,
                max,
                actual: value.len(),
            })
        } else {
            None
        }
    }

    /// Validate hex string (must be valid hex and specific length)
    ///
    /// Uses character-based validation to avoid allocating a Vec for decoding.
    pub fn validate_hex_string(
        value: &str,
        field: &'static str,
        expected_bytes: usize,
    ) -> Option<ValidationError> {
        let expected_chars = expected_bytes * 2;

        if value.len() != expected_chars {
            return Some(ValidationError::InvalidLength {
                field,
                expected: expected_chars,
                actual: value.len(),
            });
        }

        // Validate hex characters without allocating
        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(ValidationError::InvalidHexEncoding {
                field,
                reason: "contains non-hexadecimal characters".to_owned(),
            });
        }

        None
    }

    /// Validate optional hex string
    pub fn validate_optional_hex_string(
        value: &Option<String>,
        field: &'static str,
        expected_bytes: usize,
    ) -> Option<ValidationError> {
        value
            .as_ref()
            .and_then(|s| validate_hex_string(s, field, expected_bytes))
    }

    /// Validate pagination offset
    pub fn validate_offset(value: usize, field: &'static str) -> Option<ValidationError> {
        if value > MAX_PAGINATION_OFFSET {
            Some(ValidationError::ValueTooLarge {
                field,
                max: MAX_PAGINATION_OFFSET as u64,
                actual: value as u64,
            })
        } else {
            None
        }
    }

    /// Validate pagination limit (must be > 0 and <= MAX_PAGINATION_LIMIT)
    pub fn validate_limit(value: usize, field: &'static str) -> Option<ValidationError> {
        if value == 0 {
            return Some(ValidationError::ValueTooSmall {
                field,
                min: 1,
                actual: 0,
            });
        }
        if value > MAX_PAGINATION_LIMIT {
            Some(ValidationError::ValueTooLarge {
                field,
                max: MAX_PAGINATION_LIMIT as u64,
                actual: value as u64,
            })
        } else {
            None
        }
    }

    /// Validate a local filesystem path supplied by a client (e.g. dev install).
    ///
    /// Rejects `..` traversal components, which are never legitimate and are the
    /// classic way to escape an intended directory (e.g. `foo/../../etc/passwd`).
    ///
    /// Absolute paths are intentionally allowed: dev installs commonly point at
    /// an absolute build-output path (`meroctl app install --path /abs/app.wasm`),
    /// and the `install-dev-application` endpoint is node-owner/admin-only (the
    /// permission gate denies non-admin tokens), so reading the owner's own
    /// filesystem is not a privilege boundary. This check is defense-in-depth
    /// against traversal tricks layered on top of that gate.
    pub fn validate_safe_path(path: &str, field: &'static str) -> Option<ValidationError> {
        use std::path::{Component, Path};

        if path.len() > MAX_PATH_LENGTH {
            return Some(ValidationError::StringTooLong {
                field,
                max: MAX_PATH_LENGTH,
                actual: path.len(),
            });
        }

        if Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Some(ValidationError::InvalidFormat {
                field,
                reason: "path must not contain '..' traversal components".to_owned(),
            });
        }

        None
    }

    /// Validate method name (checks for empty, length, and control characters)
    ///
    /// Only minimal character restrictions are enforced here (control characters are rejected).
    /// The OpenAPI spec does not define specific character constraints for method names, so
    /// more specific validation is handled by the WASM execution layer at runtime.
    pub fn validate_method_name(value: &str, field: &'static str) -> Option<ValidationError> {
        if value.is_empty() {
            return Some(ValidationError::EmptyField { field });
        }

        if value.len() > MAX_METHOD_NAME_LENGTH {
            return Some(ValidationError::StringTooLong {
                field,
                max: MAX_METHOD_NAME_LENGTH,
                actual: value.len(),
            });
        }

        // Check for control characters which are never valid in method names
        for c in value.chars() {
            if c.is_ascii_control() {
                return Some(ValidationError::InvalidFormat {
                    field,
                    reason: format!(
                        "contains control character '{}' which is not allowed",
                        c.escape_default()
                    ),
                });
            }
        }

        None
    }

    /// Validate JSON value size using a recursive size estimator.
    ///
    /// This estimates the serialized size without allocating by walking the JSON tree.
    /// The estimate uses a conservative 2x multiplier for strings to account for
    /// JSON escape sequences (e.g., `"` becomes `\"`). This may overestimate but
    /// ensures security against strings crafted to expand during serialization.
    pub fn validate_json_size(
        value: &serde_json::Value,
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        let size = estimate_json_size(value);
        if size > max {
            Some(ValidationError::PayloadTooLarge {
                field,
                max,
                actual: size,
            })
        } else {
            None
        }
    }

    /// Recursively estimate the serialized size of a JSON value without allocating.
    ///
    /// Uses conservative estimates for strings (2x multiplier) to account for escape sequences.
    /// This may overestimate the actual serialized size but prevents underestimation attacks
    /// where strings with many escapable characters expand significantly during serialization.
    fn estimate_json_size(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Null => 4, // "null"
            serde_json::Value::Bool(b) => {
                if *b {
                    4
                } else {
                    5
                }
            } // "true" or "false"
            serde_json::Value::Number(n) => n.to_string().len(), // Numbers vary in length
            // Conservative: assume worst case where chars may need escaping (2x) + quotes
            serde_json::Value::String(s) => s.len() * 2 + 2,
            serde_json::Value::Array(arr) => {
                // 2 for brackets, commas between elements
                let content_size: usize = arr.iter().map(estimate_json_size).sum();
                let comma_size = if arr.is_empty() { 0 } else { arr.len() - 1 };
                2 + content_size + comma_size
            }
            serde_json::Value::Object(obj) => {
                // 2 for braces, commas between entries, colons after keys
                // Keys also use conservative 2x multiplier for escaping
                let content_size: usize = obj
                    .iter()
                    .map(|(k, v)| k.len() * 2 + 2 + 1 + estimate_json_size(v)) // key*2 + quotes + colon + value
                    .sum();
                let comma_size = if obj.is_empty() { 0 } else { obj.len() - 1 };
                2 + content_size + comma_size
            }
        }
    }
}

#[cfg(test)]
mod path_tests {
    use super::helpers::validate_safe_path;
    use super::ValidationError;

    #[test]
    fn path_traversal_is_rejected_absolute_allowed() {
        for bad in ["../../etc/passwd", "foo/../../bar", "a/../b/../../c"] {
            assert!(
                matches!(
                    validate_safe_path(bad, "path"),
                    Some(ValidationError::InvalidFormat { .. })
                ),
                "{bad} must be rejected for traversal",
            );
        }
        // Absolute and plain relative paths are allowed (dev-install ergonomics;
        // the endpoint is admin-only).
        assert!(validate_safe_path("/abs/build/app.wasm", "path").is_none());
        assert!(validate_safe_path("res/app.wasm", "path").is_none());
        assert!(validate_safe_path("./res/app.wasm", "path").is_none());
    }
}
