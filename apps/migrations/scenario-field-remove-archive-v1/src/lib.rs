use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::LwwRegister;

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFieldRemoveArchiveV1 {
    name: LwwRegister<String>,
    legacy_note: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub name: String,
    pub legacy_note: String,
    pub counter: u64,
}

#[app::logic]
impl ScenarioFieldRemoveArchiveV1 {
    #[app::init]
    pub fn init() -> ScenarioFieldRemoveArchiveV1 {
        ScenarioFieldRemoveArchiveV1 {
            name: LwwRegister::new("entity".to_owned()),
            legacy_note: LwwRegister::new(String::new()),
            counter: LwwRegister::new(0),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn set_legacy_note(&mut self, note: String) -> app::Result<()> {
        self.legacy_note.set(note);
        Ok(())
    }

    pub fn get_legacy_note(&self) -> app::Result<String> {
        Ok(self.legacy_note.get().clone())
    }

    pub fn bump_counter(&mut self) -> app::Result<u64> {
        let next = *self.counter.get() + 1;
        self.counter.set(next);
        Ok(next)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            name: self.name.get().clone(),
            legacy_note: self.legacy_note.get().clone(),
            counter: *self.counter.get(),
        })
    }
}
