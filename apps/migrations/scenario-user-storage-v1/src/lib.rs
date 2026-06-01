use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UserStorage};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: a `UserStorage` (each user owns their own slot, keyed by
/// `env::executor_id`) holding per-user notes, plus a plain `LwwRegister`
/// title. The migrate scenario carries the `UserStorage` through to v2
/// unchanged — the cross-node assertion is that each user's note survives
/// the migration identically on every node (the per-user slot key is part of
/// the stored map, so it must round-trip).
#[app::state]
#[derive(Debug)]
pub struct ScenarioUserStorageV1 {
    notes: UserStorage<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub note_count: u64,
}

#[app::logic]
impl ScenarioUserStorageV1 {
    #[app::init]
    pub fn init() -> ScenarioUserStorageV1 {
        ScenarioUserStorageV1 {
            notes: UserStorage::new_with_field_name("notes"),
            title: LwwRegister::new("untitled".to_owned()),
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

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
            note_count: self.note_count()?,
        })
    }
}
