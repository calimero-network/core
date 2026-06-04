//! Role-based access control as a merge-enforced membership registry.
//!
//! [`AccessControl`] tracks which members hold which named roles, backed by a
//! single [`SharedStorage`] whose **writer set is the admin tier**. Granting or
//! revoking a role mutates the guarded registry, so — like every component here
//! — the authoritative check is at merge: a non-admin's hand-crafted grant delta
//! is rejected because the registry entries are writer-set-guarded. Admin
//! changes are a writer-set rotation (O(1), authenticated). `only_role` /
//! `only_admin` are fail-fast API guards that mirror what merge enforces.
//!
//! # Model
//!
//! - **Admins** are exactly the writer set of the backing storage. Any admin may
//!   grant/revoke any role (a single admin tier, like OpenZeppelin's
//!   `DEFAULT_ADMIN_ROLE`). Change the admin set with
//!   [`grant_admin`](AccessControl::grant_admin) /
//!   [`revoke_admin`](AccessControl::revoke_admin) (authenticated rotation).
//! - **Roles** are named member sets recorded in the guarded registry. A grant
//!   is a present-flag entry (`true`); a revoke sets it to `false` rather than
//!   removing, so membership is a last-writer-wins boolean and never needs a
//!   tombstone. [`has_role`](AccessControl::has_role) is the lookup app guards
//!   use.
//!
//! To make a role actually gate *writes to specific data*, store that data in
//! its own [`SharedStorage`] and rotate its writers to the role's members; this
//! registry is the source of truth for "who is in role R", and `only_role` is
//! the fail-fast guard at the API surface.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, Mergeable, StorageStrategy};
use super::permissioned::SharedStorage;
use super::{LwwRegister, StoreError, UnorderedMap};
use crate::entities::{ChildInfo, Data, Element};
use crate::env;
use crate::interface::StorageError;

/// Separator between the role name and the member key in a registry key. A NUL
/// byte cannot appear in a well-formed role name, so the composite key is
/// unambiguous without escaping.
const ROLE_MEMBER_SEP: char = '\0';

/// The registry value type: a last-writer-wins boolean. `true` = the member
/// currently holds the role; `false` = revoked. Storing a flag (rather than
/// removing the entry) keeps membership a plain LWW merge with no tombstone.
type Grant = LwwRegister<bool>;

/// Role-based access control backed by a single writer-set-guarded registry.
///
/// The backing [`SharedStorage`]'s writers are the admin tier; role grants are
/// entries in its guarded map. See the [module docs](self) for the model.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct AccessControl {
    /// `role\0member_hex -> LwwRegister<bool>`. Writers of this storage are the
    /// admins; entries inherit the writer domain and are verified at merge.
    #[borsh(bound(serialize = "", deserialize = ""))]
    grants: SharedStorage<UnorderedMap<String, Grant>>,
}

impl core::fmt::Debug for AccessControl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AccessControl")
            .field("grants", &self.grants)
            .finish()
    }
}

impl AccessControl {
    /// Create an `AccessControl` with `admin` as the sole initial admin. Use for
    /// nested fields; the `#[app::state]` macro canonicalises the id via
    /// [`reassign_deterministic_id`](Self::reassign_deterministic_id).
    pub fn new(admin: PublicKey) -> Self {
        Self {
            grants: SharedStorage::new(BTreeSet::from([admin]), false),
        }
    }

    /// Create an `AccessControl` administered by the current executor (the
    /// common case in `init`).
    pub fn new_admin_caller() -> Self {
        let me: PublicKey = env::executor_id().into();
        Self::new(me)
    }

