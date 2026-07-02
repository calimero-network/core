//! Composite keys for nested CRDT storage
//!
//! Enables flattened storage of nested structures like `Map<K, Map<K2, V>>` by
//! combining outer and inner keys into a single storage key.
//!
//! # Encoding
//!
//! Parts are **length-prefixed**: each part is encoded as a 4-byte big-endian
//! length followed by the raw part bytes, and the parts are concatenated. A
//! naive delimiter scheme (joining with a `::` separator) is *not* injective —
//! `new(b"a::b", b"c")` and `new(b"a", b"b::c")` would both serialize to
//! `a::b::c`, so a part whose bytes contain the delimiter could forge a
//! different key structure (key injection). Length prefixing removes any
//! dependence on the part contents: the decoder is driven purely by the
//! declared lengths, so distinct part sequences always produce distinct bytes.
//!
//! Big-endian lengths keep keys that share an outer part contiguous under
//! lexicographic (byte) ordering, so prefix scans via [`CompositeKey::prefix_for`]
//! still work.
//!
//! # Example
//!
//! ```ignore
//! let composite = CompositeKey::new(b"doc-1", b"title");
//!
//! let (outer, inner) = CompositeKey::parse(composite.as_bytes())?;
//! assert_eq!(outer, b"doc-1");
//! assert_eq!(inner, b"title");
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

/// Width in bytes of the per-part length prefix (big-endian `u32`).
const LEN_PREFIX_SIZE: usize = 4;

/// A composite key combining multiple key parts for hierarchical storage
///
/// Used to flatten nested structures into single-level storage while preserving
/// the ability to reconstruct the hierarchy. See the module docs for the
/// length-prefixed encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct CompositeKey {
    /// The serialized bytes of the composite key
    bytes: Vec<u8>,
}

/// Append a single length-prefixed part to `bytes`.
fn encode_part(bytes: &mut Vec<u8>, part: &[u8]) {
    debug_assert!(
        u32::try_from(part.len()).is_ok(),
        "composite key part exceeds u32::MAX bytes"
    );
    // Truncation here is impossible for any realistic storage key (parts are a
    // handful of bytes); the debug_assert guards the theoretical case.
    let len = part.len() as u32;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(part);
}

/// Read one length-prefixed part starting at `offset`, returning the part and
/// the offset just past it.
fn decode_part(bytes: &[u8], offset: usize) -> Result<(Vec<u8>, usize), ParseError> {
    let len_end = offset
        .checked_add(LEN_PREFIX_SIZE)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| ParseError::Truncated("missing length prefix".to_owned()))?;

    let mut len_bytes = [0u8; LEN_PREFIX_SIZE];
    len_bytes.copy_from_slice(&bytes[offset..len_end]);
    let part_len = u32::from_be_bytes(len_bytes) as usize;

    let part_end = len_end
        .checked_add(part_len)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| ParseError::Truncated("part length exceeds remaining bytes".to_owned()))?;

    Ok((bytes[len_end..part_end].to_vec(), part_end))
}

impl CompositeKey {
    /// Create a new composite key from two parts
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = CompositeKey::new(b"doc-1", b"title");
    /// let (outer, inner) = CompositeKey::parse(key.as_bytes()).unwrap();
    /// assert_eq!((outer.as_slice(), inner.as_slice()), (&b"doc-1"[..], &b"title"[..]));
    /// ```
    pub fn new(outer: &[u8], inner: &[u8]) -> Self {
        Self::new_multi(&[outer, inner])
    }

    /// Create a composite key from three parts (for deeper nesting)
    pub fn new_3(part1: &[u8], part2: &[u8], part3: &[u8]) -> Self {
        Self::new_multi(&[part1, part2, part3])
    }

