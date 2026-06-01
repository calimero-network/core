use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
pub struct ScenarioPureBugfixV1 {
    items: UnorderedMap<String, LwwRegister<u64>>,
    last_sum: LwwRegister<u64>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub last_sum: u64,
}

#[app::logic]
impl ScenarioPureBugfixV1 {
    #[app::init]
    pub fn init() -> ScenarioPureBugfixV1 {
        ScenarioPureBugfixV1 {
            items: UnorderedMap::new_with_field_name("items"),
            last_sum: LwwRegister::new(0),
        }
    }

    pub fn add_value(&mut self, key: String, value: u64) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_value(&self, key: &str) -> app::Result<Option<u64>> {
        Ok(self.items.get(key)?.map(|v| *v.get()))
    }

    pub fn sum_all_values(&mut self) -> app::Result<u64> {
        let mut total: u64 = 0;
        for (_k, v) in self.items.entries()? {
            total += *v.get();
        }
        // BUG: off-by-one — accidentally adds 1 to the total.
        total += 1;
        self.last_sum.set(total);
        Ok(total)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            last_sum: *self.last_sum.get(),
        })
    }
}
