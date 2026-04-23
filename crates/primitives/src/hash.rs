#[cfg(test)]
#[path = "tests/hash.rs"]
mod tests;

use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash as StdHash, Hasher};
use core::ops::Deref;
use core::str::{from_utf8_unchecked, FromStr};
#[cfg(feature = "borsh")]
use std::io;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use bs58::decode::Error as Bs58Error;
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{to_writer as to_json_writer, Result as JsonResult};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;

const BYTES_LEN: usize = 32;
#[expect(clippy::integer_division, reason = "Not harmful here")]
// https://github.com/bitcoin/libbase58/blob/master/base58.c#L155
const MAX_STR_LEN: usize = (BYTES_LEN * 138 / 100) + 1;

/// A 32-byte cryptographic digest that displays as base58.
///
/// The base58 representation is computed on demand rather than cached on
/// construction. This makes `Hash::from([u8; 32])` a cheap memcpy and
/// shrinks the struct from ~80 bytes to 32, which matters on hot paths
/// that construct IDs just to compare or hash them (delta-store
/// iteration, RocksDB key parsing, etc.). Call sites that need the
/// string form can use [`Display`] (zero-alloc) or [`Self::to_base58`]
/// (allocates). For writing the string into an existing buffer without
/// allocating, see [`Self::encode_base58`].
#[derive(Clone, Copy)]
pub struct Hash {
    bytes: [u8; BYTES_LEN],
}

impl Hash {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    /// All-zero digest. Cheap — no base58 work on construction.
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
        let mut hasher = Sha256::default();
        to_json_writer(&mut hasher, data)?;
        let hash_bytes: [u8; BYTES_LEN] = hasher.finalize().into();
        Ok(Self { bytes: hash_bytes })
    }

    #[cfg(feature = "borsh")]
    pub fn hash_borsh<T: BorshSerialize>(data: &T) -> io::Result<Self> {
        let mut hasher = Sha256::default();
        data.serialize(&mut hasher)?;
        let hash_bytes: [u8; BYTES_LEN] = hasher.finalize().into();
        Ok(Self { bytes: hash_bytes })
    }

    /// Returns the base58 representation as a freshly allocated
    /// `String`. Use [`Self::encode_base58`] when you already have a
    /// stack buffer and want to avoid the allocation.
    #[must_use]
    pub fn to_base58(&self) -> String {
        let mut buf = [0u8; MAX_STR_LEN];
        let s = self.encode_base58(&mut buf);
        s.to_owned()
    }

    /// Writes the base58 representation into `buf` and returns it as a
    /// `&str` borrowing from `buf`. Zero allocation; intended for hot
    /// paths that can bring their own buffer.
    ///
    /// # Panics
    ///
    /// Panics if base58 encoding fails, which cannot happen given the
    /// fixed 32-byte input and correctly-sized output buffer.
    #[must_use]
    pub fn encode_base58<'a>(&self, buf: &'a mut [u8; MAX_STR_LEN]) -> &'a str {
        let len = bs58::encode(&self.bytes)
            .onto(&mut buf[..])
            .expect("Base58 encoding failed");
        // SAFETY: bs58 alphabet is pure ASCII.
        unsafe { from_utf8_unchecked(&buf[..len]) }
    }

    fn from_str(s: &str) -> Result<Self, Option<Bs58Error>> {
        if s.len() > MAX_STR_LEN {
            return Err(Some(Bs58Error::BufferTooSmall));
        }

        let mut bytes = [0; BYTES_LEN];
        match bs58::decode(s).onto(&mut bytes) {
            Ok(len) if len == bytes.len() => Ok(Self { bytes }),
            Ok(_) => Err(None),
            Err(err) => Err(Some(err)),
        }
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
        let mut buf = [0u8; MAX_STR_LEN];
        f.pad(self.encode_base58(&mut buf))
    }
}

impl Debug for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; MAX_STR_LEN];
        f.debug_tuple("Hash")
            .field(&self.encode_base58(&mut buf))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[non_exhaustive]
pub enum HashError {
    #[error("invalid hash length")]
    InvalidLength,

    #[error("invalid base58")]
    DecodeError(#[from] Bs58Error),
}

impl FromStr for Hash {
    type Err = HashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::from_str(s) {
            Ok(hash) => Ok(hash),
            Err(None) => Err(HashError::InvalidLength),
            Err(Some(err)) => Err(HashError::DecodeError(err)),
        }
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
        let mut buf = [0u8; MAX_STR_LEN];
        serializer.serialize_str(self.encode_base58(&mut buf))
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HashVisitor;

        impl Visitor<'_> for HashVisitor {
            type Value = Hash;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a base58 encoded hash")
            }

            fn visit_str<E: SerdeError>(self, v: &str) -> Result<Self::Value, E> {
                match Hash::from_str(v) {
                    Ok(hash) => Ok(hash),
                    Err(None) => Err(E::invalid_length(v.len(), &self)),
                    Err(Some(err)) => Err(E::custom(err)),
                }
            }
        }

        deserializer.deserialize_str(HashVisitor)
    }
}
