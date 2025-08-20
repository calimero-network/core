#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::UnorderedMap;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct KvStore {
    items: UnorderedMap<String, String>,
    counter: i64,
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        app::log!("Initializing KvStore");
        KvStore {
            items: UnorderedMap::new(),
            counter: 0,
        }
    }

    pub fn set(&mut self) -> app::Result<()> {
        app::log!("Executing set");
        Ok(())
    }

    pub fn delete(&mut self) -> app::Result<()> {
        app::log!("Executing delete");
        Ok(())
    }

    pub fn if(&mut self) -> app::Result<()> {
        app::log!("Executing if");
        Ok(())
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Executing clear");
        Ok(())
    }

    pub fn getCount(&mut self) -> app::Result<()> {
        app::log!("Executing getCount");
        Ok(())
    }
}