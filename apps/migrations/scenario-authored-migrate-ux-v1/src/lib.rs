use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: an `AuthoredMap` of owner-keyed notes. `version = 1` declares the
/// schema target identity-gated writes stamp, so after a v1→v2 migrate each
/// note carries `schema_version = 1` and is below the v2 target until its owner
/// re-signs it (organically or via the one-tap `migrate_my_entries()`).
#[app::state(version = 1)]
pub struct ScenarioAuthoredMigrateUxV1 {
    notes: AuthoredMap<String, LwwRegister<String>>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub note_count: u64,
}

#[app::logic]
impl ScenarioAuthoredMigrateUxV1 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMigrateUxV1 {
        ScenarioAuthoredMigrateUxV1 {
            notes: AuthoredMap::new(),
        }
    }

    /// Write a note under `key`, owned by the caller. Re-writing an existing
    /// key requires the caller to be its owner.
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

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            note_count: self.notes.len()? as u64,
        })
    }
}
