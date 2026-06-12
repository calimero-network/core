use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UserStorage};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `UserStorage` THROUGH the migrate (preserving every
/// user's per-slot note) and adds a plain `migration_note` register seeded
/// during migrate. The per-user notes are NOT re-inserted: `insert` writes
/// the CURRENT executor's own slot (keyed by `env::executor_id`), and migrate
/// runs independently per node under LazyOnAccess, so re-inserting would key
/// every user's data under each node's OWN id, diverging the map. Carrying
/// the collection preserves the v1 slot keys byte-for-byte, so every node
/// converges.
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = ScenarioUserStorageV1,
    emit = Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    }
)]
pub struct ScenarioUserStorageV2 {
    notes: UserStorage<LwwRegister<String>>,
    title: LwwRegister<String>,
    #[migrate(new = LwwRegister::new("migrated-v1-to-v2".to_owned()))]
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
    pub note_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioUserStorageV1 {
    notes: UserStorage<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::logic]
impl ScenarioUserStorageV2 {
    #[app::init]
    pub fn init() -> ScenarioUserStorageV2 {
        ScenarioUserStorageV2 {
            notes: UserStorage::new(),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Write the calling executor's own note slot.
    pub fn set_note(&mut self, value: String) -> app::Result<()> {
        self.notes.insert(value.into())?;
        Ok(())
    }

    /// The calling executor's own note, if present.
    pub fn my_note(&self) -> app::Result<Option<String>> {
        Ok(self.notes.get()?.map(|v| v.get().clone()))
    }

    pub fn note_count(&self) -> app::Result<u64> {
        Ok(self.notes.entries()?.count() as u64)
    }

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            note_count: self.note_count()?,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
