//! Blobs (R8): what the host core asks the node about an app's blobs, and how a stored
//! value names one.
//!
//! A blob's bytes never cross the WIT boundary: clients upload and download them through
//! the node's API, and a run sees only their metadata (`info`) and can hand out a
//! time-limited download grant (`grant`). A value in state refers to a blob through a typed
//! `BlobRef`, which both SDKs encode as CBOR tag [`BLOB_REF_TAG`] around the blob's 32-byte
//! SHA-256, so the host finds every reference without knowing the app's types and keeps a
//! blob for as long as state refers to it (host-tracked retention).

use ciborium::Value;

use crate::types::BlobInfo;

/// The CBOR tag of a `BlobRef` in a stored value: tag 51,739 (`0xCA1B`, unassigned in the
/// IANA registry), content a byte string of 32 bytes (the blob's SHA-256). Part of the
/// encoding contract both SDKs share (D10).
pub const BLOB_REF_TAG: u64 = 0xCA1B;

/// The longest grant a run may hand out, in seconds (one day).
pub const MAX_GRANT_TTL_SECS: u32 = 86_400;

/// The bytes a value holding a `BlobRef` contains: the tag's head (major type 6, two-byte
/// argument) followed by a 32-byte byte string's head. A value without them is not decoded.
const REF_HEAD: [u8; 5] = [0xD9, 0xCA, 0x1B, 0x58, 0x20];

/// What the host core asks the node about the app's blobs. The node answers from the
/// replica's record of the app's blobs at the state the run reads.
pub trait BlobHost: Send + Sync {
    /// The blob's metadata, if it is one of the app's blobs.
    fn info(&self, id: &[u8; 32]) -> Option<BlobInfo>;
    /// A download grant for the blob, valid until `expires_ms` (the run's wall clock plus
    /// the TTL). Only called for a blob `info` knows.
    fn grant(&self, id: &[u8; 32], expires_ms: u64) -> String;
}

/// The host of an app with no blob store (and the default of every run): no blobs.
pub struct NoBlobs;

impl BlobHost for NoBlobs {
    fn info(&self, _id: &[u8; 32]) -> Option<BlobInfo> {
        None
    }

    fn grant(&self, _id: &[u8; 32], _expires_ms: u64) -> String {
        String::new()
    }
}

/// A blob id as the WIT `blob-id` carries it: 64 lowercase hex digits.
#[must_use]
pub fn parse_blob_id(id: &str) -> Option<[u8; 32]> {
    if id.len() != 64 || !id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = id.get(2 * i..2 * i + 2)?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// The blobs a stored value refers to, each once per reference, in the order they appear.
/// A value that is not CBOR, or holds no `BlobRef`, refers to none.
#[must_use]
pub fn blob_refs(value: &[u8]) -> Vec<[u8; 32]> {
    if !value.windows(REF_HEAD.len()).any(|w| w == REF_HEAD) {
        return Vec::new();
    }
    let Ok(decoded) = ciborium::from_reader::<Value, _>(value) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect(&decoded, &mut out);
    out
}

fn collect(value: &Value, out: &mut Vec<[u8; 32]>) {
    match value {
        Value::Tag(BLOB_REF_TAG, inner) => {
            if let Value::Bytes(b) = inner.as_ref() {
                if let Ok(id) = <[u8; 32]>::try_from(b.as_slice()) {
                    out.push(id);
                }
            }
        }
        Value::Tag(_, inner) => collect(inner, out),
        Value::Array(items) => items.iter().for_each(|v| collect(v, out)),
        Value::Map(entries) => entries.iter().for_each(|(k, v)| {
            collect(k, out);
            collect(v, out);
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged(id: [u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::into_writer(
            &Value::Map(vec![(
                Value::Text("file".into()),
                Value::Tag(BLOB_REF_TAG, Box::new(Value::Bytes(id.to_vec()))),
            )]),
            &mut out,
        )
        .unwrap();
        out
    }

    #[test]
    fn a_tagged_reference_is_found_and_plain_bytes_are_not() {
        assert_eq!(blob_refs(&tagged([7; 32])), vec![[7; 32]]);
        let mut plain = Vec::new();
        ciborium::into_writer(&Value::Bytes(vec![7; 32]), &mut plain).unwrap();
        assert!(blob_refs(&plain).is_empty());
        assert!(blob_refs(b"\xD9\xCA\x1B\x58\x20 not cbor").is_empty());
    }

    #[test]
    fn blob_ids_are_lowercase_hex() {
        let id = "ab".repeat(32);
        assert_eq!(parse_blob_id(&id), Some([0xAB; 32]));
        assert_eq!(parse_blob_id(&"AB".repeat(32)), None);
        assert_eq!(parse_blob_id("ab"), None);
    }
}
