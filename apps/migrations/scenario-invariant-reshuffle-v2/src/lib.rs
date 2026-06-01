use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

// v2 normalizes the v1 denormalized pair (`global_count`,
// `per_item_counts`) by funneling all writes through a single method
// (`record`) that bumps both fields atomically. v1 had the implicit
// invariant `global_count == sum(per_item_counts)`, but exposed two
// independent setters; v2 keeps the same field shape but eliminates
// the racy multi-step mutation API. `record` is the only public
// mutator that touches the counter pair, so the invariant holds by
// construction. The migrate fn ALSO re-derives `total` from the v1
// per-item map rather than trusting `old.global_count` — so even a
// state that violated the v1 invariant pre-migration is healed on
// the way over.
//
// Inline fields rather than a nested `Stats` substruct: `#[app::state]`
// requires every top-level field to be `Mergeable`, which collection
// types satisfy but a plain struct of collections does not.
#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioInvariantReshuffleV2 {
    total: LwwRegister<u64>,
    per_item: UnorderedMap<String, LwwRegister<u64>>,
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
    pub total: u64,
    pub item_keys: Vec<String>,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioInvariantReshuffleV1 {
    // Read via borsh during migrate, then intentionally discarded — the
    // whole point is re-deriving `total` from `per_item_counts` so a
    // possibly-stale denormalized counter doesn't survive the migration.
    #[allow(dead_code)]
    global_count: LwwRegister<u64>,
    per_item_counts: UnorderedMap<String, LwwRegister<u64>>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioInvariantReshuffleV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioInvariantReshuffleV1 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // Re-derive `total` from the per-item values rather than trusting
    // v1's redundant `global_count`. This is the whole point of the
    // normalization: the invariant is reconstructed from the source
    // of truth, not preserved from a possibly-stale denormalized
    // field.
    //
    // Cross-node determinism is handled at the SDK layer, not here:
    // `per_item` entries get key-derived ids (`compute_id(parent,
    // key)`), so insertion order doesn't affect them; and the
    // `#[app::migrate]` macro now calls `__assign_deterministic_ids()`
    // on the returned state so the `total` LwwRegister (and any field
    // materialised via `.into()`) gets a deterministic field-name id
    // instead of a random one. Both nodes therefore land on identical
    // v2 roots.
    let mut total: u64 = 0;
    let mut per_item: UnorderedMap<String, LwwRegister<u64>> =
        UnorderedMap::new_with_field_name("per_item");
    for (k, v) in old_state.per_item_counts.entries().unwrap_or_else(|e| {
        panic!(
            "Migration failed: V1 per_item_counts iteration error {:?}",
            e
        );
    }) {
        let n = *v.get();
        total += n;
        per_item.insert(k, n.into()).unwrap_or_else(|e| {
            panic!("Migration failed: V2 per_item insert error {:?}", e);
        });
    }

    ScenarioInvariantReshuffleV2 {
        total: total.into(),
        per_item,
    }
}

#[app::logic]
impl ScenarioInvariantReshuffleV2 {
    #[app::init]
    pub fn init() -> ScenarioInvariantReshuffleV2 {
        ScenarioInvariantReshuffleV2 {
            total: LwwRegister::new(0),
            per_item: UnorderedMap::new_with_field_name("per_item"),
        }
    }

    // SINGLE entry point: enforces the `total == sum(per_item)`
    // invariant by bumping both `per_item[item]` and `total` in one
    // method. Callers cannot accidentally update one without the
    // other.
    pub fn record(&mut self, item: String) -> app::Result<u64> {
        let n = self.per_item.get(&item)?.map(|r| *r.get()).unwrap_or(0) + 1;
        self.per_item.insert(item, n.into())?;
        let new_total = *self.total.get() + 1;
        self.total.set(new_total);
        Ok(new_total)
    }

    pub fn get_total(&self) -> app::Result<u64> {
        Ok(*self.total.get())
    }

    pub fn get_item_count(&self, item: &str) -> app::Result<Option<u64>> {
        Ok(self.per_item.get(item)?.map(|r| *r.get()))
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        let mut item_keys: Vec<String> = Vec::new();
        for (k, _v) in self.per_item.entries()? {
            item_keys.push(k);
        }
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            total: *self.total.get(),
            item_keys,
        })
    }
}
