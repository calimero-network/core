use std::collections::BTreeSet;

use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::env;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{LwwRegister, SharedStorage};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `SharedStorage` THROUGH the migrate (preserving both the
/// stored value and the writer set) and adds a plain `migration_note` register
/// seeded during migrate. The writer set is not rebuilt: rebuilding during
/// migrate would seed each node's OWN executor id (migrate runs independently
/// per node under LazyOnAccess), diverging the writer sets. Carrying the
/// wrapper preserves the v1 writer set byte-for-byte, so every node converges.
#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioSharedStorageV2 {
    doc: SharedStorage<LwwRegister<String>>,
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
    pub writer_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioSharedStorageV1 {
    doc: SharedStorage<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioSharedStorageV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioSharedStorageV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // Carry `doc` over unchanged — value and writer set preserved. Only the
    // new `migration_note` is seeded, and it is a plain `LwwRegister` (its
    // per-replica metadata is zeroed under migrate merge mode), so the seed
    // is deterministic across nodes.
    ScenarioSharedStorageV2 {
        doc: old_state.doc,
        title: old_state.title,
        migration_note: LwwRegister::new("migrated-v1-to-v2".to_owned()),
    }
}

#[app::logic]
impl ScenarioSharedStorageV2 {
    #[app::init]
    pub fn init() -> ScenarioSharedStorageV2 {
        // Seed the writer set with the creating node so it can write.
        let mut writers = BTreeSet::new();
        let executor: PublicKey = env::executor_id().into();
        writers.insert(executor);
        ScenarioSharedStorageV2 {
            doc: SharedStorage::new(writers, false),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    pub fn set_doc(&mut self, value: String) -> app::Result<()> {
        self.doc.insert(value.into())?;
        Ok(())
    }

    pub fn get_doc(&self) -> app::Result<String> {
        Ok(self.doc.get()?.get().clone())
    }

    pub fn writer_count(&self) -> app::Result<u64> {
        Ok(self.doc.writers().len() as u64)
    }

    pub fn is_frozen(&self) -> app::Result<bool> {
        Ok(self.doc.is_frozen())
    }

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            writer_count: self.doc.writers().len() as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
