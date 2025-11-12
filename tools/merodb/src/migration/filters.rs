//!
//! Shared filter resolution and matching logic for migration operations.
//!
//! This module provides the common filtering infrastructure used by both dry-run preview
//! and actual migration execution. It handles:
//!
//! - **Filter Resolution**: Decoding hex strings, parsing context IDs, handling prefixes
//! - **Key Matching**: Applying resolved filters to database keys with AND logic
//! - **Validation**: Accumulating warnings for unsupported or malformed filters
//!
//! ## Filter Types
//!
//! - `context_ids`: Match keys where the first 32 bytes equal one of the specified context IDs
//! - `context_aliases`: Context name resolution (not yet implemented, generates warning)
//! - `state_key_prefix`: Match State column keys where bytes [32..] start with prefix
//! - `raw_key_prefix`: Match keys starting with the specified byte sequence
//! - `key_range`: Lexicographic range [start, end) for keys
//! - `alias_name`: Match Alias column keys by alias name (83-byte canonical format)
//!
//! ## Usage
//!
//! ```ignore
//! let filters = defaults.merge_filters(&step.filters);
//! let resolved = ResolvedFilters::resolve(column, &filters);
//!
//! for (key, value) in db_iterator {
//!     if resolved.matches(column, &key) {
//!         // Process matching key
//!     }
//! }
//!
//! // Check for any warnings during resolution
//! for warning in &resolved.warnings {
//!     eprintln!("Warning: {}", warning);
//! }
//! ```

use std::collections::HashSet;

use eyre::{bail, Result};

use crate::types::Column;

use super::plan::PlanFilters;

/// Concrete filter values (decoded/parsed) used during column scans.
///
/// This structure contains the resolved, byte-oriented representation of filters
/// from the migration plan. All filters are applied with AND logic - a key must
/// satisfy every active filter to match.
pub struct ResolvedFilters {
    /// Set of context IDs (32 bytes each) to match against key prefix
    pub context_ids: Option<HashSet<Vec<u8>>>,
    /// State key prefix (matches bytes [32..] in State column)
    pub state_key_prefix: Option<Vec<u8>>,
    /// Raw key prefix (matches from byte 0)
    pub raw_key_prefix: Option<Vec<u8>>,
    /// Key range start bound (inclusive)
    pub key_range_start: Option<Vec<u8>>,
    /// Key range end bound (exclusive)
    pub key_range_end: Option<Vec<u8>>,
    /// Alias name filter (only for Alias column)
    pub alias_name: Option<String>,
    /// Warnings accumulated during filter resolution
    pub warnings: Vec<String>,
}

