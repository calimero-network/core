use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{FrozenStorage, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `FrozenStorage` THROUGH the migrate (preserving every
/// content-addressed entry) and adds a plain `migration_note` register seeded
/// during migrate. Entries are NOT re-inserted: carrying the collection
/// preserves the v1 content hashes byte-for-byte, so a document frozen under v1
/// is still retrievable under the same hash on every node after migration.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFrozenStorageV2 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
    migration_note: LwwRegister<String>,
}

#[app::event]
pub enum Event<'a> {
    Migrated {
        from_version: &'a str,
        to_version: &'a str,
    },
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioFrozenStorageV1 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
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

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioFrozenStorageV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioFrozenStorageV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // Carry `documents` over unchanged — content hashes preserved. Only the new
    // `migration_note` is seeded, and it is a plain `LwwRegister` (its
    // per-replica metadata is zeroed under migrate merge mode), so the seed is
    // deterministic across nodes.
    ScenarioFrozenStorageV2 {
        documents: old_state.documents,
        title: old_state.title,
        migration_note: LwwRegister::new("migrated-v1-to-v2".to_owned()),
    }
}

#[app::logic]
impl ScenarioFrozenStorageV2 {
    #[app::init]
    pub fn init() -> ScenarioFrozenStorageV2 {
        ScenarioFrozenStorageV2 {
            documents: FrozenStorage::new_with_field_name("documents"),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
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

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            migration_note: self.migration_note.get().clone(),
        })
    }
}
