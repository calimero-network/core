use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{FrozenStorage, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `FrozenStorage` THROUGH the migrate (preserving every
/// content-addressed entry) AND performs a real content-rewrite during the
/// migrate: it freezes a NEW document derived from the carried `title`. This is
/// the convergent case of build-during-migrate for an identity-bearing-looking
/// type: `FrozenStorage` is content-addressed (`id = hash(value)`), so every
/// node deriving the SAME content from the SAME carried state freezes it under
/// an IDENTICAL hash — the new entry converges without any owner/identity
/// stamp. `migration_doc_hash` records that hash (a plain `LwwRegister`, its
/// per-replica metadata zeroed under migrate merge mode, and the hash string is
/// itself deterministic) so workflows can assert the new entry is byte-identical
/// across nodes.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFrozenStorageV2 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
    migration_note: LwwRegister<String>,
    /// Content hash (base58) of the document frozen DURING migrate. Identical
    /// on every node because the derived content is a pure function of carried
    /// state.
    migration_doc_hash: LwwRegister<String>,
}

/// The document content frozen during migrate, derived deterministically from
/// the carried `title` so every node produces byte-identical content (hence an
/// identical content hash).
fn derived_doc_content(title: &str) -> String {
    format!("v2-derived::{title}")
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
    pub migration_doc_hash: String,
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

    // Carry `documents` over unchanged — v1 content hashes preserved — and ALSO
    // freeze a new document derived from the carried `title` during the migrate.
    // Because `FrozenStorage` is content-addressed, every node freezing the same
    // derived content lands the same hash, so the new entry converges with no
    // identity stamp. This is the real content-rewrite-during-migrate case.
    let mut documents = old_state.documents;
    let derived = derived_doc_content(old_state.title.get());
    let new_hash = documents
        .insert(derived)
        .unwrap_or_else(|e| panic!("Migration failed: derived-doc freeze error {:?}", e));

    ScenarioFrozenStorageV2 {
        documents,
        title: old_state.title,
        migration_note: LwwRegister::new("migrated-v1-to-v2".to_owned()),
        migration_doc_hash: LwwRegister::new(hash_to_string(new_hash)),
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
            migration_doc_hash: LwwRegister::new(String::new()),
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

    /// Base58 content hash of the document frozen DURING migrate. Identical on
    /// every node (derived content is a pure function of carried state).
    pub fn migration_doc_hash(&self) -> app::Result<String> {
        Ok(self.migration_doc_hash.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            migration_note: self.migration_note.get().clone(),
            migration_doc_hash: self.migration_doc_hash.get().clone(),
        })
    }
}
