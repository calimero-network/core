use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V3: &str = "3.0.0";
const SCHEMA_VERSION_V4: &str = "4.0.0";

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MigrationSuiteV4RenameField {
    items: UnorderedMap<String, LwwRegister<String>>,
    details: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[app::event]
pub enum Event<'a> {
    Migrated { from_version: &'a str, to_version: &'a str },
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
struct MigrationSuiteV3 {
    items: UnorderedMap<String, LwwRegister<String>>,
    description: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[app::migrate]
pub fn migrate_v3_to_v4() -> MigrationSuiteV4RenameField {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V3 context first.");
    });

    let old_state: MigrationSuiteV3 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!("Migration failed: V3 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V3,
        to_version: SCHEMA_VERSION_V4,
    });

    MigrationSuiteV4RenameField {
        items: old_state.items,
        details: old_state.description,
        counter: old_state.counter,
    }
}

#[app::logic]
impl MigrationSuiteV4RenameField {
    #[app::init]
    pub fn init() -> MigrationSuiteV4RenameField {
        MigrationSuiteV4RenameField {
            items: UnorderedMap::new_with_field_name("items"),
            details: LwwRegister::new("initial".to_owned()),
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

    pub fn set_details(&mut self, details: String) -> app::Result<()> {
        self.details.set(details);
        Ok(())
    }

    pub fn get_details(&self) -> app::Result<String> {
        Ok(self.details.get().clone())
    }

    pub fn increment_counter(&mut self) -> app::Result<u64> {
        let next = *self.counter.get() + 1;
        self.counter.set(next);
        Ok(next)
    }

    pub fn get_counter(&self) -> app::Result<u64> {
        Ok(*self.counter.get())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V4.to_owned(),
            details: self.details.get().clone(),
            counter: self.counter.get().to_string(),
        })
    }
}
