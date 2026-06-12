use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

// version = 2 is what the v2 upgrade path resolves against: the node compares
// this against the running bytecode's state_version and picks the declared
// edge — no caller-supplied migrate_method (see workflows 00/01).
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = MigrationSuiteV1,
    emit = Event::Migrated { from_version: SCHEMA_VERSION_V1, to_version: SCHEMA_VERSION_V2 }
)]
pub struct MigrationSuiteV2AddField {
    items: UnorderedMap<String, LwwRegister<String>>,
    description: LwwRegister<String>,
    counter: LwwRegister<u64>,
    #[migrate(new = LwwRegister::new("added in v2".to_owned()))]
    notes: LwwRegister<String>,
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
    pub description: String,
    pub counter: String,
    pub notes: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MigrationSuiteV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    description: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[app::logic]
impl MigrationSuiteV2AddField {
    #[app::init]
    pub fn init() -> MigrationSuiteV2AddField {
        MigrationSuiteV2AddField {
            items: UnorderedMap::new(),
            description: LwwRegister::new("initial".to_owned()),
            counter: LwwRegister::new(0),
            notes: LwwRegister::new("added in v2".to_owned()),
        }
    }

    pub fn set_item(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get_item(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn set_description(&mut self, description: String) -> app::Result<()> {
        self.description.set(description);
        Ok(())
    }

    pub fn get_description(&self) -> app::Result<String> {
        Ok(self.description.get().clone())
    }

    pub fn set_notes(&mut self, notes: String) -> app::Result<()> {
        self.notes.set(notes);
        Ok(())
    }

    pub fn get_notes(&self) -> app::Result<String> {
        Ok(self.notes.get().clone())
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
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            description: self.description.get().clone(),
            counter: self.counter.get().to_string(),
            notes: self.notes.get().clone(),
        })
    }
}
