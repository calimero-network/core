use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ScenarioFieldRemoveArchiveV2 {
    name: LwwRegister<String>,
    counter: LwwRegister<u64>,
    archived_legacy: UnorderedMap<String, LwwRegister<String>>,
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
    pub name: String,
    pub counter: u64,
    pub archived_latest: Option<String>,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioFieldRemoveArchiveV1 {
    name: LwwRegister<String>,
    legacy_note: LwwRegister<String>,
    counter: LwwRegister<u64>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioFieldRemoveArchiveV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioFieldRemoveArchiveV1 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    let mut archived_legacy: UnorderedMap<String, LwwRegister<String>> =
        UnorderedMap::new_with_field_name("archived_legacy");
    let legacy_value = old_state.legacy_note.get().clone();
    archived_legacy
        .insert("latest".to_owned(), LwwRegister::new(legacy_value))
        .unwrap_or_else(|e| {
            panic!("Migration failed: archive insert error {:?}", e);
        });

    ScenarioFieldRemoveArchiveV2 {
        name: old_state.name,
        counter: old_state.counter,
        archived_legacy,
    }
}

#[app::logic]
impl ScenarioFieldRemoveArchiveV2 {
    #[app::init]
    pub fn init() -> ScenarioFieldRemoveArchiveV2 {
        ScenarioFieldRemoveArchiveV2 {
            name: LwwRegister::new("entity".to_owned()),
            counter: LwwRegister::new(0),
            archived_legacy: UnorderedMap::new_with_field_name("archived_legacy"),
        }
    }

    pub fn set_name(&mut self, name: String) -> app::Result<()> {
        self.name.set(name);
        Ok(())
    }

    pub fn bump_counter(&mut self) -> app::Result<u64> {
        let next = *self.counter.get() + 1;
        self.counter.set(next);
        Ok(next)
    }

    pub fn get_archived(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.archived_legacy.get(&key)?.map(|r| r.get().clone()))
    }

    pub fn archived_keys(&self) -> app::Result<Vec<String>> {
        let keys: Vec<String> = self.archived_legacy.entries()?.map(|(k, _)| k).collect();
        Ok(keys)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        let archived_latest = self.archived_legacy.get("latest")?.map(|r| r.get().clone());
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            name: self.name.get().clone(),
            counter: *self.counter.get(),
            archived_latest,
        })
    }
}
