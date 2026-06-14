use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `AuthoredMap` through the migrate (preserving each note's
/// owner stamp byte-for-byte) and adds a plain `migration_note`. `version = 2`
/// raises the identity-gated target: each carried note is still stamped at 1,
/// so its owner's next signed write — or one tap of the generated
/// `migrate_my_entries()` — re-stamps it to 2. The generated export is emitted
/// automatically because the state has an `AuthoredMap` field.
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = ScenarioAuthoredMigrateUxV1,
    emit = Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    }
)]
pub struct ScenarioAuthoredMigrateUxV2 {
    notes: AuthoredMap<String, LwwRegister<String>>,
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
    pub note_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioAuthoredMigrateUxV1 {
    notes: AuthoredMap<String, LwwRegister<String>>,
}

#[app::logic]
impl ScenarioAuthoredMigrateUxV2 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMigrateUxV2 {
        ScenarioAuthoredMigrateUxV2 {
            notes: AuthoredMap::new(),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_note(&mut self, key: String, text: String) -> app::Result<()> {
        if self.notes.contains(&key)? {
            self.notes.update(&key, text.into())?;
        } else {
            self.notes.insert(key, text.into())?;
        }
        Ok(())
    }

    pub fn my_note(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.notes.get(&key)?.map(|v| v.get().clone()))
    }

    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.notes.owner_of(&key)?.map(|pk| pk.to_string()))
    }

    pub fn note_count(&self) -> app::Result<u64> {
        Ok(self.notes.len()? as u64)
    }

    /// The note's stored `schema_version` — `Some(1)` before convert, `Some(2)`
    /// after. Lets the e2e assert the one-tap convert actually re-stamped it.
    pub fn note_schema_version(&self, key: String) -> app::Result<Option<u32>> {
        Ok(self.notes.entry_schema_version(&key)?)
    }

    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            note_count: self.notes.len()? as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
