#[cfg(test)]
#[path = "tests/hash.rs"]
mod tests;

use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash as StdHash, Hasher};
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::str::{from_utf8, FromStr};
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
const MAX_STR_LEN: usize = (BYTES_LEN + 1) * 4 / 3;

#[derive(Clone, Copy)]
pub struct Hash {
    // todo! consider genericizing over a const N
    bytes: [u8; BYTES_LEN],
    bs58: MaybeUninit<(usize, [u8; MAX_STR_LEN])>,
}

impl Hash {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    // todo! genericize over D: Digest
    #[must_use]
    pub fn new(data: &[u8]) -> Self {
        Self {
            bytes: Sha256::digest(data).into(),
            bs58: MaybeUninit::zeroed(),
        }
    }

    // todo! genericize over D: Digest
    pub fn hash_json<T: Serialize>(data: &T) -> JsonResult<Self> {
        let mut hasher = Sha256::default();

        to_json_writer(&mut hasher, data)?;

        Ok(Self {
            bytes: hasher.finalize().into(),
            bs58: MaybeUninit::zeroed(),
        })
    }

    #[cfg(feature = "borsh")]
    pub fn hash_borsh<T: BorshSerialize>(data: &T) -> io::Result<Self> {
        let mut hasher = Sha256::default();

        data.serialize(&mut hasher)?;

        Ok(Self {
            bytes: hasher.finalize().into(),
            bs58: MaybeUninit::zeroed(),
        })
    }

    // todo! using generic-array;
    // todo! as_str(&self, buf: &mut [u8; N]) -> &str
    #[must_use]
    pub fn as_str(&self) -> &str {
        let (stored_len, bs58) = unsafe { &mut *self.bs58.as_ptr().cast_mut() };

        let mut len = *stored_len;

        if len == 0 {
            len = bs58::encode(&self.bytes).onto(&mut bs58[..]).unwrap();
            *stored_len = len;
        }

        from_utf8(&bs58[..len]).unwrap()
    }

    fn from_str(s: &str) -> Result<Self, Option<Bs58Error>> {
        let mut bytes = [0; BYTES_LEN];
        let mut bs58 = [0; MAX_STR_LEN];
        let len = s.len().min(MAX_STR_LEN);
        bs58[..len].copy_from_slice(&s.as_bytes()[..len]);
        match bs58::decode(s).onto(&mut bytes) {
            Ok(len) if len == bytes.len() => Ok(Self {
                bytes,
                bs58: MaybeUninit::new((s.len(), bs58)),
            }),
            Ok(_) => Err(None),
            Err(err) => Err(Some(err)),
        }
    }
}

// todo! re-evaluate controlled construction
impl From<[u8; BYTES_LEN]> for Hash {
    fn from(bytes: [u8; BYTES_LEN]) -> Self {
        Self {
            bytes,
            bs58: MaybeUninit::zeroed(),
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
        Self {
            bytes: [0; BYTES_LEN],
            bs58: MaybeUninit::zeroed(),
        }
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
        Ok(Self {
            bytes,
            bs58: MaybeUninit::zeroed(),
        })
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
