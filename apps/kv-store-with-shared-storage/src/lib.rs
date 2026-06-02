use std::collections::BTreeSet;

use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::{LwwRegister, SharedStorage, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct KvStore {
    /// Group-writable register. Initial writer is whoever installed the app.
    /// Rotation lets the writer set evolve over the app's lifetime.
    shared_value: SharedStorage<LwwRegister<String>>,
    /// Group-writable *map*: a whole collection guarded by the same writer set.
    /// Every entry inherits `Shared{writers}`, so a non-writer's delta to any
    /// entry is rejected at merge — not just the wrapper.
    shared_map: SharedStorage<UnorderedMap<String, LwwRegister<String>>>,
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
        writers.insert(initializer);
        KvStore {
            shared_value: SharedStorage::new(writers.clone(), false),
            shared_map: SharedStorage::new(writers, false),
        }
    }

    /// Set the shared value. Caller must be a current writer.
    pub fn set_shared(&mut self, value: String) -> app::Result<()> {
        app::log!("Setting shared value: {:?}", value);
        self.shared_value.insert(LwwRegister::new(value.clone()))?;
        app::emit!(Event::SharedSet { value: &value });
        Ok(())
    }

    /// Get the shared value (anyone can read).
    pub fn get_shared(&self) -> app::Result<String> {
        app::log!("Getting shared value");
        Ok(self.shared_value.get()?.get().clone())
    }

    /// Set an entry in the group-writable map. Only a current writer's write
    /// converges across nodes; a non-writer's write is rejected at merge.
    pub fn map_set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting map entry: {:?} = {:?}", key, value);
        self.shared_map
            .get_mut()?
            .insert(key, LwwRegister::new(value))?;
        Ok(())
    }

    /// Read a map entry, or `None` if absent (anyone can read).
    pub fn map_get(&self, key: String) -> app::Result<Option<String>> {
        app::log!("Getting map entry: {:?}", key);
        Ok(self.shared_map.get()?.get(&key)?.map(|v| v.get().clone()))
    }

    /// Rotate the writer set. Caller must be a current writer; rejected if frozen.
    pub fn rotate_writers(&mut self, new_writers: Vec<PublicKey>) -> app::Result<()> {
        app::log!("Rotating writers: {:?}", new_writers);
        let set: BTreeSet<PublicKey> = new_writers.into_iter().collect();
        let count = set.len() as u32;
        self.shared_value.rotate_writers(set)?;
        app::emit!(Event::WritersRotated { count });
        Ok(())
    }

    /// Read the current writer count of the shared register. Used by the
    /// adversarial e2e to assert a forged rotation did not change the set on the
    /// honest node.
    pub fn writer_count(&self) -> app::Result<u64> {
        Ok(self.shared_value.writers().len() as u64)
    }

    /// Adversarial-test helper: attempt to rotate the writer set, reporting
    /// whether the **local** writer gate accepted it (`true`) or refused it
    /// (`false`) instead of trapping. Lets an e2e assert that a non-writer
    /// member cannot rotate the set without failing the call itself. The
    /// authoritative rejection of a *forged rotation delta* (one that bypasses
    /// this local gate) happens at merge on the honest node and is covered by
    /// the storage-layer test `forged_shared_rotation_rejected_at_merge`.
    pub fn try_rotate_writers(&mut self, new_writers: Vec<PublicKey>) -> app::Result<bool> {
        let set: BTreeSet<PublicKey> = new_writers.into_iter().collect();
        match self.shared_value.rotate_writers(set) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn set_and_get_shared() {
        // The default executor is registered as the sole writer in `init`.
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.set_shared("hello".into())).unwrap();
        assert_eq!(app.view(|s| s.get_shared()).unwrap(), "hello");

        app.call(|s| s.set_shared("world".into())).unwrap();
        assert_eq!(app.view(|s| s.get_shared()).unwrap(), "world");
    }

    #[test]
    fn rotate_writers_emits_event() {
        let mut app = TestHost::new(KvStore::init);

        let me: PublicKey = app.executor_id().into();
        app.call(|s| s.rotate_writers(vec![me])).unwrap();

        assert!(app.events().iter().any(|e| e.kind == "WritersRotated"));
    }

    #[test]
    fn map_set_and_get() {
        // The default executor is the sole writer; writing the guarded map works.
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.map_set("greeting".into(), "hello".into()))
            .unwrap();
        assert_eq!(
            app.view(|s| s.map_get("greeting".into())).unwrap(),
            Some("hello".to_owned())
        );
        assert_eq!(app.view(|s| s.map_get("absent".into())).unwrap(), None);
    }
}
