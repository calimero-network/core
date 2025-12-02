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

#[derive(Clone, Copy)]
pub struct Hash {
    bytes: [u8; BYTES_LEN],
    bs58_cache: [u8; MAX_STR_LEN],
    bs58_len: u8,
}

impl Hash {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    #[must_use]
    pub fn new(data: &[u8]) -> Self {
        let hash_bytes: [u8; BYTES_LEN] = Sha256::digest(data).into();
        // Will call `From<[u8; 32]>` which computes the cache
        hash_bytes.into()
    }

    pub fn hash_json<T: Serialize>(data: &T) -> JsonResult<Self> {
        let mut hasher = Sha256::default();

        to_json_writer(&mut hasher, data)?;

        // Get the raw [u8; 32] bytes from the hasher
        let hash_bytes: [u8; BYTES_LEN] = hasher.finalize().into();

        // Let `From<[u8; 32]>` handle construction and caching
        Ok(hash_bytes.into())
    }

    #[cfg(feature = "borsh")]
    pub fn hash_borsh<T: BorshSerialize>(data: &T) -> io::Result<Self> {
        let mut hasher = Sha256::default();

        data.serialize(&mut hasher)?;

        // Get the raw [u8; 32] bytes from the hasher
        let hash_bytes: [u8; BYTES_LEN] = hasher.finalize().into();

        // Let `From<[u8; 32]>` handle construction and caching
        Ok(hash_bytes.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Safe: Read from the pre-computed cache.
        let s = &self.bs58_cache[..self.bs58_len as usize];

        // We can trust this is valid UTF-8 because we are the ones
        // who put it there during initialization.
        // Using from_utf8().unwrap() is also perfectly fine.
        unsafe { from_utf8_unchecked(s) }
    }

    fn from_str(s: &str) -> Result<Self, Option<Bs58Error>> {
        let s_len = s.len();
        if s_len > MAX_STR_LEN {
            return Err(Some(Bs58Error::BufferTooSmall));
        }

        let mut bytes = [0; BYTES_LEN];
        match bs58::decode(s).onto(&mut bytes) {
            Ok(len) if len == bytes.len() => {
                let mut bs58_cache = [0; MAX_STR_LEN];
                bs58_cache[..s_len].copy_from_slice(s.as_bytes());

                Ok(Self {
                    bytes,
                    bs58_cache,
                    bs58_len: s_len.try_into().expect("infallible conversion: checked before string length is less than MAX_STR_LEN"),
                })
            }
            Ok(_) => Err(None),
            Err(err) => Err(Some(err)),
        }
    }
}

impl From<[u8; BYTES_LEN]> for Hash {
    fn from(bytes: [u8; BYTES_LEN]) -> Self {
        let mut bs58_cache = [0; MAX_STR_LEN];
        let len = bs58::encode(&bytes)
            .onto(&mut bs58_cache[..])
            // Panics if MAX_STR_LEN is wrong, which is good.
            .expect("Base58 encoding failed");

        Self {
            bytes,
            bs58_cache,
            // Safe: max len is 45
            bs58_len: len
                .try_into()
                .expect("infaliible conversion: bs58_len conversion failed, but shouldn't have"),
        }
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
        // Create the default byte array
        const DEFAULT_BYTES: [u8; BYTES_LEN] = [0; BYTES_LEN];

        // Let `From<[u8; 32]>` handle construction and caching
        DEFAULT_BYTES.into()
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
        f.pad(self.as_str())
    }
}

impl Debug for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hash").field(&self.as_str()).finish()
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

        // Let `From<[u8; 32]>` handle construction and caching
        Ok(bytes.into())
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
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
