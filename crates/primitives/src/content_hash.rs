#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use core::fmt;
use core::str::FromStr;
use thiserror::Error as ThisError;

/// Byte length of a content hash (a sha256 digest).
const BYTES_LEN: usize = 32;

/// The digest of a file's bytes, straight through: `sha256(content)`.
///
/// Distinct from [`crate::blobs::BlobId`] (which hashes the file's *chunk ids*):
/// conversion is neither implicit nor single-step, so no accidental swap.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContentHash([u8; BYTES_LEN]);

impl From<[u8; BYTES_LEN]> for ContentHash {
    fn from(bytes: [u8; BYTES_LEN]) -> Self {
        Self(bytes)
    }
}

// Beyond the wrapped byte array's own AsRef<[u8]>: bridges a ContentHash
// back into a plain [u8; 32] field without an unwrap.
impl From<ContentHash> for [u8; BYTES_LEN] {
    fn from(hash: ContentHash) -> Self {
        hash.0
    }
}

impl AsRef<[u8]> for ContentHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for ContentHash {
    type Err = InvalidContentHash;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0_u8; BYTES_LEN];
        hex::decode_to_slice(s, &mut out).map_err(|_| InvalidContentHash)?;
        Ok(Self(out))
    }
}

/// A content hash must be exactly 64 hex characters.
#[derive(Debug, ThisError)]
#[error("expected 64 hex characters")]
pub struct InvalidContentHash;

#[cfg(test)]
mod tests {
    use super::ContentHash;

    #[test]
    fn round_trips_through_hex() {
        let h = ContentHash::from([0xAB; 32]);
        let s = h.to_string();
        assert_eq!(s.len(), 64, "32 bytes render as 64 hex chars");
        assert_eq!(s.parse::<ContentHash>().expect("parses back"), h);
    }

    #[test]
    fn rejects_a_wrong_length_hex_string() {
        assert!("abcd".parse::<ContentHash>().is_err());
        assert!("zz".repeat(32).parse::<ContentHash>().is_err());
    }
}
