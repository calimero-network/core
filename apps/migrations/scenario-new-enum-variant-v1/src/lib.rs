use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub enum Status {
    Active,
    Paused,
}

#[app::state]
pub struct ScenarioNewEnumVariantV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    status: LwwRegister<Status>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub status: String,
}

#[app::logic]
impl ScenarioNewEnumVariantV1 {
    #[app::init]
    pub fn init() -> ScenarioNewEnumVariantV1 {
        ScenarioNewEnumVariantV1 {
            items: UnorderedMap::new_with_field_name("items"),
            status: LwwRegister::new(Status::Active),
        }
    }

    pub fn set_status_active(&mut self) -> app::Result<()> {
        self.status.set(Status::Active);
        Ok(())
    }

    pub fn set_status_paused(&mut self) -> app::Result<()> {
        self.status.set(Status::Paused);
        Ok(())
    }

    pub fn set_item(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_item(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            status: format!("{:?}", self.status.get()),
        })
    }
}
