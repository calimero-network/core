use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V2: &str = "2.0.0";

// Borsh enum encoding is by discriminant INDEX (0, 1, 2, ...).
// Appending `Archived` at the END preserves the v1 indices
// (Active=0, Paused=1) so v1-encoded state bytes deserialize
// unchanged under v2 — no migration function required.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub enum Status {
    Active,
    Paused,
    Archived,
}

#[app::state]
pub struct ScenarioNewEnumVariantV2 {
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
impl ScenarioNewEnumVariantV2 {
    #[app::init]
    pub fn init() -> ScenarioNewEnumVariantV2 {
        ScenarioNewEnumVariantV2 {
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

    pub fn set_status_archived(&mut self) -> app::Result<()> {
        self.status.set(Status::Archived);
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
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            status: format!("{:?}", self.status.get()),
        })
    }
}