impl ResolvedFilters {
    /// Decode plan filters into byte-oriented structures, accumulating warnings as needed.
    ///
    /// This method performs filter resolution by:
    /// 1. Decoding hex-encoded values (context IDs, prefixes, ranges)
    /// 2. Validating filter applicability to the target column
    /// 3. Collecting warnings for unsupported or malformed filters
    ///
    /// # Arguments
    ///
    /// * `column` - The column these filters will be applied to
    /// * `filters` - The plan-level filter configuration
    ///
    /// # Returns
    ///
    /// A `ResolvedFilters` instance with decoded byte values and any accumulated warnings.
    pub fn resolve(column: Column, filters: &PlanFilters) -> Self {
        let mut warnings = Vec::new();

        let context_ids = if filters.context_ids.is_empty() {
            None
        } else {
            let mut set = HashSet::new();
            for id in &filters.context_ids {
                match decode_hex_string(id) {
                    Ok(bytes) => {
                        let _ = set.insert(bytes);
                    }
                    Err(err) => warnings.push(format!("unable to parse context_id '{id}': {err}")),
                }
            }
            Some(set)
        };

        // Context aliases are not yet supported
        if !filters.context_aliases.is_empty() {
            warnings.push(
                "context_aliases filter is not yet supported; step may process more keys than expected"
                    .into(),
            );
        }

        let state_key_prefix =
            filters
                .state_key_prefix
                .as_ref()
                .map(|prefix| match decode_hex_string(prefix) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret state_key_prefix '{prefix}': {err}"
                        ));
                        prefix.as_bytes().to_vec()
                    }
                });

        let raw_key_prefix =
            filters
                .raw_key_prefix
                .as_ref()
                .and_then(|prefix| match decode_hex_string(prefix) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret raw_key_prefix '{prefix}': {err}"
                        ));
                        None
                    }
                });

        let key_range_start = filters.key_range.as_ref().and_then(|range| {
            range
                .start
                .as_ref()
                .and_then(|start| match decode_hex_string(start) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!(
                            "unable to interpret key_range start '{start}': {err}"
                        ));
                        None
                    }
                })
        });

        let key_range_end = filters.key_range.as_ref().and_then(|range| {
            range
                .end
                .as_ref()
                .and_then(|end| match decode_hex_string(end) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        warnings.push(format!("unable to interpret key_range end '{end}': {err}"));
                        None
                    }
                })
        });

        let alias_name = filters.alias_name.clone();
        if alias_name.is_some() && column != Column::Alias {
            warnings.push(
                "alias_name filter only applies to the Alias column; no rows will match in other columns"
                    .into(),
            );
        }

        Self {
            context_ids,
            state_key_prefix,
            raw_key_prefix,
            key_range_start,
            key_range_end,
            alias_name,
            warnings,
        }
    }

    /// Check if a raw key satisfies every resolved predicate.
    ///
    /// This method applies all active filters to determine whether a key should be
    /// included in the current operation. It returns true only if the key passes
    /// all filter checks (AND logic).
    ///
    /// # Arguments
    ///
    /// * `column` - The column this key belongs to (affects context ID extraction)
    /// * `key` - The raw key bytes to test
    ///
    /// # Returns
    ///
    /// `true` if the key matches all active filters, `false` otherwise.
    pub fn matches(&self, column: Column, key: &[u8]) -> bool {
        // Context ID filter: extract and check the first 32 bytes
        if let Some(set) = &self.context_ids {
            let Some(context_slice) = extract_context_id(column, key) else {
                return false;
            };

            if !set.contains(&context_slice.to_vec()) {
                return false;
            }
        }

        // State key prefix filter: check bytes [32..] in State column
        if let Some(prefix) = &self.state_key_prefix {
            if column != Column::State {
                return false;
            }
            let Some(end) = 32_usize.checked_add(prefix.len()) else {
                return false;
            };
            if key.len() < end || !key[32..end].starts_with(prefix) {
                return false;
            }
        }

        // Raw key prefix filter: check the entire key
        if let Some(prefix) = &self.raw_key_prefix {
            if !key.starts_with(prefix) {
                return false;
            }
        }

        // Key range start filter: lexicographic comparison
        if let Some(start) = &self.key_range_start {
            if key < start.as_slice() {
                return false;
            }
        }

        // Key range end filter: lexicographic comparison (exclusive)
        if let Some(end) = &self.key_range_end {
            if key >= end.as_slice() {
                return false;
            }
        }

        // Alias name filter: only applies to Alias column
        if let Some(alias) = &self.alias_name {
            if column != Column::Alias {
                return false;
            }

            match extract_alias_name(key) {
                Some(name) => {
                    if &name != alias {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }
}

/// Best-effort hex decoder used by filter resolution.
///
/// Accepts hex strings with or without "0x" prefix and ensures even length.
///
/// # Errors
///
/// Returns an error if the hex string has odd length or contains invalid hex characters.
fn decode_hex_string(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim().trim_start_matches("0x");
    if trimmed.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    Ok(hex::decode(trimmed)?)
}

/// Extract the context ID portion from a key when the column layout supports it.
///
/// For columns with context ID as the first 32 bytes (Meta, Config, Identity, State, Delta),
/// this function returns a slice of those bytes. For other columns, returns None.
///
/// # Arguments
///
/// * `column` - The column family this key belongs to
/// * `key` - The raw key bytes
///
/// # Returns
///
/// A slice of the first 32 bytes if the column supports context IDs and the key is long enough,
/// otherwise None.
fn extract_context_id(column: Column, key: &[u8]) -> Option<&[u8]> {
    if key.len() < 32 {
        return None;
    }

    match column {
        Column::Meta | Column::Config | Column::Identity | Column::State | Column::Delta => {
            Some(&key[0..32])
        }
        _ => None,
    }
}

/// Pull the alias name out of the canonical 83-byte alias key.
///
/// Alias keys have the structure: [kind: 1 byte][scope: 32 bytes][name: 50 bytes].
/// This function extracts the name portion and strips trailing null bytes.
///
/// # Arguments
///
/// * `key` - The raw alias key bytes
///
/// # Returns
///
/// The extracted alias name with trailing nulls removed, or None if the key is not 83 bytes.
fn extract_alias_name(key: &[u8]) -> Option<String> {
    if key.len() != 83 {
        return None;
    }
    let name_bytes = &key[33..83];
    Some(
        String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_hex_string_with_0x_prefix() {
        let result = decode_hex_string("0xaabbcc").unwrap();
        assert_eq!(result, vec![0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn test_decode_hex_string_without_prefix() {
        let result = decode_hex_string("aabbcc").unwrap();
        assert_eq!(result, vec![0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn test_decode_hex_string_odd_length_fails() {
        let result = decode_hex_string("aabbc");
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_context_id_from_state_column() {
        let mut key = [0_u8; 64];
        key[..32].copy_from_slice(&[0x11; 32]);
        key[32..].copy_from_slice(&[0x22; 32]);

        let context = extract_context_id(Column::State, &key);
        assert_eq!(context, Some(&[0x11; 32][..]));
    }

    #[test]
    fn test_extract_context_id_from_generic_returns_none() {
        let key = [0x11; 64];
        let context = extract_context_id(Column::Generic, &key);
        assert_eq!(context, None);
    }

    #[test]
    fn test_extract_context_id_short_key_returns_none() {
        let key = [0x11; 16]; // Only 16 bytes
        let context = extract_context_id(Column::State, &key);
        assert_eq!(context, None);
    }

    #[test]
    fn test_extract_alias_name() {
        let mut key = [0_u8; 83];
        key[0] = 0x01; // kind
        key[1..33].copy_from_slice(&[0x22; 32]); // scope
        key[33..43].copy_from_slice(b"test_alias"); // name (padded with nulls)

        let name = extract_alias_name(&key);
        assert_eq!(name, Some(String::from("test_alias")));
    }

    #[test]
    fn test_extract_alias_name_wrong_length_returns_none() {
        let key = [0_u8; 50]; // Wrong length
        let name = extract_alias_name(&key);
        assert_eq!(name, None);
    }

    #[test]
    fn test_resolved_filters_matches_context_id() {
        let filters = ResolvedFilters {
            context_ids: Some({
                let mut set = HashSet::new();
                let _ = set.insert(vec![0x11; 32]);
                set
            }),
            state_key_prefix: None,
            raw_key_prefix: None,
            key_range_start: None,
            key_range_end: None,
            alias_name: None,
            warnings: Vec::new(),
        };

        let mut key = [0_u8; 64];
        key[..32].copy_from_slice(&[0x11; 32]);

        assert!(filters.matches(Column::State, &key));

        // Different context should not match
        key[..32].copy_from_slice(&[0x22; 32]);
        assert!(!filters.matches(Column::State, &key));
    }

    #[test]
    fn test_resolved_filters_matches_raw_key_prefix() {
        let filters = ResolvedFilters {
            context_ids: None,
            state_key_prefix: None,
            raw_key_prefix: Some(b"prefix_".to_vec()),
            key_range_start: None,
            key_range_end: None,
            alias_name: None,
            warnings: Vec::new(),
        };

        assert!(filters.matches(Column::Generic, b"prefix_test"));
        assert!(!filters.matches(Column::Generic, b"other_test"));
    }

    #[test]
    fn test_resolved_filters_matches_key_range() {
        let filters = ResolvedFilters {
            context_ids: None,
            state_key_prefix: None,
            raw_key_prefix: None,
            key_range_start: Some(b"bbb".to_vec()),
            key_range_end: Some(b"ddd".to_vec()),
            alias_name: None,
            warnings: Vec::new(),
        };

        assert!(!filters.matches(Column::Generic, b"aaa")); // Before start
        assert!(filters.matches(Column::Generic, b"bbb")); // At start (inclusive)
        assert!(filters.matches(Column::Generic, b"ccc")); // In range
        assert!(!filters.matches(Column::Generic, b"ddd")); // At end (exclusive)
        assert!(!filters.matches(Column::Generic, b"eee")); // After end
    }
}
