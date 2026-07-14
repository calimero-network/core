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

/// Maximum valid_for_seconds value (roughly 1 year)
pub const MAX_VALID_FOR_SECONDS: u64 = 31_536_000;

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

    #[error("Field '{field}' value {actual} is below minimum of {min}")]
    ValueTooSmall {
        field: &'static str,
        min: u64,
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

    /// Validate a URL the node will fetch from (e.g. application install).
    ///
    /// Beyond the length cap this is the first line of SSRF defense: it enforces
    /// an `http`/`https` scheme and rejects URLs whose host is a literal
    /// loopback / private / link-local / unspecified IP, or `localhost`. This
    /// blocks the obvious metadata-service and internal-service targets
    /// (`http://169.254.169.254/...`, `http://127.0.0.1`, `http://10.0.0.1`,
    /// `http://[::1]`, `http://localhost`).
    ///
    /// It does NOT catch a public hostname that *resolves* to a private address
    /// (DNS rebinding) or a redirect to a private address — those require
    /// resolution at fetch time and are enforced separately by the download
    /// path. Keep both layers.
    pub fn validate_url(value: &url::Url, field: &'static str) -> Option<ValidationError> {
        let url_str = value.as_str();
        if url_str.len() > MAX_URL_LENGTH {
            return Some(ValidationError::StringTooLong {
                field,
                max: MAX_URL_LENGTH,
                actual: url_str.len(),
            });
        }

        match value.scheme() {
            "http" | "https" => {}
            other => {
                return Some(ValidationError::InvalidFormat {
                    field,
                    reason: format!(
                        "unsupported URL scheme '{other}'; only http and https are allowed"
                    ),
                });
            }
        }

        match value.host() {
            Some(host) if url_host_is_blocked(&host) => Some(ValidationError::InvalidFormat {
                field,
                reason:
                    "URL host is a loopback, private, link-local, or otherwise non-public address"
                        .to_owned(),
            }),
            Some(_) => None,
            None => Some(ValidationError::InvalidFormat {
                field,
                reason: "URL has no host".to_owned(),
            }),
        }
    }

    /// Whether a URL host must be refused as an SSRF target (literal private/
    /// loopback/link-local/unspecified IP, or a `localhost` domain).
    ///
    /// SYNC(#3053): the same blocked-range logic is duplicated in
    /// `calimero-node-primitives` (`client/application/install.rs::host_is_blocked`),
    /// which applies it at fetch time to also cover redirect hops. The two
    /// crates do not share a dependency; if you change the blocked ranges here
    /// (e.g. add CGNAT 100.64.0.0/10 or a new IPv6 special prefix), update that
    /// copy too. Tracked for deduplication into a shared crate in #3053.
    pub fn url_host_is_blocked(host: &url::Host<&str>) -> bool {
        match host {
            url::Host::Ipv4(ip) => ipv4_is_blocked(*ip),
            url::Host::Ipv6(ip) => ipv6_is_blocked(*ip),
            url::Host::Domain(domain) => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                domain == "localhost" || domain.ends_with(".localhost")
            }
        }
    }

    /// Blocked IPv4 ranges: loopback (127/8), private (10/8, 172.16/12,
    /// 192.168/16), link-local (169.254/16 — incl. the cloud metadata IP),
    /// unspecified (0.0.0.0), broadcast, and CGNAT (100.64.0.0/10, RFC 6598 —
    /// used by some cloud providers for internal/metadata reachability).
    fn ipv4_is_blocked(ip: std::net::Ipv4Addr) -> bool {
        let o = ip.octets();
        // 100.64.0.0/10: first octet 100, second octet 64..=127. `is_shared()`
        // would cover this but is unstable, so check the prefix directly.
        let is_cgnat = o[0] == 100 && (o[1] & 0xc0) == 0x40;
        ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || is_cgnat
    }

    /// Blocked IPv6: loopback (::1), unspecified (::), unique-local (fc00::/7),
    /// link-local (fe80::/10), and IPv4-mapped addresses whose embedded v4 is
    /// blocked. (Avoids unstable `Ipv6Addr` helpers by checking segments.)
    fn ipv6_is_blocked(ip: std::net::Ipv6Addr) -> bool {
        if ip.is_loopback() || ip.is_unspecified() {
            return true;
        }
        if let Some(v4) = ip.to_ipv4_mapped() {
            return ipv4_is_blocked(v4);
        }
        let first = ip.segments()[0];
        // fc00::/7 (unique-local) and fe80::/10 (link-local).
        (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
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
mod ssrf_and_path_tests {
    use url::Url;

    use super::helpers::{validate_safe_path, validate_url};
    use super::ValidationError;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn ssrf_targets_are_rejected() {
        for bad in [
            "http://169.254.169.254/latest/meta-data/", // cloud metadata
            "http://127.0.0.1:6379",
            "http://localhost:8080/admin",
            "https://LOCALHOST/x",
            "http://10.0.0.5/internal",
            "http://172.16.3.4/",
            "http://192.168.1.1/",
            "http://100.64.0.1/", // CGNAT (RFC 6598)
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fc00::1]/",
            "http://[::ffff:127.0.0.1]/", // IPv4-mapped loopback
        ] {
            assert!(
                matches!(
                    validate_url(&url(bad), "url"),
                    Some(ValidationError::InvalidFormat { .. })
                ),
                "{bad} must be rejected as an SSRF target",
            );
        }
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        for bad in ["file:///etc/passwd", "ftp://example.com/x", "gopher://x/"] {
            assert!(matches!(
                validate_url(&url(bad), "url"),
                Some(ValidationError::InvalidFormat { .. })
            ));
        }
    }

    #[test]
    fn public_urls_are_allowed() {
        for ok in [
            "https://registry.example.com/app.wasm",
            "http://93.184.216.34/app.mpk", // public literal IP
            "https://calimero.network/pkg/v1.mpk",
        ] {
            assert!(
                validate_url(&url(ok), "url").is_none(),
                "{ok} must be allowed",
            );
        }
    }

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
