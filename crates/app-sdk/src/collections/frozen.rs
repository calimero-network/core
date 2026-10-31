//! `Frozen<T>`: write-once values at `(name, "f", hash)`, under the SHA-256 of their content
//! (v1's `FrozenStorage`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::app::StateField;
use crate::commons::collection_key;
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{DefaultStore, Store};

/// How a [`Frozen`] addresses a value: the 32 bytes it is stored under.
pub trait ContentHash<T> {
    /// The address of `value`.
    fn hash(value: &T) -> Result<[u8; 32], StorageError>;
}

/// The default address: SHA-256 of the value's deterministic CBOR, the bytes it is stored as.
pub struct CborSha256;

impl<T: Serialize> ContentHash<T> for CborSha256 {
    fn hash(value: &T) -> Result<[u8; 32], StorageError> {
        Ok(Sha256::digest(value::to_vec(value)?).into())
    }
}

/// Immutable values under their content hash: nothing changes or removes one, and inserting
/// the same content again stores nothing. Hashes travel as 64 lowercase hex digits.
pub struct Frozen<T, H = CborSha256, S: Store = DefaultStore> {
    name: String,
    store: S,
    _types: PhantomData<fn() -> (T, H)>,
}

impl<T, H, S: Store> StateField for Frozen<T, H, S> {
    const KIND: &'static str = "frozen";
}

impl<T: Serialize + DeserializeOwned, H: ContentHash<T>, S: Store + Default> Frozen<T, H, S> {
    /// A frozen store on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: Serialize + DeserializeOwned, H: ContentHash<T>, S: Store> Frozen<T, H, S> {
    /// A frozen store on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _types: PhantomData,
        }
    }

    fn storage_key(&self, hash: [u8; 32]) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "f", &[KeyPart::Bytes(hash.to_vec())])
    }

    /// The hex address `value` has, or would have, here.
    pub fn hash_of(value: &T) -> Result<String, StorageError> {
        Ok(to_hex(&H::hash(value)?))
    }

    /// Store `value` under its hash; returns the hash and whether the value was new.
    pub fn insert(&mut self, value: &T) -> Result<(String, bool), StorageError> {
        let hash = H::hash(value)?;
        let key = self.storage_key(hash)?;
        let fresh = self.store.get(&key).is_none();
        if fresh {
            self.store.put(&key, &value::to_vec(value)?);
        }
        Ok((to_hex(&hash), fresh))
    }

    /// The value under the hex `hash` (either case); `None` when there is none or `hash` is
    /// not 64 hex digits.
    pub fn get(&self, hash: &str) -> Result<Option<T>, StorageError> {
        let Some(hash) = from_hex(hash) else {
            return Ok(None);
        };
        self.store
            .get(&self.storage_key(hash)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Whether a value is stored under the hex `hash`.
    pub fn contains(&self, hash: &str) -> Result<bool, StorageError> {
        let Some(hash) = from_hex(hash) else {
            return Ok(false);
        };
        Ok(self.store.get(&self.storage_key(hash)?).is_some())
    }
}

fn to_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    let text = text.as_bytes();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(text.as_chunks::<2>().0) {
        let pair = core::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    #[test]
    fn insert_is_write_once_and_content_addressed() {
        let store = MemStore::new();
        let mut f: Frozen<String, CborSha256, MemStore> = Frozen::with_store("f", store.clone());
        let (h1, fresh) = f.insert(&"hello".into()).unwrap();
        assert!(fresh);
        assert_eq!(h1.len(), 64);
        assert_eq!(
            h1,
            Frozen::<String, CborSha256, MemStore>::hash_of(&"hello".into()).unwrap()
        );
        let writes = store.writes();
        assert_eq!(f.insert(&"hello".into()).unwrap(), (h1.clone(), false));
        assert_eq!(store.writes(), writes, "the same content writes nothing");
        assert_eq!(f.get(&h1).unwrap().as_deref(), Some("hello"));
        assert_eq!(f.get(&h1.to_uppercase()).unwrap().as_deref(), Some("hello"));
        assert!(f.contains(&h1).unwrap());
        let (h2, _) = f.insert(&"world".into()).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn the_default_hash_is_sha256_of_the_stored_cbor() {
        // "abc" as CBOR is 0x63 'a' 'b' 'c'.
        let expected: String = Sha256::digest(b"\x63abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            Frozen::<String, CborSha256, MemStore>::hash_of(&"abc".into()).unwrap(),
            expected
        );
    }

    #[test]
    fn malformed_and_unknown_hashes_read_nothing() {
        let f: Frozen<String, CborSha256, MemStore> = Frozen::with_store("f", MemStore::new());
        assert_eq!(f.get("nothex").unwrap(), None);
        assert_eq!(f.get(&"zz".repeat(32)).unwrap(), None);
        assert_eq!(f.get(&"00".repeat(32)).unwrap(), None);
        assert!(!f.contains("").unwrap());
    }

    /// An app keeping v1's hashes: SHA-256 of the borsh encoding of a string.
    struct BorshSha256;

    impl ContentHash<String> for BorshSha256 {
        fn hash(value: &String) -> Result<[u8; 32], StorageError> {
            let mut hasher = Sha256::new();
            hasher.update((value.len() as u32).to_le_bytes());
            hasher.update(value.as_bytes());
            Ok(hasher.finalize().into())
        }
    }

    #[test]
    fn a_custom_hash_addresses_the_value() {
        let mut f: Frozen<String, BorshSha256, MemStore> = Frozen::with_store("f", MemStore::new());
        let (hash, _) = f.insert(&"SomeFrozenString".into()).unwrap();
        assert_eq!(
            hash,
            "c8acd3c44a96324de12a82dd674aa1c93a635fbb76a852ae14363f9b1971beb1"
        );
    }
}
