//! Blobs (R8): a typed reference to an uploaded blob, kept in state like any value.
//!
//! Clients upload a blob's bytes to the node (`POST /apps/{app}/blobs`) and pass its id to a
//! method, which stores a [`BlobRef`]. The host finds every `BlobRef` in stored values and
//! keeps the blob for as long as state refers to one (host-tracked retention); a reference
//! to a blob the app does not hold traps `HostDenied`. A run reads the blob's metadata with
//! [`BlobRef::info`] and hands out a time-limited download link with [`BlobRef::grant`]; the
//! bytes themselves never enter the component.
//!
//! Encoding (the TypeScript SDK must match): in stored CBOR a `BlobRef` is tag 51,739
//! (`0xCA1B`) around a 32-byte byte string, the blob's SHA-256; in JSON (arguments, results,
//! events) it is the blob id, 64 lowercase hex digits.

use std::fmt;

use ciborium::tag::Required;
use ciborium::Value;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::env;

/// The CBOR tag of a `BlobRef` (the host's `BLOB_REF_TAG`).
pub const BLOB_REF_TAG: u64 = 0xCA1B;

/// A reference to one of the app's blobs: its SHA-256.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobRef([u8; 32]);

/// A blob's metadata, as the node recorded it at upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobInfo {
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256 (the id).
    pub sha256: Vec<u8>,
    /// The content type the uploader gave.
    pub mime: String,
}

impl BlobRef {
    /// The reference to blob `id` (64 lowercase hex digits, what an upload answers as `id`).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        if id.len() != 64 || !id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut out = [0_u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(id.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }

    /// The reference to the blob with this SHA-256.
    #[must_use]
    pub const fn from_sha256(sha256: [u8; 32]) -> Self {
        Self(sha256)
    }

    /// The blob's SHA-256.
    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.0
    }

    /// The blob id: 64 lowercase hex digits.
    #[must_use]
    pub fn id(&self) -> String {
        use std::fmt::Write as _;
        self.0.iter().fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    /// The blob's metadata; none when the app does not hold it (never uploaded, or deleted
    /// by its owner). Not in actions or migrations (`HostDenied`).
    #[must_use]
    pub fn info(&self) -> Option<BlobInfo> {
        env::blob_info(&self.id())
    }

    /// A download grant valid for `ttl_seconds` (1 to 86,400) from the run's clock: a client
    /// downloads with `GET /apps/{app}/blobs/{id}?grant=<grant>`, signed in or not. Traps
    /// `HostDenied` for a blob the app does not hold or a TTL out of range.
    #[must_use]
    pub fn grant(&self, ttl_seconds: u32) -> String {
        env::blob_grant(&self.id(), ttl_seconds)
    }
}

impl fmt::Debug for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobRef({})", self.id())
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id())
    }
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            return s.serialize_str(&self.id());
        }
        Required::<Value, BLOB_REF_TAG>(Value::Bytes(self.0.to_vec())).serialize(s)
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if d.is_human_readable() {
            let id = String::deserialize(d)?;
            return Self::parse(&id).ok_or_else(|| D::Error::custom("not a blob id"));
        }
        let Required(value) = Required::<Value, BLOB_REF_TAG>::deserialize(d)?;
        match value {
            Value::Bytes(b) => <[u8; 32]>::try_from(b.as_slice())
                .map(Self)
                .map_err(|_| D::Error::custom("a BlobRef holds 32 bytes")),
            _ => Err(D::Error::custom("a BlobRef holds a byte string")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::value;

    #[test]
    fn a_ref_is_a_tagged_byte_string_in_cbor_and_hex_in_json() {
        let r = BlobRef::from_sha256([0xAB; 32]);
        let cbor = value::to_vec(&r).unwrap();
        assert_eq!(&cbor[..5], &[0xD9, 0xCA, 0x1B, 0x58, 0x20]);
        assert_eq!(value::from_slice::<BlobRef>(&cbor).unwrap(), r);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(serde_json::from_str::<BlobRef>(&json).unwrap(), r);
        assert!(serde_json::from_str::<BlobRef>("\"AB\"").is_err());
    }
}
