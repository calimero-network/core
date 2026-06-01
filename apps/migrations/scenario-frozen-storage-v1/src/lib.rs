use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{FrozenStorage, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: a content-addressed `FrozenStorage` of immutable documents, a plain
/// `LwwRegister` title, and `last_hash` — the raw 32-byte content hash of the
/// most-recently frozen document (empty = none). The hash is kept as raw bytes
/// (not a string) so it never needs an encode/decode codec, and is fetched back
/// via `get_last_doc()` so no hash has to round-trip through a workflow variable.
#[app::state]
pub struct ScenarioFrozenStorageV1 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
    last_hash: LwwRegister<Vec<u8>>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
}

/// Convert the stored hash bytes back to the `[u8; 32]` content-hash key, or
/// `None` if no document has been frozen yet (or the stored length is wrong).
fn stored_hash(bytes: &[u8]) -> Option<[u8; 32]> {
    <[u8; 32]>::try_from(bytes).ok()
}

#[app::logic]
impl ScenarioFrozenStorageV1 {
    #[app::init]
    pub fn init() -> ScenarioFrozenStorageV1 {
        ScenarioFrozenStorageV1 {
            documents: FrozenStorage::new_with_field_name("documents"),
            title: LwwRegister::new("untitled".to_owned()),
            last_hash: LwwRegister::new(Vec::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Freeze a document; records its content hash internally (raw bytes).
    pub fn freeze_doc(&mut self, value: String) -> app::Result<()> {
        let hash = self.documents.insert(value)?;
        self.last_hash.set(hash.to_vec());
        Ok(())
    }

    /// Fetch the most-recently frozen document by its stored content hash. No
    /// hash needs to be passed in (or captured by a workflow) — the app holds it.
    pub fn get_last_doc(&self) -> app::Result<Option<String>> {
        match stored_hash(self.last_hash.get()) {
            Some(hash) => Ok(self.documents.get(&hash)?),
            None => Ok(None),
        }
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
        })
    }
}
