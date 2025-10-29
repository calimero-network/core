//! Composite keys for nested CRDT storage
//!
//! Enables flattened storage of nested structures like `Map<K, Map<K2, V>>` by
//! combining outer and inner keys into a single storage key: "outer::inner"
//!
//! # Example
//!
//! ```ignore
//! let doc_id = b"doc-1";
//! let field = b"title";
//!
//! let composite = CompositeKey::new(doc_id, field);
//! // Produces: b"doc-1::title"
//!
//! let (outer, inner) = CompositeKey::parse(&composite.as_bytes())?;
//! assert_eq!(outer, b"doc-1");
//! assert_eq!(inner, b"title");
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

/// Separator between key components (:: chosen for readability)
const SEPARATOR: &[u8] = b"::";

/// A composite key combining multiple key parts for hierarchical storage
///
/// Used to flatten nested structures into single-level storage while preserving
/// the ability to reconstruct the hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct CompositeKey {
    /// The serialized bytes of the composite key
    bytes: Vec<u8>,
}

impl CompositeKey {
    /// Create a new composite key from two parts
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = CompositeKey::new(b"doc-1", b"title");
    /// assert_eq!(key.as_bytes(), b"doc-1::title");
    /// ```
    pub fn new(outer: &[u8], inner: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(outer.len() + SEPARATOR.len() + inner.len());
        bytes.extend_from_slice(outer);
        bytes.extend_from_slice(SEPARATOR);
        bytes.extend_from_slice(inner);

        Self { bytes }
    }

    /// Create a composite key from three parts (for deeper nesting)
    ///
    /// Produces: "part1::part2::part3"
    pub fn new_3(part1: &[u8], part2: &[u8], part3: &[u8]) -> Self {
        let total_len = part1.len() + SEPARATOR.len() + part2.len() + SEPARATOR.len() + part3.len();
        let mut bytes = Vec::with_capacity(total_len);

        bytes.extend_from_slice(part1);
        bytes.extend_from_slice(SEPARATOR);
        bytes.extend_from_slice(part2);
        bytes.extend_from_slice(SEPARATOR);
        bytes.extend_from_slice(part3);

        Self { bytes }
    }

    /// Create a composite key from multiple parts
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = CompositeKey::new_multi(&[b"a", b"b", b"c"]);
    /// assert_eq!(key.as_bytes(), b"a::b::c");
    /// ```
    pub fn new_multi(parts: &[&[u8]]) -> Self {
        if parts.is_empty() {
            return Self { bytes: Vec::new() };
        }

        let total_len: usize =
            parts.iter().map(|p| p.len()).sum::<usize>() + (SEPARATOR.len() * (parts.len() - 1));

        let mut bytes = Vec::with_capacity(total_len);

        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                bytes.extend_from_slice(SEPARATOR);
            }
            bytes.extend_from_slice(part);
        }

        Self { bytes }
    }

    /// Get the composite key as bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume and return the inner bytes
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Parse a composite key back into its two parts
    ///
    /// # Errors
    ///
    /// Returns error if the key doesn't contain the separator
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (outer, inner) = CompositeKey::parse(b"doc-1::title")?;
    /// assert_eq!(outer, b"doc-1");
    /// assert_eq!(inner, b"title");
    /// ```
    pub fn parse(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ParseError> {
        let pos = bytes
            .windows(SEPARATOR.len())
            .position(|window| window == SEPARATOR)
            .ok_or(ParseError::MissingSeparator)?;

        let outer = bytes[..pos].to_vec();
        let inner = bytes[pos + SEPARATOR.len()..].to_vec();

        Ok((outer, inner))
    }

    /// Parse a composite key into all its parts
    ///
    /// # Example
    ///
    /// ```ignore
    /// let parts = CompositeKey::parse_multi(b"a::b::c")?;
    /// assert_eq!(parts, vec![b"a", b"b", b"c"]);
    /// ```
    pub fn parse_multi(bytes: &[u8]) -> Result<Vec<Vec<u8>>, ParseError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }

        let mut parts = Vec::new();
        let mut start = 0;

        while start < bytes.len() {
            if let Some(pos) = bytes[start..]
                .windows(SEPARATOR.len())
                .position(|window| window == SEPARATOR)
            {
                parts.push(bytes[start..start + pos].to_vec());
                start += pos + SEPARATOR.len();
            } else {
                // Last part
                parts.push(bytes[start..].to_vec());
                break;
            }
        }

        Ok(parts)
    }

    /// Check if a key starts with the given prefix
    ///
    /// Useful for prefix scanning to find all sub-keys
    pub fn has_prefix(&self, prefix: &[u8]) -> bool {
        self.bytes.starts_with(prefix)
    }

    /// Extract the prefix for scanning all keys with a given outer key
    ///
    /// # Example
    ///
    /// ```ignore
    /// let prefix = CompositeKey::prefix_for(b"doc-1");
    /// // Returns: b"doc-1::" for scanning all fields of doc-1
    /// ```
    pub fn prefix_for(outer_key: &[u8]) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(outer_key.len() + SEPARATOR.len());
        prefix.extend_from_slice(outer_key);
        prefix.extend_from_slice(SEPARATOR);
        prefix
    }
}

