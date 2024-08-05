use std::fmt;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::str::FromStr;

use sha2::Digest;
use thiserror::Error;

const BYTES_LEN: usize = 32;
const MAX_STR_LEN: usize = (BYTES_LEN + 1) * 4 / 3;

#[derive(Copy, Clone)]
pub struct Hash {
    // todo! consider genericizing over a const N
    bytes: [u8; BYTES_LEN],
    bs58: MaybeUninit<(usize, [u8; MAX_STR_LEN])>,
}

impl Hash {
    pub fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    // todo! genericize over D: Digest
    pub fn new(data: &[u8]) -> Self {
        Self {
            bytes: sha2::Sha256::digest(data).into(),
            bs58: MaybeUninit::zeroed(),
        }
    }

    // todo! genericize over D: Digest
    pub fn hash_json<T: serde::Serialize>(data: &T) -> serde_json::Result<Self> {
        let mut hasher = sha2::Sha256::default();

        serde_json::to_writer(&mut hasher, data)?;

        Ok(Hash {
            bytes: hasher.finalize().into(),
            bs58: MaybeUninit::zeroed(),
        })
    }

    // todo! pub fn hash_borsh

    // todo! using generic-array;
    // todo! as_str(&self, buf: &mut [u8; N]) -> &str
    pub fn as_str(&self) -> &str {
        let (len, bs58) = unsafe { &mut *self.bs58.as_ptr().cast_mut() };

        if *len == 0 {
            *len = bs58::encode(&self.bytes).onto(&mut bs58[..]).unwrap();
        }

        std::str::from_utf8(&bs58[..*len]).unwrap()
    }

    fn from_str(s: &str) -> Result<Self, Option<bs58::decode::Error>> {
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

impl std::hash::Hash for Hash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
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
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hash {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hash").field(&self.as_str()).finish()
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid hash length")]
    InvalidLength,

    #[error("invalid base58")]
    DecodeError(#[from] bs58::decode::Error),
}

impl FromStr for Hash {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::from_str(s) {
            Ok(hash) => Ok(hash),
            Err(None) => Err(Error::InvalidLength),
            Err(Some(err)) => Err(Error::DecodeError(err)),
        }
    }
}

impl serde::Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Hash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HashVisitor;

        impl<'de> serde::de::Visitor<'de> for HashVisitor {
            type Value = Hash;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a base58 encoded hash")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_43() {
        let hash = Hash::new(b"Hello, World");

        assert_eq!(
            hex::encode(hash.as_bytes()),
            "03675ac53ff9cd1535ccc7dfcdfa2c458c5218371f418dc136f2d19ac1fbe8a5"
        );

        assert_eq!(hash.as_str(), "EHdZfnzn717B56XYH8sWLAHfDC3icGEkccNzpAF4PwS");
        assert_eq!(
            (*&*&*&*&*&*&hash).as_str(),
            "EHdZfnzn717B56XYH8sWLAHfDC3icGEkccNzpAF4PwS"
        );
    }

    #[test]
    fn test_hash_44() {
        let hash = Hash::new(b"Hello World");

        assert_eq!(
            hex::encode(hash.as_bytes()),
            "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
        );

        assert_eq!(
            hash.as_str(),
            "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
        );

        assert_eq!(
            (*&*&*&*&*&*&hash).as_str(),
            "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
        );
    }

    #[test]
    fn test_serde() {
        let hash = Hash::new(b"Hello World");

        assert_eq!(
            serde_json::to_string(&hash).unwrap(),
            "\"C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK\""
        );

        assert_eq!(
            serde_json::from_value::<Hash>(serde_json::json!(
                "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
            ))
            .unwrap(),
            hash
        );
    }
}