    /// Canonicalise the backing storage id from `field_name`. Called by the
    /// `#[app::state]` macro after `init()` so every node derives the same id.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.grants.reassign_deterministic_id(field_name);
    }

    /// Composite registry key for `(role, member)`. Member is hex of its 32
    /// bytes — a stable, separator-free encoding.
    fn key(role: &str, member: &PublicKey) -> String {
        let member_hex = hex::encode(member.as_ref() as &[u8; 32]);
        format!("{role}{ROLE_MEMBER_SEP}{member_hex}")
    }

    // --- admin tier (the writer set) ---

    /// The current admin set (the backing storage's writers).
    pub fn admins(&self) -> BTreeSet<PublicKey> {
        self.grants.writers()
    }

    /// Whether `who` is an admin.
    pub fn is_admin(&self, who: &PublicKey) -> bool {
        self.grants.writers().contains(who)
    }

    /// Fail-fast admin guard. NOT the security boundary — merge is.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the current executor is not an admin.
    pub fn only_admin(&self) -> Result<(), StoreError> {
        let me: PublicKey = env::executor_id().into();
        if self.is_admin(&me) {
            Ok(())
        } else {
            Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not an admin".to_owned(),
            )))
        }
    }

    /// Add `who` to the admin set via an authenticated writer-set rotation.
    /// Only a current admin may; enforced at merge.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not a current admin.
    pub fn grant_admin(&mut self, who: PublicKey) -> Result<(), StoreError> {
        let mut admins = self.grants.writers();
        let _ = admins.insert(who);
        self.grants.rotate_writers(admins)
    }

    /// Remove `who` from the admin set via an authenticated rotation. Only a
    /// current admin may; enforced at merge. The set may not become empty.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not a current admin, or if removing
    /// `who` would leave no admins.
    pub fn revoke_admin(&mut self, who: &PublicKey) -> Result<(), StoreError> {
        let mut admins = self.grants.writers();
        let _ = admins.remove(who);
        // `rotate_writers` also rejects an empty set, but bail early with a
        // clearer message before attempting the rotation.
        if admins.is_empty() {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot revoke the last admin".to_owned(),
            )));
        }
        self.grants.rotate_writers(admins)
    }

    // --- named roles (registry entries) ---

    /// Whether `who` currently holds `role`.
    ///
    /// # Errors
    /// Propagates a storage error from the registry lookup.
    pub fn has_role(&self, role: &str, who: &PublicKey) -> Result<bool, StoreError> {
        let key = Self::key(role, who);
        Ok(self
            .grants
            .get()?
            .get(&key)?
            .map(|v| *v.get())
            .unwrap_or(false))
    }

    /// Fail-fast guard: the current executor must hold `role`. NOT the security
    /// boundary — merge is.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the current executor does not hold `role`;
    /// propagates a storage error from the lookup.
    pub fn only_role(&self, role: &str) -> Result<(), StoreError> {
        let me: PublicKey = env::executor_id().into();
        if self.has_role(role, &me)? {
            Ok(())
        } else {
            Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor does not hold the required role".to_owned(),
            )))
        }
    }

    /// Grant `role` to `who`. Only an admin may; enforced at merge (the registry
    /// entry inherits the admin writer set).
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not an admin; propagates a storage
    /// error from the write.
    pub fn grant(&mut self, role: &str, who: PublicKey) -> Result<(), StoreError> {
        self.only_admin()?;
        let key = Self::key(role, &who);
        let _ = self.grants.get_mut()?.insert(key, LwwRegister::new(true))?;
        Ok(())
    }

    /// Revoke `role` from `who` (sets the present-flag to `false`). Only an admin
    /// may; enforced at merge.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not an admin; propagates a storage
    /// error from the write.
    pub fn revoke(&mut self, role: &str, who: &PublicKey) -> Result<(), StoreError> {
        self.only_admin()?;
        let key = Self::key(role, who);
        let _ = self
            .grants
            .get_mut()?
            .insert(key, LwwRegister::new(false))?;
        Ok(())
    }
}

// `Data`/`Mergeable`/`CrdtMeta` delegate to the backing storage so `AccessControl`
// can be nested directly in `#[app::state]`, mirroring `PermissionedStorage`.
impl Data for AccessControl {
    fn collections(&self) -> std::collections::BTreeMap<String, Vec<ChildInfo>> {
        self.grants.collections()
    }

    fn element(&self) -> &Element {
        self.grants.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.grants.element_mut()
    }
}

impl Mergeable for AccessControl {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.grants.merge(&other.grants)
    }
}

impl CrdtMeta for AccessControl {
    fn crdt_type() -> CrdtType {
        CrdtType::SharedStorage
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::AccessControl;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];
    const CAROL: [u8; 32] = [0x33; 32];

    #[test]
    #[serial]
    fn admin_can_grant_and_revoke_roles() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        assert!(ac.is_admin(&ALICE.into()));
        assert!(!ac.has_role("editor", &BOB.into()).unwrap());

        ac.grant("editor", BOB.into()).unwrap();
        assert!(ac.has_role("editor", &BOB.into()).unwrap());

        ac.revoke("editor", &BOB.into()).unwrap();
        assert!(!ac.has_role("editor", &BOB.into()).unwrap());
    }

    #[test]
    #[serial]
    fn non_admin_cannot_grant() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        // Bob is not an admin — the fail-fast guard rejects his grant.
        env::set_executor_id(BOB);
        assert!(ac.grant("editor", CAROL.into()).is_err());
        assert!(ac.only_admin().is_err());
    }

    #[test]
    #[serial]
    fn admin_tier_rotation() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        ac.grant_admin(BOB.into()).unwrap();
        assert!(ac.is_admin(&BOB.into()));

        // Bob, now an admin, can grant.
        env::set_executor_id(BOB);
        ac.grant("editor", CAROL.into()).unwrap();
        assert!(ac.has_role("editor", &CAROL.into()).unwrap());

        // An admin can drop another admin, but not the last one.
        ac.revoke_admin(&ALICE.into()).unwrap();
        assert!(!ac.is_admin(&ALICE.into()));
        assert!(ac.revoke_admin(&BOB.into()).is_err());
    }

    #[test]
    #[serial]
    fn roles_are_independent() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        ac.grant("editor", BOB.into()).unwrap();
        ac.grant("viewer", BOB.into()).unwrap();
        ac.revoke("editor", &BOB.into()).unwrap();

        assert!(!ac.has_role("editor", &BOB.into()).unwrap());
        assert!(ac.has_role("viewer", &BOB.into()).unwrap());
    }
}
