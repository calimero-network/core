use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap, Vector};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

#[app::state(version = 2, emits = for<'a> Event<'a>)]
pub struct ScenarioCrdtNativeV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
    tags: Vector<LwwRegister<String>>,
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
    pub tag_count: u64,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioCrdtNativeV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioCrdtNativeV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioCrdtNativeV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {e:?}");
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // Seed `tags` DURING the migration by denormalising the v1 item keys
    // into an ordered list. This is the cross-node-determinism stress
    // case for a `Vector` POPULATED inside a migrate: every node runs
    // migrate independently (LazyOnAccess emits no sync delta), so the
    // seeded element ids must be a pure function of position — not
    // `Id::random()` as `push` uses on the live path — or the two
    // replicas' `tags` diverge and double up on the next sync. The SDK
    // guarantees this: `#[app::migrate]` runs under storage merge mode
    // (zeroes the per-element `LwwRegister` node_id/timestamp) and
    // `__assign_deterministic_ids()` re-keys each Vector element by its
    // append index. Keys are sorted so the seed order is canonical
    // regardless of the v1 map's internal iteration order.
    let mut keys: Vec<String> = old_state
        .items
        .entries()
        .unwrap_or_else(|e| panic!("Migration failed: V1 items iteration error {e:?}"))
        .map(|(k, _v)| k)
        .collect();
    keys.sort();

    let mut tags: Vector<LwwRegister<String>> = Vector::new();
    for k in keys {
        tags.push(k.into())
            .unwrap_or_else(|e| panic!("Migration failed: V2 tags seed error {e:?}"));
    }

    ScenarioCrdtNativeV2 {
        items: old_state.items,
        title: old_state.title,
        tags,
    }
}

#[app::logic]
impl ScenarioCrdtNativeV2 {
    #[app::init]
    pub fn init() -> ScenarioCrdtNativeV2 {
        ScenarioCrdtNativeV2 {
            items: UnorderedMap::new(),
            title: LwwRegister::new("untitled".to_owned()),
            tags: Vector::new(),
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

    pub fn push_tag(&mut self, tag: String) -> app::Result<()> {
        self.tags.push(tag.into())?;
        Ok(())
    }

    pub fn get_tag(&self, index: usize) -> app::Result<Option<String>> {
        Ok(self.tags.get(index)?.map(|t| t.get().clone()))
    }

    pub fn tag_count(&self) -> app::Result<u64> {
        Ok(self.tags.len()? as u64)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            tag_count: self.tags.len()? as u64,
        })
    }
}
