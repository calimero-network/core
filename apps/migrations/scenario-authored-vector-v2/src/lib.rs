use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{AuthoredVector, LwwRegister};

const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// v2 carries the v1 `AuthoredVector` THROUGH the migrate (preserving every
/// element's recorded owner AND its index/order) and adds a plain
/// `migration_note` register seeded during migrate. Elements are NOT re-pushed:
/// `AuthoredVector::push` stamps `env::executor_id()` as owner, and migrate runs
/// independently per node under LazyOnAccess, so re-pushing would stamp each
/// node's OWN identity and diverge the owners. Carrying the collection preserves
/// the v1 owner stamps byte-for-byte, so every node converges.
///
/// (Content-rewrite of an `AuthoredVector` during migrate — transforming
/// element values while preserving authorship — needs a migration-context gate
/// relaxation and is tracked in #2534.)
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(
    from = ScenarioAuthoredVectorV1,
    emit = Event::Migrated { from_version: SCHEMA_VERSION_V1, to_version: SCHEMA_VERSION_V2 }
)]
pub struct ScenarioAuthoredVectorV2 {
    entries: AuthoredVector<LwwRegister<String>>,
    title: LwwRegister<String>,
    #[migrate(new = LwwRegister::new("migrated-v1-to-v2".to_owned()))]
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
    pub entry_count: u64,
    pub migration_note: String,
}

#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ScenarioAuthoredVectorV1 {
    entries: AuthoredVector<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[app::logic]
impl ScenarioAuthoredVectorV2 {
    #[app::init]
    pub fn init() -> ScenarioAuthoredVectorV2 {
        ScenarioAuthoredVectorV2 {
            entries: AuthoredVector::new(),
            title: LwwRegister::new("untitled".to_owned()),
            migration_note: LwwRegister::new(String::new()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    pub fn push_entry(&mut self, value: String) -> app::Result<u64> {
        let idx = self.entries.push(value.into())?;
        Ok(idx as u64)
    }

    pub fn get_entry(&self, index: u64) -> app::Result<Option<String>> {
        Ok(self.entries.get(index as usize)?.map(|v| v.get().clone()))
    }

    pub fn owner_of(&self, index: u64) -> app::Result<Option<String>> {
        Ok(self
            .entries
            .owner_of(index as usize)?
            .map(|pk| pk.to_string()))
    }

    pub fn entry_count(&self) -> app::Result<u64> {
        Ok(self.entries.len()? as u64)
    }

    /// v2-only getter for the field seeded during migrate.
    pub fn migration_note(&self) -> app::Result<String> {
        Ok(self.migration_note.get().clone())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            title: self.title.get().clone(),
            entry_count: self.entries.len()? as u64,
            migration_note: self.migration_note.get().clone(),
        })
    }
}
