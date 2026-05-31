use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{FrozenStorage, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `FrozenStorage` THROUGH the migrate (preserving every
/// content-addressed entry, incl. the v1 doc fetchable via `get_last_doc`) AND
/// performs a real content-rewrite during the migrate: it freezes a NEW document
/// derived from the carried `title`. This is the convergent build-during-migrate
/// case for a content-addressed type — every node deriving the SAME content from
/// the SAME carried state freezes it under an IDENTICAL hash, so the new entry
/// converges with no owner/identity stamp. The new doc's hash is recorded in
/// `migration_hash` (raw bytes) and fetched back via `get_migration_doc()` — no
/// hash crosses a workflow variable and no encode/decode codec is needed.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFrozenStorageV2 {
    documents: FrozenStorage<String>,
    title: LwwRegister<String>,
    last_hash: LwwRegister<Vec<u8>>,
    migration_note: LwwRegister<String>,
    /// Raw 32-byte content hash of the document frozen DURING migrate. Identical
    /// on every node because the derived content is a pure function of carried
    /// state.
    migration_hash: LwwRegister<Vec<u8>>,
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
    last_hash: LwwRegister<Vec<u8>>,
}

fn stored_hash(bytes: &[u8]) -> Option<[u8; 32]> {
    <[u8; 32]>::try_from(bytes).ok()
}

/// The document content frozen during migrate, derived deterministically from
/// the carried `title` so every node produces byte-identical content (hence an
/// identical content hash).
fn derived_doc_content(title: &str) -> String {
    format!("v2-derived::{title}")
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
    // Because FrozenStorage is content-addressed, every node freezing the same
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
        last_hash: old_state.last_hash,
        migration_note: LwwRegister::new("migrated-v1-to-v2".to_owned()),
        migration_hash: LwwRegister::new(new_hash.to_vec()),
    }
}

#[app::logic]
impl ScenarioFrozenStorageV2 {
    #[app::init]
    pub fn init() -> ScenarioFrozenStorageV2 {
        ScenarioFrozenStorageV2 {
            documents: FrozenStorage::new_with_field_name("documents"),
            title: LwwRegister::new("untitled".to_owned()),
            last_hash: LwwRegister::new(Vec::new()),
            migration_note: LwwRegister::new(String::new()),
            migration_hash: LwwRegister::new(Vec::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Fetch the v1-frozen document (carried through migrate) by its stored hash.
    pub fn get_last_doc(&self) -> app::Result<Option<String>> {
        match stored_hash(self.last_hash.get()) {
            Some(hash) => Ok(self.documents.get(&hash)?),
            None => Ok(None),
        }
    }

    /// Fetch the document frozen DURING migrate by its stored hash. Resolves to
    /// the derived content on every node (the hash is identical cross-node).
    pub fn get_migration_doc(&self) -> app::Result<Option<String>> {
        match stored_hash(self.migration_hash.get()) {
            Some(hash) => Ok(self.documents.get(&hash)?),
            None => Ok(None),
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
