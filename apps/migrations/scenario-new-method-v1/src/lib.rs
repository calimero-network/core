use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioNewMethodV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    counter: LwwRegister<u64>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub items_len: u64,
}

#[app::logic]
impl ScenarioNewMethodV1 {
    #[app::init]
    pub fn init() -> ScenarioNewMethodV1 {
        ScenarioNewMethodV1 {
            items: UnorderedMap::new_with_field_name("items"),
            counter: LwwRegister::new(0),
        }
    }

    pub fn set_item(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_item(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn increment_counter(&mut self) -> app::Result<u64> {
        let next = *self.counter.get() + 1;
        self.counter.set(next);
        Ok(next)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            items_len: self.items.len()? as u64,
        })
    }
}
