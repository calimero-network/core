use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state]
pub struct SvcB {
    items: UnorderedMap<String, LwwRegister<u64>>,
}

#[app::logic]
impl SvcB {
    #[app::init]
    pub fn init() -> SvcB {
        SvcB {
            items: UnorderedMap::new(),
        }
    }

    pub fn set(&mut self, key: String, value: u64) -> app::Result<()> {
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<u64>> {
        Ok(self.items.get(key)?.map(|v| *v.get()))
    }
}
