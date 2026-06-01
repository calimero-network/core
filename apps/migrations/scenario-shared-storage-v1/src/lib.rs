use std::collections::BTreeSet;

use calimero_sdk::app;
use calimero_sdk::env;
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{LwwRegister, SharedStorage};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state: a `SharedStorage` holding a single LWW value, gated by a writer
/// set, plus a plain `LwwRegister` title. The migrate scenario carries the
/// `SharedStorage` through to v2 unchanged — the cross-node assertion is that
/// both the stored value and the writer set survive the migration identically
/// on every node.
#[app::state]
pub struct ScenarioSharedStorageV1 {
    doc: SharedStorage<LwwRegister<String>>,
    title: LwwRegister<String>,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub writer_count: u64,
}

#[app::logic]
impl ScenarioSharedStorageV1 {
    #[app::init]
    pub fn init() -> ScenarioSharedStorageV1 {
        // Seed the writer set with the creating node so it can write.
        let mut writers = BTreeSet::new();
        let executor: PublicKey = env::executor_id().into();
        writers.insert(executor);
        ScenarioSharedStorageV1 {
            doc: SharedStorage::new(writers, false),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    /// Replace the shared value. Only a writer may write.
    pub fn set_doc(&mut self, value: String) -> app::Result<()> {
        self.doc.insert(value.into())?;
        Ok(())
    }

    pub fn get_doc(&self) -> app::Result<String> {
        Ok(self.doc.get()?.get().clone())
    }

    pub fn writer_count(&self) -> app::Result<u64> {
        Ok(self.doc.writers().len() as u64)
    }

    pub fn is_frozen(&self) -> app::Result<bool> {
        Ok(self.doc.is_frozen())
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
            writer_count: self.doc.writers().len() as u64,
        })
    }
}
