use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredMap, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: an `AuthoredMap` (each entry remembers the executor that wrote
/// it) plus a plain `LwwRegister` title. The migrate scenario carries the
/// `AuthoredMap` through to v2 unchanged — the cross-node assertion is that
/// each entry's recorded owner survives the migration identically on every
/// node (authorship is part of the stored value, so it must round-trip).
#[app::state]
pub struct ScenarioAuthoredMapV1 {
    entries: AuthoredMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub entry_count: u64,
}

#[app::logic]
impl ScenarioAuthoredMapV1 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredMapV1 {
        ScenarioAuthoredMapV1 {
            entries: AuthoredMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Insert a new entry, stamping the current executor as its owner.
    pub fn put_entry(&mut self, key: String, value: String) -> app::Result<()> {
        self.entries.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_entry(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?.map(|v| v.get().clone()))
    }

    /// Hex/display string of the recorded owner of `key`, if present.
    pub fn owner_of(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.owner_of(&key)?.map(|pk| pk.to_string()))
    }

    pub fn entry_count(&self) -> app::Result<u64> {
        Ok(self.entries.len()? as u64)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
            entry_count: self.entries.len()? as u64,
        })
    }
}