    /// Create a composite key from multiple parts
    ///
    /// # Example
    ///
    /// ```ignore
    /// let key = CompositeKey::new_multi(&[b"a", b"b", b"c"]);
    /// assert_eq!(CompositeKey::parse_multi(key.as_bytes()).unwrap(), vec![b"a", b"b", b"c"]);
    /// ```
    pub fn new_multi(parts: &[&[u8]]) -> Self {
        let total_len: usize = parts.iter().map(|p| LEN_PREFIX_SIZE + p.len()).sum();

        let mut bytes = Vec::with_capacity(total_len);
        for part in parts {
            encode_part(&mut bytes, part);
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

    /// Parse a composite key back into exactly two parts
    ///
    /// This is the inverse of [`CompositeKey::new`]. Use
    /// [`CompositeKey::parse_multi`] for keys with an arbitrary number of parts.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::Truncated`] if the buffer is malformed, or
    /// [`ParseError::InvalidFormat`] if it does not decode to exactly two parts.
    pub fn parse(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ParseError> {
        let mut parts = Self::parse_multi(bytes)?;
        if parts.len() != 2 {
            return Err(ParseError::InvalidFormat(format!(
                "expected 2 parts, found {}",
                parts.len()
            )));
        }
        let inner = parts.pop().expect("len checked == 2");
        let outer = parts.pop().expect("len checked == 2");
        Ok((outer, inner))
    }

    /// Parse a composite key into all its parts
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::Truncated`] if a length prefix or part body runs
    /// past the end of the buffer.
    pub fn parse_multi(bytes: &[u8]) -> Result<Vec<Vec<u8>>, ParseError> {
        let mut parts = Vec::new();
        let mut offset = 0;

        while offset < bytes.len() {
            let (part, next) = decode_part(bytes, offset)?;
            parts.push(part);
            offset = next;
        }

        Ok(parts)
    }

    /// Check if a key starts with the given prefix
    ///
    /// The prefix must itself be an encoded prefix (e.g. from
    /// [`CompositeKey::prefix_for`]); raw part bytes will not match.
    pub fn has_prefix(&self, prefix: &[u8]) -> bool {
        self.bytes.starts_with(prefix)
    }

    /// Extract the prefix for scanning all keys with a given outer part
    ///
    /// Returns the length-prefixed encoding of `outer_key`. Every key whose
    /// first part is exactly `outer_key` starts with these bytes; keys whose
    /// first part merely *begins* with `outer_key` do not (the length prefix
    /// differs), so scans cannot leak across outer keys.
    pub fn prefix_for(outer_key: &[u8]) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(LEN_PREFIX_SIZE + outer_key.len());
        encode_part(&mut prefix, outer_key);
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
    /// A length prefix or part body ran past the end of the buffer.
    Truncated(String),
    /// The key decoded successfully but did not match the expected shape.
    InvalidFormat(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Truncated(msg) => write!(f, "Truncated composite key: {msg}"),
            ParseError::InvalidFormat(msg) => write!(f, "Invalid composite key format: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

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
        let parts = CompositeKey::parse_multi(key.as_bytes()).unwrap();
        assert_eq!(parts, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn test_composite_key_multi() {
        let key = CompositeKey::new_multi(&[b"x", b"y", b"z"]);
        let parts = CompositeKey::parse_multi(key.as_bytes()).unwrap();
        assert_eq!(parts, vec![b"x".to_vec(), b"y".to_vec(), b"z".to_vec()]);
    }

    #[test]
    fn test_composite_key_parse_multi() {
        let key = CompositeKey::new_multi(&[b"a", b"b", b"c", b"d"]);
        let parts = CompositeKey::parse_multi(key.as_bytes()).unwrap();
        assert_eq!(
            parts,
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]
        );
    }

    #[test]
    fn test_composite_key_prefix_roundtrips_with_new() {
        let key = CompositeKey::new(b"doc-1", b"title");
        assert!(key.has_prefix(&CompositeKey::prefix_for(b"doc-1")));
        assert!(!key.has_prefix(&CompositeKey::prefix_for(b"doc-2")));
    }

    #[test]
    fn test_prefix_scan_does_not_leak_across_outer_keys() {
        // A key whose outer part merely *begins* with the scan prefix must not
        // be matched — the length prefix disambiguates "doc" from "doc-1".
        let key = CompositeKey::new(b"doc-1", b"title");
        assert!(!key.has_prefix(&CompositeKey::prefix_for(b"doc")));
    }

    #[test]
    fn test_no_collision_on_separator_injection() {
        // The historical delimiter scheme collided here; length prefixing must
        // keep these two distinct.
        let a = CompositeKey::new(b"a::b", b"c");
        let b = CompositeKey::new(b"a", b"b::c");
        assert_ne!(a.as_bytes(), b.as_bytes());

        assert_eq!(
            CompositeKey::parse(a.as_bytes()).unwrap(),
            (b"a::b".to_vec(), b"c".to_vec())
        );
        assert_eq!(
            CompositeKey::parse(b.as_bytes()).unwrap(),
            (b"a".to_vec(), b"b::c".to_vec())
        );
    }

    #[test]
    fn test_empty_parts_are_injective() {
        let a = CompositeKey::new(b"", b"::");
        let b = CompositeKey::new(b"::", b"");
        assert_ne!(a.as_bytes(), b.as_bytes());
        assert_eq!(
            CompositeKey::parse(a.as_bytes()).unwrap(),
            (Vec::new(), b"::".to_vec())
        );
    }

    #[test]
    fn test_composite_key_empty() {
        let key = CompositeKey::new_multi(&[]);
        assert!(key.as_bytes().is_empty());
        assert!(CompositeKey::parse_multi(key.as_bytes())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_composite_key_single_part() {
        let key = CompositeKey::new_multi(&[b"only"]);
        let parts = CompositeKey::parse_multi(key.as_bytes()).unwrap();
        assert_eq!(parts, vec![b"only".to_vec()]);
    }

    #[test]
    fn test_parse_requires_exactly_two_parts() {
        let one = CompositeKey::new_multi(&[b"solo"]);
        assert!(matches!(
            CompositeKey::parse(one.as_bytes()),
            Err(ParseError::InvalidFormat(_))
        ));

        let three = CompositeKey::new_3(b"a", b"b", b"c");
        assert!(matches!(
            CompositeKey::parse(three.as_bytes()),
            Err(ParseError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_parse_truncated_length_prefix() {
        // Fewer than LEN_PREFIX_SIZE bytes cannot hold a length.
        assert!(matches!(
            CompositeKey::parse_multi(&[0u8, 0u8]),
            Err(ParseError::Truncated(_))
        ));
    }

    #[test]
    fn test_parse_truncated_part_body() {
        // Declares a 10-byte part but supplies none.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10u32.to_be_bytes());
        assert!(matches!(
            CompositeKey::parse_multi(&bytes),
            Err(ParseError::Truncated(_))
        ));
    }

    #[test]
    fn test_composite_key_with_special_chars() {
        let key = CompositeKey::new(b"doc-#1@!", b"field$%");
        let (outer, inner) = CompositeKey::parse(key.as_bytes()).unwrap();
        assert_eq!(outer, b"doc-#1@!");
        assert_eq!(inner, b"field$%");
    }

    #[test]
    fn test_composite_key_borsh_serialization() {
        let key = CompositeKey::new(b"test", b"value");
        let bytes = borsh::to_vec(&key).unwrap();
        let deserialized: CompositeKey = borsh::from_slice(&bytes).unwrap();
        assert_eq!(deserialized, key);
    }
}
