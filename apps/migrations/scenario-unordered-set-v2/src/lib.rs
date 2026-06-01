use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedSet};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 performs a real content-rewrite of the set DURING migrate: it reads the v1
/// `tags` and builds a NEW set with every tag upper-cased. This is the
/// convergent build-during-migrate case for a plain collection: `UnorderedSet`
/// element ids are `compute_id(set_id, value)` (content-derived, no executor
/// identity), so every node applying the SAME transform over the SAME old set
/// produces a byte-identical set — even though migrate emits no sync delta.
#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioUnorderedSetV2 {
    tags: UnorderedSet<String>,
    title: LwwRegister<String>,
    migration_note: LwwRegister<String>,
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
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioUnorderedSetV1 {
    tags: UnorderedSet<String>,
    title: LwwRegister<String>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> ScenarioUnorderedSetV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state. Create a V1 context first.");
    });

    let old_state: ScenarioUnorderedSetV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| {
            panic!("Migration failed: V1 deserialization error {:?}", e);
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    // Transform the set IN PLACE: carry the v1 set, then clear it and re-insert
    // every tag upper-cased. Carrying (not constructing a fresh set with the
    // same field name) matters — a `new_with_field_name("tags")` would alias the
    // same deterministic storage id as the v1 set, so its old entries would
    // survive and the upper-cased inserts would just ADD to them (doubling the
    // count). Clearing the carried set removes the originals first. Sorting the
    // source tags makes the rebuild order canonical (set ids are content-derived
    // so order does not affect the result — belt-and-braces for determinism).
    let mut tags = old_state.tags;
    let mut old_tags: Vec<String> = tags
        .iter()
        .unwrap_or_else(|e| panic!("Migration failed: V1 tags iteration error {:?}", e))
        .collect();
    old_tags.sort();

    tags.clear()
        .unwrap_or_else(|e| panic!("Migration failed: V2 tags clear error {:?}", e));
    for tag in old_tags {
        tags.insert(tag.to_uppercase())
            .unwrap_or_else(|e| panic!("Migration failed: V2 tag insert error {:?}", e));
    }

    ScenarioUnorderedSetV2 {
        tags,
        title: old_state.title,
        migration_note: LwwRegister::new("migrated-v1-to-v2".to_owned()),
    }
}

#[app::logic]
impl ScenarioUnorderedSetV2 {
    #[app::init]
    pub fn init() -> ScenarioUnorderedSetV2 {
        ScenarioUnorderedSetV2 {
            tags: UnorderedSet::new_with_field_name("tags"),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    pub fn add_tag(&mut self, tag: String) -> app::Result<bool> {
        Ok(self.tags.insert(tag)?)
    }

    pub fn has_tag(&self, tag: String) -> app::Result<bool> {
        Ok(self.tags.contains(&tag)?)
    }

    pub fn tag_count(&self) -> app::Result<u64> {
        Ok(self.tags.len()? as u64)
    }

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            tag_count: self.tags.len()? as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
