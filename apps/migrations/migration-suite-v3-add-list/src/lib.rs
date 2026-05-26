use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap, Vector};

const SCHEMA_VERSION_V2: &str = "2.0.0";
const SCHEMA_VERSION_V3: &str = "3.0.0";

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MigrationSuiteV3AddList {
    items: UnorderedMap<String, LwwRegister<String>>,
    description: LwwRegister<String>,
    counter: LwwRegister<u64>,
    notes: LwwRegister<String>,
    list: Vector<LwwRegister<String>>,
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
    pub list_len: u64,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MigrationSuiteV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    description: LwwRegister<String>,
    counter: LwwRegister<u64>,
    notes: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v2_to_v3() -> MigrationSuiteV3AddList {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V2 context first.");
    });

    let old_state: MigrationSuiteV2 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V2 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V2,
        to_version: SCHEMA_VERSION_V3,
    });

    MigrationSuiteV3AddList {
        items: old_state.items,
        description: old_state.description,
        counter: old_state.counter,
        notes: old_state.notes,
        list: Vector::new_with_field_name("list"),
    }
}

#[app::logic]
impl MigrationSuiteV3AddList {
    #[app::init]
    pub fn init() -> MigrationSuiteV3AddList {
        MigrationSuiteV3AddList {
            items: UnorderedMap::new_with_field_name("items"),
            description: LwwRegister::new("initial".to_owned()),
            counter: LwwRegister::new(0),
            notes: LwwRegister::new("added in v2".to_owned()),
            list: Vector::new_with_field_name("list"),
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

    pub fn add_to_list(&mut self, item: String) -> app::Result<()> {
        self.list.push(LwwRegister::new(item))?;
        Ok(())
    }

    pub fn get_list(&self) -> app::Result<Vec<String>> {
        Ok(self.list.iter()?.map(|entry| entry.get().clone()).collect())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V3.to_owned(),
            description: self.description.get().clone(),
            counter: self.counter.get().to_string(),
            notes: self.notes.get().clone(),
            list_len: self.list.len()? as u64,
        })
    }
}
