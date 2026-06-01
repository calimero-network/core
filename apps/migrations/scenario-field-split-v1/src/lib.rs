use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::LwwRegister;

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
pub struct ScenarioFieldSplitV1 {
    name: LwwRegister<String>,
    address: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub name: String,
    pub address: String,
}

#[app::logic]
impl ScenarioFieldSplitV1 {
    #[app::init]
    pub fn init() -> ScenarioFieldSplitV1 {
        ScenarioFieldSplitV1 {
            name: LwwRegister::new("customer".to_owned()),
            address: LwwRegister::new(String::new()),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn set_address(&mut self, address: String) -> app::Result<()> {
        self.address.set(address);
        Ok(())
    }

    pub fn get_address(&self) -> app::Result<String> {
        Ok(self.address.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            name: self.name.get().clone(),
            address: self.address.get().clone(),
        })
    }
}
