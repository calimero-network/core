//! Example app for the access-control components.
//!
//! Demonstrates two of the `calimero_storage` components:
//! - [`Ownable`] — a single-owner config cell with transferable ownership.
//! - [`PermissionedStorage`] — a group-writable settings map.
//!
//! The guards in the methods (`only_owner`) are fail-fast UX. The real
//! authorization boundary is at merge: the components store data inside
//! writer-set-guarded entities, so a peer that hand-crafts a delta around this
//! WASM still has an unauthorized write rejected when honest nodes apply it.

use std::collections::BTreeSet;

use calimero_sdk::{app, env, PublicKey};
use calimero_storage::collections::{LwwRegister, Ownable, PermissionedStorage, UnorderedMap};

#[app::state]
pub struct ComponentsDemo {
    /// Owner-gated config blob. Ownership is a one-key writer set and is
    /// transferable via an authenticated writer-set rotation.
    config: Ownable<LwwRegister<String>>,
    /// Group-writable settings. Every entry inherits the writer domain, so a
    /// non-writer's forged entry is rejected at merge — not just the wrapper.
    settings: PermissionedStorage<UnorderedMap<String, LwwRegister<String>>>,
}

#[app::logic]
impl ComponentsDemo {
    /// The installer becomes the config owner and the sole settings writer.
    #[app::init]
    pub fn init() -> ComponentsDemo {
        let me: PublicKey = env::executor_id().into();
        ComponentsDemo {
            config: Ownable::new_owned_by(me),
            settings: PermissionedStorage::new(BTreeSet::from([me]), false),
        }
    }

    /// Owner-only write. The guard fails fast; the boundary is that `config` is
    /// a writer-set-guarded entity whose sole writer is the owner.
    pub fn set_config(&mut self, value: String) -> app::Result<()> {
        self.config.only_owner()?;
        self.config.insert(LwwRegister::new(value))?;
        Ok(())
    }

    /// Read the config (anyone may read).
    pub fn get_config(&self) -> app::Result<String> {
        Ok(self.config.get()?.get().clone())
    }

    /// The current owner of the config cell.
    pub fn owner(&self) -> app::Result<Option<PublicKey>> {
        Ok(self.config.owner())
    }

    /// Transfer ownership to `new_owner`. Only the current owner may; the
    /// rotation is authenticated at merge, so a non-owner's forged transfer is
    /// rejected.
    pub fn transfer(&mut self, new_owner: PublicKey) -> app::Result<()> {
        self.config.transfer_ownership(new_owner)?;
        Ok(())
    }

    /// Set a settings entry. Only a current writer's entry converges across
    /// nodes; a non-writer's write is rejected at merge.
    pub fn settings_set(&mut self, key: String, value: String) -> app::Result<()> {
        self.settings
            .get_mut()?
            .insert(key, LwwRegister::new(value))?;
        Ok(())
    }

    /// Read a settings entry, or `None` if absent.
    pub fn settings_get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.settings.get()?.get(&key)?.map(|v| v.get().clone()))
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const OTHER: [u8; 32] = [0x22; 32];

    #[test]
    fn owner_can_set_and_read_config() {
        let mut app = TestHost::new(ComponentsDemo::init);
        app.call(|s| s.set_config("hello".to_owned())).unwrap();
        assert_eq!(app.view(|s| s.get_config()).unwrap(), "hello");
    }

    #[test]
    fn non_owner_cannot_set_config() {
        let mut app = TestHost::new(ComponentsDemo::init);
        // A different executor is not the owner — the guard rejects it.
        let result = app.call_as(OTHER, |s| s.set_config("forged".to_owned()));
        assert!(result.is_err());
    }

    #[test]
    fn transfer_moves_control_to_new_owner() {
        let mut app = TestHost::new(ComponentsDemo::init);
        let other: PublicKey = OTHER.into();

        app.call(|s| s.transfer(other)).unwrap();
        assert_eq!(app.view(|s| s.owner()).unwrap(), Some(other));

        // The new owner can write; the old owner no longer can.
        app.call_as(OTHER, |s| s.set_config("by-other".to_owned()))
            .unwrap();
        assert_eq!(app.view(|s| s.get_config()).unwrap(), "by-other");
        assert!(app.call(|s| s.set_config("nope".to_owned())).is_err());
    }

    #[test]
    fn settings_map_roundtrip() {
        let mut app = TestHost::new(ComponentsDemo::init);
        app.call(|s| s.settings_set("k".to_owned(), "v".to_owned()))
            .unwrap();
        assert_eq!(
            app.view(|s| s.settings_get("k".to_owned())).unwrap(),
            Some("v".to_owned())
        );
        assert_eq!(
            app.view(|s| s.settings_get("absent".to_owned())).unwrap(),
            None
        );
    }
}
