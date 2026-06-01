use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V2: &str = "2.0.0";

#[app::state]
pub struct ScenarioNewMethodV2 {
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
impl ScenarioNewMethodV2 {
    #[app::init]
    pub fn init() -> ScenarioNewMethodV2 {
        ScenarioNewMethodV2 {
            items: UnorderedMap::new(),
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
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            items_len: self.items.len()? as u64,
        })
    }

    pub fn clear_items(&mut self) -> app::Result<u64> {
        let keys: Vec<String> = self.items.entries()?.map(|(k, _)| k).collect();
        let count = keys.len() as u64;
        for key in keys {
            self.items.remove(&key)?;
        }
        Ok(count)
    }
}
