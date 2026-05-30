use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{FrozenStorage, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: a content-addressed `FrozenStorage` of immutable documents plus a
/// plain `LwwRegister` title. The migrate scenario carries the `FrozenStorage`
/// through to v2 unchanged — the cross-node assertion is that every stored
/// document is still retrievable under its original content hash after the
/// migration, on every node (the key IS the SHA-256 of the value, so identical
/// content always round-trips to the identical hash).
#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFrozenStorageV1 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
}

/// `FrozenStorage::insert` returns the content hash as a raw `[u8; 32]`. We
/// surface it to workflows as a string by wrapping it in a `PublicKey` (a thin
/// `[u8; 32]` newtype re-exported by the SDK) purely for its base58
/// `Display`/`FromStr` round-trip — it is used here only as a bytes<->string
/// codec, not as an identity.
fn hash_to_string(hash: [u8; 32]) -> String {
    PublicKey::from(hash).to_string()
}

/// Parses a base58 hash string back into the raw `[u8; 32]` key. Returns `None`
/// if the string is not a valid encoded hash.
fn string_to_hash(hash_str: &str) -> Option<[u8; 32]> {
    hash_str.parse::<PublicKey>().ok().map(|pk| *pk.as_ref())
}

#[app::logic]
impl ScenarioFrozenStorageV1 {
    #[app::init]
    pub fn init() -> ScenarioFrozenStorageV1 {
        ScenarioFrozenStorageV1 {
            documents: FrozenStorage::new_with_field_name("documents"),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Freeze a document and return its content hash as a base58 string.
    pub fn freeze_doc(&mut self, value: String) -> app::Result<String> {
        let hash = self.documents.insert(value)?;
        Ok(hash_to_string(hash))
    }

    /// Retrieve a frozen document by its content-hash string. A malformed hash
    /// string yields `Ok(None)` rather than an error.
    pub fn get_doc(&self, hash_str: String) -> app::Result<Option<String>> {
        match string_to_hash(&hash_str) {
            Some(hash) => Ok(self.documents.get(&hash)?),
            None => Ok(None),
        }
    }

    /// Whether a document with the given content-hash string exists.
    pub fn has_doc(&self, hash_str: String) -> app::Result<bool> {
        match string_to_hash(&hash_str) {
            Some(hash) => Ok(self.documents.contains(&hash)?),
            None => Ok(false),
        }
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
        })
    }
}
