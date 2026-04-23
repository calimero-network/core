use std::collections::BTreeSet;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{LwwRegister, SharedStorage};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    /// Group-writable register. Initial writer is whoever installed the app.
    /// Rotation lets the writer set evolve over the app's lifetime.
    shared_value: SharedStorage<LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    SharedSet { value: &'a str },
    WritersRotated { count: u32 },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("not initialized: {0}")]
    NotInitialized(&'a str),
}

#[app::logic]
impl KvStore {
    /// Initialize the app. The initial writer set is the installer.
    /// Use `rotate_writers` to add or replace writers.
    #[app::init]
    pub fn init() -> KvStore {
        let initializer: PublicKey = calimero_sdk::env::executor_id().into();
        let mut writers = BTreeSet::new();
        let _ = writers.insert(initializer);
        KvStore {
            shared_value: SharedStorage::new(writers, false),
        }
    }

    /// Set the shared value. Caller must be a current writer.
    pub fn set_shared(&mut self, value: String) -> app::Result<()> {
        app::log!("Setting shared value: {:?}", value);
        let _ = self.shared_value.insert(LwwRegister::new(value.clone()))?;
        app::emit!(Event::SharedSet { value: &value });
        Ok(())
    }

    /// Get the shared value (anyone can read).
    pub fn get_shared(&self) -> app::Result<String> {
        app::log!("Getting shared value");
        Ok(self.shared_value.get()?.get().clone())
    }

    /// Rotate the writer set. Caller must be a current writer; rejected if frozen.
    pub fn rotate_writers(&mut self, new_writers: Vec<PublicKey>) -> app::Result<()> {
        app::log!("Rotating writers: {:?}", new_writers);
        let count = new_writers.len() as u32;
        let set: BTreeSet<PublicKey> = new_writers.into_iter().collect();
        self.shared_value.rotate_writers(set)?;
        app::emit!(Event::WritersRotated { count });
        Ok(())
    }
}
