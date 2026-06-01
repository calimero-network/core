use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";

#[app::state]
pub struct ScenarioInvariantReshuffleV1 {
    global_count: LwwRegister<u64>,
    per_item_counts: UnorderedMap<String, LwwRegister<u64>>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub global_count: u64,
    pub item_keys: Vec<String>,
}

#[app::logic]
impl ScenarioInvariantReshuffleV1 {
    #[app::init]
    pub fn init() -> ScenarioInvariantReshuffleV1 {
        ScenarioInvariantReshuffleV1 {
            global_count: LwwRegister::new(0),
            per_item_counts: UnorderedMap::new(),
        }
    }

    // Denormalized: caller must remember to bump BOTH the per-item counter
    // AND the top-level `global_count`. If either write is forgotten,
    // the implicit invariant `global_count == sum(per_item_counts)` breaks.
    pub fn record(&mut self, item: String) -> app::Result<()> {
        let n = self
            .per_item_counts
            .get(&item)?
            .map(|r| *r.get())
            .unwrap_or(0)
            + 1;
        self.per_item_counts.insert(item, n.into())?;
        self.global_count.set(*self.global_count.get() + 1);
        Ok(())
    }

    pub fn get_global_count(&self) -> app::Result<u64> {
        Ok(*self.global_count.get())
    }

    pub fn get_item_count(&self, item: &str) -> app::Result<Option<u64>> {
        Ok(self.per_item_counts.get(item)?.map(|r| *r.get()))
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        let mut item_keys: Vec<String> = Vec::new();
        for (k, _v) in self.per_item_counts.entries()? {
            item_keys.push(k);
        }
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            global_count: *self.global_count.get(),
            item_keys,
        })
    }
}
