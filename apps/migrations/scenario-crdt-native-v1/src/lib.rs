use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
pub struct ScenarioCrdtNativeV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub tag_count: u64,
}

#[app::logic]
impl ScenarioCrdtNativeV1 {
    #[app::init]
    pub fn init() -> ScenarioCrdtNativeV1 {
        ScenarioCrdtNativeV1 {
            items: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
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
            title: self.title.get().clone(),
            tag_count: 0,
        })
    }
}
