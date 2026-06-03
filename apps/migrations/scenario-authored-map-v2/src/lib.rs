use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `AuthoredMap` THROUGH the migrate (preserving every
/// entry's recorded owner) and adds a plain `migration_note` register seeded
/// during migrate. Authorship is not re-stamped: re-inserting during migrate
/// would stamp each node's OWN executor id (migrate runs independently per
/// node under LazyOnAccess), diverging the owners. Carrying the collection
/// preserves the v1 owner stamps byte-for-byte, so every node converges.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = ScenarioAuthoredMapV1,
    method = migrate_v1_to_v2,
    emit = Event::Migrated { from_version: SCHEMA_VERSION_V1, to_version: SCHEMA_VERSION_V2 }
)]
pub struct ScenarioAuthoredMapV2 {
    entries: AuthoredMap<String, LwwRegister<String>>,
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
    pub entry_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioAuthoredMapV1 {
    entries: AuthoredMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::logic]
impl ScenarioAuthoredMapV2 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMapV2 {
        ScenarioAuthoredMapV2 {
            entries: AuthoredMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    pub fn put_entry(&mut self, key: String, value: String) -> app::Result<()> {
        self.entries.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_entry(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?.map(|v| v.get().clone()))
    }

    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.owner_of(&key)?.map(|pk| pk.to_string()))
    }

    pub fn entry_count(&self) -> app::Result<u64> {
        Ok(self.entries.len()? as u64)
    }

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            entry_count: self.entries.len()? as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