impl AsRef<[u8]> for CompositeKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl From<Vec<u8>> for CompositeKey {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// Errors that can occur when parsing composite keys
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The key doesn't contain a separator
    MissingSeparator,
    /// Invalid format
    InvalidFormat(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MissingSeparator => write!(f, "Composite key missing separator '::'"),
            ParseError::InvalidFormat(msg) => write!(f, "Invalid composite key format: {}", msg),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composite_key_creation() {
        let key = CompositeKey::new(b"doc-1", b"title");
        assert_eq!(key.as_bytes(), b"doc-1::title");
    }

    #[test]
    fn test_composite_key_parse() {
        let (outer, inner) = CompositeKey::parse(b"doc-1::title").unwrap();
        assert_eq!(outer, b"doc-1");
        assert_eq!(inner, b"title");
    }

    #[test]
    fn test_composite_key_roundtrip() {
        let key = CompositeKey::new(b"user-123", b"email");
        let (outer, inner) = CompositeKey::parse(key.as_bytes()).unwrap();

        assert_eq!(outer, b"user-123");
        assert_eq!(inner, b"email");
    }

    #[test]
    fn test_composite_key_three_parts() {
        let key = CompositeKey::new_3(b"a", b"b", b"c");
        assert_eq!(key.as_bytes(), b"a::b::c");
    }

    #[test]
    fn test_composite_key_multi() {
        let key = CompositeKey::new_multi(&[b"x", b"y", b"z"]);
        assert_eq!(key.as_bytes(), b"x::y::z");
    }

    #[test]
    fn test_composite_key_parse_multi() {
        let parts = CompositeKey::parse_multi(b"a::b::c::d").unwrap();
        assert_eq!(parts, vec![b"a", b"b", b"c", b"d"]);
    }

    #[test]
    fn test_composite_key_prefix() {
        let prefix = CompositeKey::prefix_for(b"doc-1");
        assert_eq!(prefix, b"doc-1::");
    }

    #[test]
    fn test_composite_key_has_prefix() {
        let key = CompositeKey::new(b"doc-1", b"title");
        assert!(key.has_prefix(b"doc-1"));
        assert!(key.has_prefix(b"doc-1::"));
        assert!(!key.has_prefix(b"doc-2"));
    }

    #[test]
    fn test_composite_key_empty() {
        let key = CompositeKey::new_multi(&[]);
        assert!(key.as_bytes().is_empty());
    }

    #[test]
    fn test_composite_key_single_part() {
        let key = CompositeKey::new_multi(&[b"only"]);
        assert_eq!(key.as_bytes(), b"only");
    }

    #[test]
    fn test_parse_missing_separator() {
        let result = CompositeKey::parse(b"no-separator");
        assert!(matches!(result, Err(ParseError::MissingSeparator)));
    }

    #[test]
    fn test_composite_key_with_special_chars() {
        let key = CompositeKey::new(b"doc-#1@!", b"field$%");
        assert_eq!(key.as_bytes(), b"doc-#1@!::field$%");

        let (outer, inner) = CompositeKey::parse(key.as_bytes()).unwrap();
        assert_eq!(outer, b"doc-#1@!");
        assert_eq!(inner, b"field$%");
    }

    #[test]
    fn test_composite_key_borsh_serialization() {
        let key = CompositeKey::new(b"test", b"value");

        // Serialize
        let bytes = borsh::to_vec(&key).unwrap();

        // Deserialize
        let deserialized: CompositeKey = borsh::from_slice(&bytes).unwrap();

        assert_eq!(deserialized, key);
    }
}
