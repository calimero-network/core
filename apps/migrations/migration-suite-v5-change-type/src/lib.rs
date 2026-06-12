use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V4: &str = "4.0.0";
const SCHEMA_VERSION_V5: &str = "5.0.0";

#[app::state(version = 5, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = MigrationSuiteV4,
    emit = Event::Migrated {
        from_version: SCHEMA_VERSION_V4,
        to_version: SCHEMA_VERSION_V5,
    }
)]
pub struct MigrationSuiteV5ChangeType {
    items: UnorderedMap<String, LwwRegister<String>>,
    details: LwwRegister<String>,
    #[migrate(from = counter, with = counter_u64_to_string)]
    counter: LwwRegister<String>,
}

fn counter_u64_to_string(c: LwwRegister<u64>) -> LwwRegister<String> {
    LwwRegister::new(c.get().to_string())
}

#[app::event]
pub enum Event<'a> {
    Migrated {
        from_version: &'a str,
        to_version: &'a str,
    },
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub details: String,
    pub counter: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MigrationSuiteV4 {
    items: UnorderedMap<String, LwwRegister<String>>,
    details: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[app::logic]
impl MigrationSuiteV5ChangeType {
    #[app::init]
    pub fn init() -> MigrationSuiteV5ChangeType {
        MigrationSuiteV5ChangeType {
            items: UnorderedMap::new(),
            details: LwwRegister::new("initial".to_owned()),
            counter: LwwRegister::new("0".to_owned()),
        }
    }

    pub fn set_item(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_item(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn set_details(&mut self, details: String) -> app::Result<()> {
        self.details.set(details);
        Ok(())
    }

    pub fn get_details(&self) -> app::Result<String> {
        Ok(self.details.get().clone())
    }

    pub fn set_counter(&mut self, counter: String) -> app::Result<()> {
        self.counter.set(counter);
        Ok(())
    }

    pub fn get_counter(&self) -> app::Result<String> {
        Ok(self.counter.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V5.to_owned(),
            details: self.details.get().clone(),
            counter: self.counter.get().clone(),
        })
    }
}
