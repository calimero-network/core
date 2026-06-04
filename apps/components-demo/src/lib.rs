//! Example app for the access-control components.
//!
//! Demonstrates three `calimero_storage` components:
//! - [`Ownable`] — a single-owner config cell with transferable ownership.
//! - [`PermissionedStorage`] — a group-writable settings map.
//! - [`AccessControl`] — an admin-managed role registry.
//!
//! The guards in the methods (`only_owner`, the admin checks inside
//! `AccessControl`) are fail-fast UX. The real authorization boundary is at
//! merge: the components store data inside writer-set-guarded entities, so a
//! peer that hand-crafts a delta around this WASM still has an unauthorized
//! write rejected when honest nodes apply it.

use std::collections::BTreeSet;

use calimero_sdk::{app, env, PublicKey};
use calimero_storage::collections::{
    AccessControl, LwwRegister, Ownable, PermissionedStorage, UnorderedMap,
};

#[app::state]
pub struct ComponentsDemo {
    /// Owner-gated config blob. Ownership is a one-key writer set and is
    /// transferable via an authenticated writer-set rotation.
    config: Ownable<LwwRegister<String>>,
    /// Group-writable settings. Every entry inherits the writer domain, so a
    /// non-writer's forged entry is rejected at merge — not just the wrapper.
    settings: PermissionedStorage<UnorderedMap<String, LwwRegister<String>>>,
    /// Role registry. The installer is the sole initial admin; admins grant /
    /// revoke roles, and a non-admin's forged grant is rejected at merge.
    roles: AccessControl,
}

#[app::logic]
impl ComponentsDemo {
    /// The installer becomes the config owner, sole settings writer, and admin.
    #[app::init]
    pub fn init() -> ComponentsDemo {
        let me: PublicKey = env::executor_id().into();
        ComponentsDemo {
            config: Ownable::new_owned_by(me),
            settings: PermissionedStorage::new(BTreeSet::from([me]), false),
            roles: AccessControl::new(me),
        }
    }

    /// Grant a role to a member. Admin-only (fail-fast here, enforced at merge).
    pub fn grant_role(&mut self, role: String, who: PublicKey) -> app::Result<()> {
        self.roles.grant(&role, who)?;
        Ok(())
    }

    /// Revoke a role from a member. Admin-only.
    pub fn revoke_role(&mut self, role: String, who: PublicKey) -> app::Result<()> {
        self.roles.revoke(&role, &who)?;
        Ok(())
    }

    /// Whether a member holds a role (anyone may query).
    pub fn has_role(&self, role: String, who: PublicKey) -> app::Result<bool> {
        Ok(self.roles.has_role(&role, &who)?)
    }

    /// e2e-support method (not for production). Attempt to grant a role,
    /// returning whether the local admin guard accepted it (`true`) or refused
    /// it (`false`) instead of trapping — so the adversarial workflow can assert
    /// a non-admin is refused without failing the RPC call itself. A real app
    /// should propagate the error (use `grant_role`). The authoritative
    /// rejection of a *forged grant delta* that bypasses this gate happens at
    /// merge (same writer-set-guarded mechanism the settings adversarial proves).
    pub fn try_grant_role(&mut self, role: String, who: PublicKey) -> app::Result<bool> {
        match self.roles.grant(&role, who) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Owner-only write. The guard fails fast; the boundary is that `config` is
    /// a writer-set-guarded entity whose sole writer is the owner.
    pub fn set_config(&mut self, value: String) -> app::Result<()> {
        self.config.only_owner()?;
        self.config.insert(LwwRegister::new(value))?;
        Ok(())
    }

    /// Read the config (anyone may read). Returns the empty string until
    /// `set_config` is first called (the `LwwRegister` default).
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
        // This exercises the fail-fast *API* guard only. The authoritative
        // boundary is merge-time signature verification against the writer set,
        // which requires a 2-node adversarial e2e (design doc §6.3) — a
        // single-host unit test cannot reach the merge path.
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
    fn admin_grants_role_non_admin_cannot() {
        let mut app = TestHost::new(ComponentsDemo::init);
        let other: PublicKey = OTHER.into();

        // Admin (the installer) grants a role.
        app.call(|s| s.grant_role("editor".to_owned(), other))
            .unwrap();
        assert!(app
            .view(|s| s.has_role("editor".to_owned(), other))
            .unwrap());

        // A non-admin's grant is rejected by the fail-fast guard (and would be
        // at merge). Merge-time enforcement needs a 2-node e2e (design §6.3).
        let third: PublicKey = [0x33; 32].into();
        let denied = app.call_as(OTHER, |s| s.grant_role("editor".to_owned(), third));
        assert!(denied.is_err());

        // Admin can revoke.
        app.call(|s| s.revoke_role("editor".to_owned(), other))
            .unwrap();
        assert!(!app
            .view(|s| s.has_role("editor".to_owned(), other))
            .unwrap());
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
