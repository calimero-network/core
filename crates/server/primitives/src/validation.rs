//! Input validation for server request types.
//!
//! This module provides comprehensive validation for all request types,
//! checking payload sizes, string lengths, and format constraints.

use thiserror::Error as ThisError;

/// Maximum size for metadata fields (e.g., application metadata)
pub const MAX_METADATA_SIZE: usize = 64 * 1024; // 64 KB

/// Maximum size for initialization parameters
pub const MAX_INIT_PARAMS_SIZE: usize = 1024 * 1024; // 1 MB

/// Maximum length for protocol strings
pub const MAX_PROTOCOL_LENGTH: usize = 64;

/// Maximum length for package names
pub const MAX_PACKAGE_NAME_LENGTH: usize = 128;

/// Maximum length for version strings
pub const MAX_VERSION_LENGTH: usize = 64;

/// Maximum length for nonce strings (hex-encoded, 32 bytes = 64 chars)
pub const MAX_NONCE_LENGTH: usize = 64;

/// Maximum length for hash strings (hex-encoded, 32 bytes = 64 chars)
pub const MAX_HASH_LENGTH: usize = 64;

/// Maximum length for base64-encoded quote
pub const MAX_QUOTE_B64_LENGTH: usize = 64 * 1024; // 64 KB

/// Maximum length for URL strings
pub const MAX_URL_LENGTH: usize = 2048;

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

/// Maximum valid_for_blocks value (roughly 1 year at 1 block/second)
pub const MAX_VALID_FOR_BLOCKS: u64 = 31_536_000;

/// Maximum length for method names in execution requests
pub const MAX_METHOD_NAME_LENGTH: usize = 256;

/// Maximum size for JSON arguments in execution requests (10 MB)
pub const MAX_ARGS_JSON_SIZE: usize = 10 * 1024 * 1024;

/// Maximum number of substitute aliases in execution requests
pub const MAX_SUBSTITUTE_ALIASES: usize = 100;

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

    #[error("Field '{field}' contains too many items: {actual} (max {max})")]
    TooManyItems {
        field: &'static str,
        max: usize,
        actual: usize,
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

    /// Validate optional string length
    pub fn validate_optional_string_length(
        value: &Option<String>,
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        value
            .as_ref()
            .and_then(|s| validate_string_length(s, field, max))
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

        if hex::decode(value).is_err() {
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

    /// Validate pagination limit
    pub fn validate_limit(value: usize, field: &'static str) -> Option<ValidationError> {
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

    /// Validate collection size
    pub fn validate_collection_size<T>(
        value: &[T],
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        if value.len() > max {
            Some(ValidationError::TooManyItems {
                field,
                max,
                actual: value.len(),
            })
        } else {
            None
        }
    }

    /// Validate URL length
    pub fn validate_url(value: &url::Url, field: &'static str) -> Option<ValidationError> {
        let url_str = value.as_str();
        if url_str.len() > MAX_URL_LENGTH {
            Some(ValidationError::StringTooLong {
                field,
                max: MAX_URL_LENGTH,
                actual: url_str.len(),
            })
        } else {
            None
        }
    }

    /// Validate method name (checks for empty and length only)
    ///
    /// Note: Character restrictions are intentionally not enforced here as the OpenAPI spec
    /// does not define specific character constraints for method names. Runtime validation
    /// is handled separately in the WASM execution layer.
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

    /// Validate JSON value size (serialized)
    pub fn validate_json_size(
        value: &serde_json::Value,
        field: &'static str,
        max: usize,
    ) -> Option<ValidationError> {
        // Estimate size by serializing to string
        let size = value.to_string().len();
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
}
