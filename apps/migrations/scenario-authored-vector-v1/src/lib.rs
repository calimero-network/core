use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredVector, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state for the authored-vector migration scenario. `entries` is an
/// `AuthoredVector` whose every element records the executor that pushed it.
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug)]
pub struct ScenarioAuthoredVectorV1 {
    entries: AuthoredVector<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::event]
pub enum Event<'a> {
    Pushed { value: &'a str },
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub entry_count: u64,
}

#[app::logic]
impl ScenarioAuthoredVectorV1 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredVectorV1 {
        ScenarioAuthoredVectorV1 {
            entries: AuthoredVector::new_with_field_name("entries"),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Push an entry, stamping the calling executor as its owner.
    pub fn push_entry(&mut self, value: String) -> app::Result<u64> {
        let idx = self.entries.push(value.clone().into())?;
        app::emit!(Event::Pushed { value: &value });
        Ok(idx as u64)
    }

    pub fn get_entry(&self, index: u64) -> app::Result<Option<String>> {
        Ok(self.entries.get(index as usize)?.map(|v| v.get().clone()))
    }

    /// The owner (pushing executor) of the entry at `index`, as a base58 string.
    pub fn owner_of(&self, index: u64) -> app::Result<Option<String>> {
        Ok(self
            .entries
            .owner_of(index as usize)?
            .map(|pk| pk.to_string()))
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
