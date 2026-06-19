use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 state for the `migration_check` FAIL scenario (PR-6d).
///
/// The migrate is deliberately **lossy**: it drops one item. It emits a
/// transient [`MigrationWitness`] carrying the v1 item count (captured before
/// the drop). The `#[app::migration_check]` predicate compares that baseline
/// against the **produced** v2 item count (read through the staging buffer);
/// the mismatch makes it return `false`, so the runtime **logically aborts** —
/// the staged child writes are dropped, the v1 root is never mutated, and the
/// context keeps serving v1 state with **zero residue**.
#[app::state(version = 2, emits = for<'a> Event<'a>)]
pub struct ScenarioMigrationCheckFailV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
    notes: LwwRegister<String>,
}

/// Transient migration witness — emitted by the migrate, read by the check,
/// and NEVER persisted to v2 state (rides out on the runtime Outcome).
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct MigrationWitness {
    /// The v1 item count, captured BEFORE the deliberately lossy drop.
    pub v1_count: u64,
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
pub struct ScenarioMigrationCheckFailV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> (ScenarioMigrationCheckFailV2, MigrationWitness) {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioMigrationCheckFailV1 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {e:?}");
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // DELIBERATELY LOSSY: carry the map, then REMOVE the lexicographically
    // smallest key so exactly one item is dropped. We capture the v1 count in a
    // transient witness BEFORE the drop; the migration_check compares it to the
    // produced v2 count and the runtime logically aborts the lossy migrate.
    //
    // NOTE: we remove from the CARRIED map rather than rebuilding a fresh
    // same-named `UnorderedMap`. A fresh map assigned to the `items` field is
    // re-keyed to that field's deterministic id during migrate, so it shares
    // the carried v1 storage and *unions* with it — the "skipped" key survives
    // and nothing is dropped. Removing from the carried collection is the
    // deterministic way to actually drop an entry (sort first for convergence).
    let mut items = old_state.items;
    let mut keys: Vec<String> = items
        .entries()
        .unwrap_or_else(|e| panic!("Migration failed: V1 items iteration error {e:?}"))
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    let v1_count = keys.len() as u64;
    if let Some(smallest) = keys.first() {
        items
            .remove(smallest)
            .unwrap_or_else(|e| panic!("Migration failed: V2 items drop error {e:?}"));
    }

    (
        ScenarioMigrationCheckFailV2 {
            items,
            title: old_state.title,
            notes: LwwRegister::new("added in v2".to_owned()),
        },
        MigrationWitness { v1_count },
    )
}

/// Pre-commit health check over the produced v2 root.
///
/// `new.items` reads the **produced** v2 collection through the staging buffer
/// (= v1_count − 1 after the lossy drop); `witness.v1_count` is the v1 count the
/// migrate captured before dropping. They differ, so the check returns `false`
/// and the runtime logically aborts — dropping the staged writes (zero residue)
/// and leaving the v1 root intact. (`_old` is the still-committed v1 root; its
/// lazy collections would read the staged buffer, so we rely on the witness
/// baseline instead of an `old`-vs-`new` collection diff.)
#[app::migration_check]
pub fn check(
    _old: ScenarioMigrationCheckFailV1,
    new: ScenarioMigrationCheckFailV2,
    witness: MigrationWitness,
) -> bool {
    matches!(new.items.len(), Ok(n) if n as u64 == witness.v1_count)
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
