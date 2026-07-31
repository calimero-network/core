use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[cfg(not(feature = "schema_v2"))]
#[app::state]
pub struct SvcA {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[cfg(not(feature = "schema_v2"))]
#[app::logic]
impl SvcA {
    #[app::init]
    pub fn init() -> SvcA {
        SvcA {
            items: UnorderedMap::new(),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }
}

/// Alternate schema behind `schema_v2`, mirroring how a real app expresses a
/// migration target: a distinct state root plus a method only this version has.
#[cfg(feature = "schema_v2")]
#[app::state]
pub struct SvcAV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    revision: LwwRegister<u64>,
}

#[cfg(feature = "schema_v2")]
#[app::logic]
impl SvcAV2 {
    #[app::init]
    pub fn init() -> SvcAV2 {
        SvcAV2 {
            items: UnorderedMap::new(),
            revision: LwwRegister::new(0),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn revision(&self) -> app::Result<u64> {
        Ok(*self.revision.get())
    }
}
