use std::cell::Cell;
use std::fmt;
use std::str::FromStr;

use sha2::Digest;

const BYTES_LEN: usize = 32;
const MAX_STR_LEN: usize = 44;

#[derive(Clone, Default)]
pub struct Hash {
    bytes: [u8; BYTES_LEN],
    bs58: Cell<Option<(usize, [u8; MAX_STR_LEN])>>,
}

impl Hash {
    pub fn as_bytes(&self) -> &[u8; BYTES_LEN] {
        &self.bytes
    }

    pub fn hash(data: &[u8]) -> Self {
        Self {
            bytes: sha2::Sha256::digest(data).into(),
            bs58: Default::default(),
        }
    }

    pub fn hash_json<T: serde::Serialize>(data: &T) -> Self {
        let mut hasher = sha2::Sha256::default();
        serde_json::to_writer(&mut hasher, data).unwrap();
        Hash {
            bytes: hasher.finalize().into(),
            bs58: Default::default(),
        }
    }

    // todo! pub fn hash_borsh

    pub fn as_str(&self) -> &str {
        let bs58 = unsafe { &mut *self.bs58.as_ptr() };
        if bs58.is_none() {
            let mut buf = [0; MAX_STR_LEN];
            let len = bs58::encode(&self.bytes).onto(&mut buf[..]).unwrap();
            *bs58 = Some((len, buf));
        }

        let (len, bs58) = bs58.as_ref().unwrap();
        std::str::from_utf8(&bs58[..*len]).unwrap()
    }

    fn from_str(s: &str) -> Result<Self, Option<bs58::decode::Error>> {
        let mut bytes = [0; BYTES_LEN];
        let mut bs58 = [0; MAX_STR_LEN];
        bs58.copy_from_slice(s.as_bytes());
        match bs58::decode(s).onto(&mut bytes) {
            Ok(len) if len == bytes.len() => Ok(Self {
                bytes,
                bs58: Cell::new(Some((s.len(), bs58))),
            }),
            Ok(_) => Err(None),
            Err(err) => Err(Some(err)),
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

impl FromStr for Hash {
    type Err = String; // todo! use a better-typed error

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::from_str(s) {
            Ok(hash) => Ok(hash),
            Err(None) => Err("invalid length".to_string()),
            Err(Some(err)) => Err(err.to_string()),
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
