#[cfg(test)]
#[path = "tests/hash.rs"]
mod tests;

use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash as StdHash, Hasher};
use core::ops::Deref;
use core::str::FromStr;
#[cfg(feature = "borsh")]
use std::io;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{to_vec as to_json_vec, Result as JsonResult};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

const BYTES_LEN: usize = 32;
/// A 32-byte cryptographic digest that displays as 64 lowercase hex characters.
///
/// Every id in this crate is this type or a newtype over it, and they all spell
/// their bytes this one way — see `tests/encoding.rs`, which pins that across
/// types rather than per type.
///
/// The string form is computed on demand rather than cached on construction.
/// This makes `Hash::from([u8; 32])` a cheap memcpy and shrinks the struct from
/// ~80 bytes to 32, which matters on hot paths that construct IDs just to
/// compare or hash them (delta-store iteration, RocksDB key parsing, etc.).
///
/// Hex also makes that on-demand encoding trivial where base58 was not: it is a
/// per-byte mapping rather than a bignum base conversion, so it needs no length
/// bound and no scratch buffer, which is why the `to_base58`/`encode_base58`
/// pair this type used to carry is gone with nothing in its place.
#[derive(Clone, Copy)]
pub struct Hash {
    bytes: [u8; BYTES_LEN],
}

impl Hash {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    /// All-zero digest. Cheap — no string work on construction.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            bytes: [0u8; BYTES_LEN],
        }
    }

    #[must_use]
    pub fn new(data: &[u8]) -> Self {
        let hash_bytes: [u8; BYTES_LEN] = Sha256::digest(data).into();
        Self { bytes: hash_bytes }
    }

    pub fn is_zero(&self) -> bool {
        self.bytes.iter().all(|&byte| byte == 0)
    }

    pub fn hash_json<T: Serialize>(data: &T) -> JsonResult<Self> {
        Ok(Self::new(&to_json_vec(data)?))
    }

    #[cfg(feature = "borsh")]
    pub fn hash_borsh<T: BorshSerialize>(data: &T) -> io::Result<Self> {
        Ok(Self::new(&borsh::to_vec(data)?))
    }

    /// Decode the hex form [`Display`] writes.
    ///
    /// Hex, not base58, and the distinction is load-bearing rather than
    /// cosmetic: base58's alphabet contains every hex digit except `0`, so a hex
    /// id handed to a base58 decoder is frequently *valid* and decodes to the
    /// wrong 32 bytes silently — `"11".repeat(32)` becomes all zeros. Hex handed
    /// to a base58 decoder fails loudly instead. For ids that authorise things,
    /// wrong-and-loud beats wrong-and-quiet.
    fn from_hex(s: &str) -> Result<Self, HashError> {
        let bytes = hex::decode(s).map_err(|_ignored| HashError::InvalidHex)?;
        let bytes: [u8; BYTES_LEN] = bytes
            .try_into()
            .map_err(|_ignored| HashError::InvalidLength)?;
        Ok(Self { bytes })
    }
}

impl From<[u8; BYTES_LEN]> for Hash {
    fn from(bytes: [u8; BYTES_LEN]) -> Self {
        Self { bytes }
    }
}

impl From<Hash> for [u8; BYTES_LEN] {
    fn from(hash: Hash) -> Self {
        hash.bytes
    }
}

impl AsRef<[u8; BYTES_LEN]> for Hash {
    fn as_ref(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }
}

impl Deref for Hash {
    type Target = [u8; BYTES_LEN];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl Default for Hash {
    fn default() -> Self {
        Self::zero()
    }
}

impl StdHash for Hash {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl PartialEq for Hash {
    fn eq(&self, other: &Self) -> bool {
        self.bytes.eq(&other.bytes)
    }
}

impl Eq for Hash {}

impl PartialOrd for Hash {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hash {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl Display for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(&hex::encode(self.bytes))
    }
}

impl Debug for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hash")
            .field(&hex::encode(self.bytes))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[non_exhaustive]
pub enum HashError {
    #[error("invalid hash length")]
    InvalidLength,

    #[error("expected 64 hex characters (32 bytes)")]
    InvalidHex,
}

impl FromStr for Hash {
    type Err = HashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

#[cfg(feature = "borsh")]
impl BorshSerialize for Hash {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&self.bytes)
    }
}

#[cfg(feature = "borsh")]
impl BorshDeserialize for Hash {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let mut bytes = [0; BYTES_LEN];
        reader.read_exact(&mut bytes)?;
        Ok(Self { bytes })
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.bytes))
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HashVisitor;

        impl Visitor<'_> for HashVisitor {
            type Value = Hash;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a hex encoded hash")
            }

            fn visit_str<E: SerdeError>(self, v: &str) -> Result<Self::Value, E> {
                Hash::from_hex(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HashVisitor)
    }
}
