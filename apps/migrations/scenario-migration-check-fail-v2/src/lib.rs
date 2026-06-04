use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::migration_check::entity_count_parity;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 state for the `migration_check` FAIL scenario (PR-6d task 6d.6).
///
/// The migrate is deliberately **lossy**: it drops one item while rebuilding the
/// map. The `#[app::migration_check]` predicate uses the built-in
/// [`entity_count_parity`] helper, which sees the count drop and returns
/// `false` — so the runtime **logically aborts** the migration: the produced v2
/// root is discarded, the v1 root is never mutated, and the context keeps
/// serving v1 state.
#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioMigrationCheckFailV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
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
    pub title: String,
    pub item_count: u64,
    pub notes: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioMigrationCheckFailV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioMigrationCheckFailV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioMigrationCheckFailV1 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // DELIBERATELY LOSSY: rebuild the map, but skip the lexicographically
    // smallest key so exactly one item is dropped. The migration_check below
    // catches the count drop and the runtime logically aborts — this is the
    // headline lossy-migrate rejection the scenario asserts.
    let mut entries: Vec<(String, String)> = old_state
        .items
        .entries()
        .unwrap_or_else(|e| panic!("Migration failed: V1 items iteration error {:?}", e))
        .map(|(k, v)| (k, v.get().clone()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut items: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
    for (k, v) in entries.into_iter().skip(1) {
        items
            .insert(k, v.into())
            .unwrap_or_else(|e| panic!("Migration failed: V2 items rebuild error {:?}", e));
    }

    ScenarioMigrationCheckFailV2 {
        items,
        title: old_state.title,
        notes: LwwRegister::new("added in v2".to_owned()),
    }
}

/// Pre-commit health check over the produced v2 root.
///
/// `old` is the still-committed v1 root (read via `read_raw`); `new` is the
/// produced-but-uncommitted (lossy) v2 root. The dropped item makes the counts
/// diverge, so [`entity_count_parity`] returns `false` and the runtime
/// logically aborts the migration.
#[app::migration_check]
pub fn check(old: ScenarioMigrationCheckFailV1, new: ScenarioMigrationCheckFailV2) -> bool {
    let old_keys: Vec<String> = old
        .items
        .entries()
        .expect("read old items")
        .map(|(k, _v)| k)
        .collect();
    let new_keys: Vec<String> = new
        .items
        .entries()
        .expect("read new items")
        .map(|(k, _v)| k)
        .collect();
    entity_count_parity(&old_keys, &new_keys, 0)
}

#[app::logic]
impl ScenarioMigrationCheckFailV2 {
    #[app::init]
    pub fn init() -> ScenarioMigrationCheckFailV2 {
        ScenarioMigrationCheckFailV2 {
            items: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
            notes: LwwRegister::new("added in v2".to_owned()),
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

    pub fn item_count(&self) -> app::Result<u64> {
        Ok(self.items.len()? as u64)
    }

    pub fn get_notes(&self) -> app::Result<String> {
        Ok(self.notes.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            item_count: self.items.len()? as u64,
            notes: self.notes.get().clone(),
        })
    }
}
