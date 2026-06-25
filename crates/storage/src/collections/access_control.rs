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
//! # Op-granular roles ([`project_onto`](AccessControl::project_onto))
//!
//! A role can confer a *capability* on guarded data, not just membership. Give
//! each role an [`OpMask`] (e.g. `editor = WRITE`, `moderator = WRITE | DELETE`)
//! and call [`project_onto`](AccessControl::project_onto) to push those masks
//! onto a [`PermissionedStorage`]'s capability map — each member gets the union
//! of its roles' masks (admins keep [`OpMask::FULL`]). The masks are then
//! signed and enforced at merge by
//! [`ProtocolAuthorizer`](super::permissioned::ProtocolAuthorizer): a member
//! with `WRITE` but not `DELETE` has a forged delete rejected by peers.
//!
//! Re-run `project_onto` after any role/admin change — the registry write and
//! the projection are separate signed actions, so `data` converges to the new
//! roles eventually (a transient skew between them is not a security gap; merge
//! always enforces whatever `data`'s map currently resolves to).

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, Mergeable, StorageStrategy};
use super::permissioned::{Authorizer, PermissionedStorage, SharedStorage};
use super::{LwwRegister, StoreError, UnorderedMap};
use crate::entities::{ChildInfo, Data, Element, OpMask};
use crate::env;
use crate::interface::StorageError;

/// Separator between the role name and the member key in a registry key. Role
/// names containing this byte are rejected (see `check_role`), so the composite
/// key is unambiguous without escaping.
const ROLE_MEMBER_SEP: char = '\0';

/// Upper bound on a role name's length. Role names are admin-supplied and become
/// part of a storage key; a bound keeps key sizes sane and caps how much a
/// (compromised) admin can inflate the registry per entry.
const MAX_ROLE_NAME_LEN: usize = 128;

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

    /// Validate a role name. Rejects names containing the separator byte
    /// (`ROLE_MEMBER_SEP`) — without this a name like `"editor\0<hex>"` could
    /// craft a key that collides with a different `(role, member)` pair, a
    /// privilege-confusion vector — and names longer than [`MAX_ROLE_NAME_LEN`].
    /// The separator check references the same constant the key uses, so it
    /// stays correct if the separator is ever changed. Called by every role
    /// method before a key is built.
    fn check_role(role: &str) -> Result<(), StoreError> {
        if role.contains(ROLE_MEMBER_SEP) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Role name must not contain a NUL byte".to_owned(),
            )));
        }
        // Byte length (not char count) — role names become storage keys, so the
        // encoded size is the relevant bound.
        if role.len() > MAX_ROLE_NAME_LEN {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                format!("Role name exceeds the maximum length ({MAX_ROLE_NAME_LEN} bytes)"),
            )));
        }
        Ok(())
    }

    /// Composite registry key for `(role, member)`. Member is hex of its 32
    /// bytes — a stable, separator-free encoding. Callers must `check_role`
    /// first so `role` cannot contain the separator.
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
    /// Only a current admin may.
    ///
    /// One authorization gate per path: `rotate_writers`'s `guard(Op::Admin)`
    /// when the set actually changes, or `only_admin()` on the idempotent no-op
    /// path (where no rotation runs). Never both — matching the one-gate rule.
    /// (Unlike `grant`/`revoke`, whose collection-insert path is not locally
    /// gated and so always need the explicit fail-fast.)
    ///
    /// Concurrent `grant_admin` calls from two admins are last-rotation-wins per
    /// the writer-set rotation merge (ADR 0001): both rotate from the same base
    /// set, so only one added admin may survive the merge. Re-run if a specific
    /// admin must end up in the set.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not a current admin.
    pub fn grant_admin(&mut self, who: PublicKey) -> Result<(), StoreError> {
        let mut admins = self.grants.writers();
        if !admins.insert(who) {
            // Already an admin: no set change, so skip the (otherwise no-op)
            // rotation — but still authorize the caller, since on this path
            // `rotate_writers` wouldn't run to provide the gate.
            return self.only_admin();
        }
        self.grants.rotate_writers(admins)
    }

    /// Remove `who` from the admin set via an authenticated rotation. Only a
    /// current admin may; the set may not become empty.
    ///
    /// The empty-set guard is **best-effort at the API surface**: two admins
    /// concurrently revoking each other both pass their local check, and the
    /// rotations merge per ADR 0001 — the result could drop to one admin. The
    /// merge layer (`rotate_writers` rejecting an empty set) is the backstop;
    /// this local check just gives a clear early error in the common case.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not a current admin, or if removing
    /// `who` would leave no admins.
    pub fn revoke_admin(&mut self, who: &PublicKey) -> Result<(), StoreError> {
        let mut admins = self.grants.writers();
        if !admins.remove(who) {
            // `who` is not an admin: no set change, so skip the no-op rotation —
            // but still authorize the caller (the rotation gate wouldn't run).
            return self.only_admin();
        }
        // `rotate_writers` also rejects an empty set, but bail early with a
        // clearer message before attempting the rotation. Authorize first so a
        // non-admin gets "not an admin" rather than learning the set is down to
        // its last member.
        if admins.is_empty() {
            self.only_admin()?;
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Cannot revoke the last admin".to_owned(),
            )));
        }
        self.grants.rotate_writers(admins)
    }

    // --- named roles (registry entries) ---

    /// Whether `who` currently holds `role`.
    ///
    /// Returns `false` for both "never granted" and "granted then revoked" —
    /// the registry is a current-membership boolean, not a history, so callers
    /// cannot distinguish those two states.
    ///
    /// # Errors
    /// `ActionNotAllowed` if `role` contains the separator byte; propagates a
    /// storage error from the registry lookup.
    pub fn has_role(&self, role: &str, who: &PublicKey) -> Result<bool, StoreError> {
        Self::check_role(role)?;
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
    /// Idempotent in effect (holding the role after any number of grants) but
    /// not in storage: each call writes a fresh `LwwRegister` and emits a delta.
    /// Concurrent `grant`/`revoke` of the same `(role, member)` from different
    /// admins resolve last-writer-wins by the `LwwRegister` timestamp — there is
    /// no semantic tie-break, and a re-grant refreshes that timestamp.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not an admin or `role` contains the
    /// separator byte; propagates a storage error from the write.
    pub fn grant(&mut self, role: &str, who: PublicKey) -> Result<(), StoreError> {
        self.only_admin()?;
        Self::check_role(role)?;
        let key = Self::key(role, &who);
        let _ = self.grants.get_mut()?.insert(key, LwwRegister::new(true))?;
        Ok(())
    }

    /// Revoke `role` from `who` (sets the present-flag to `false`). Only an admin
    /// may; enforced at merge.
    ///
    /// A revoke writes `false` rather than removing the entry (so membership is
    /// a plain LWW boolean with no tombstone). One consequence: a registry that
    /// sees many distinct `(role, member)` pairs over its lifetime accumulates a
    /// `false` entry per pair and does not shrink; this is acceptable for the
    /// expected scale (a context's member/role count), not for unbounded churn.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not an admin or `role` contains the
    /// separator byte; propagates a storage error from the write.
    pub fn revoke(&mut self, role: &str, who: &PublicKey) -> Result<(), StoreError> {
        self.only_admin()?;
        Self::check_role(role)?;
        let key = Self::key(role, who);
        let _ = self
            .grants
            .get_mut()?
            .insert(key, LwwRegister::new(false))?;
        Ok(())
    }

    // --- op-granular projection: roles -> per-writer masks on guarded data ---

    /// Enumerate the current members of `role` (those whose grant flag is
    /// `true`). O(total registry entries) — a prefix scan over `role\0`.
    ///
    /// # Errors
    /// `ActionNotAllowed` if `role` contains the separator byte; propagates a
    /// storage error from the scan.
    pub fn members_of(&self, role: &str) -> Result<Vec<PublicKey>, StoreError> {
        Self::check_role(role)?;
        let prefix = format!("{role}{ROLE_MEMBER_SEP}");
        let mut out = Vec::new();
        for (k, v) in self.grants.get()?.entries()? {
            // The `\0` separator means `role="edit"` never prefix-matches
            // `"editor\0..."`, so this is an exact role match.
            let Some(hex_tail) = k.strip_prefix(&prefix) else {
                continue;
            };
            if !*v.get() {
                continue; // revoked
            }
            if let Ok(bytes) = hex::decode(hex_tail) {
                if let Ok(arr) = <[u8; 32]>::try_from(bytes) {
                    out.push(PublicKey::from(arr));
                }
            }
        }
        Ok(out)
    }

    /// Project role memberships onto `data`'s capability map: every member of a
    /// role listed in `role_masks` receives that role's [`OpMask`] (the **union**
    /// across all roles it holds), and the admins keep [`OpMask::FULL`] so they
    /// can keep administering. This is one authenticated rotation of `data`, so
    /// the caller must be authorised for `Op::Admin` on `data`; the resulting
    /// masks are signed and enforced at merge by [`ProtocolAuthorizer`].
    ///
    /// Re-run after any role/admin change to keep `data` in sync (the registry
    /// write and this projection are separate signed actions — see the module
    /// docs on eventual consistency).
    ///
    /// # Errors
    /// Propagates the registry scan error, or `ActionNotAllowed` if the executor
    /// is not authorised to rotate `data`.
    pub fn project_onto<T, A>(
        &self,
        role_masks: &[(&str, OpMask)],
        data: &mut PermissionedStorage<T, A>,
    ) -> Result<(), StoreError>
    where
        T: BorshSerialize + BorshDeserialize + Mergeable + Default,
        A: Authorizer,
    {
        let mut caps: BTreeMap<PublicKey, OpMask> = BTreeMap::new();
        for (role, mask) in role_masks {
            for member in self.members_of(role)? {
                let entry = caps.entry(member).or_insert(OpMask::NONE);
                *entry = entry.union(*mask);
            }
        }
        // Admins keep FULL so they never lock themselves out of `data`.
        for admin in self.admins() {
            let _ = caps.insert(admin, OpMask::FULL);
        }
        data.set_capabilities(caps)
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

// #D5: RekeyTarget supertrait — delegate to the inner `grants` collection.
impl crate::collections::rekey::RekeyTarget for AccessControl {
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.grants.rekey_relative_to(parent_id);
    }
}

#[diagnostic::do_not_recommend]
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

    #[test]
    #[serial]
    fn only_role_gates_on_held_role() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);
        ac.grant("editor", BOB.into()).unwrap();

        // Alice (admin, but no editor role) is refused; Bob (editor) passes.
        assert!(ac.only_role("editor").is_err());
        env::set_executor_id(BOB);
        assert!(ac.only_role("editor").is_ok());
    }

    #[test]
    #[serial]
    fn revoke_then_check_in_same_execution_is_false() {
        // grant -> revoke -> has_role within one execution must read `false`
        // (the registry is read from storage, not a stale in-memory snapshot).
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        ac.grant("editor", BOB.into()).unwrap();
        assert!(ac.has_role("editor", &BOB.into()).unwrap());
        ac.revoke("editor", &BOB.into()).unwrap();
        assert!(!ac.has_role("editor", &BOB.into()).unwrap());
    }

    #[test]
    #[serial]
    fn role_name_with_separator_is_rejected() {
        // A NUL in the role name could otherwise craft a colliding key.
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        assert!(ac.grant("editor\0evil", BOB.into()).is_err());
        assert!(ac.revoke("editor\0evil", &BOB.into()).is_err());
        assert!(ac.has_role("editor\0evil", &BOB.into()).is_err());
    }

    #[test]
    #[serial]
    fn over_long_role_name_is_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        let long = "r".repeat(super::MAX_ROLE_NAME_LEN + 1);
        assert!(ac.grant(&long, BOB.into()).is_err());
        // A name at the limit is fine.
        let ok = "r".repeat(super::MAX_ROLE_NAME_LEN);
        assert!(ac.grant(&ok, BOB.into()).is_ok());
    }

    #[test]
    #[serial]
    fn members_of_lists_current_holders() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut ac = Root::new(AccessControl::new_admin_caller);

        ac.grant("editor", BOB.into()).unwrap();
        ac.grant("editor", CAROL.into()).unwrap();
        ac.grant("viewer", BOB.into()).unwrap();
        ac.revoke("editor", &CAROL.into()).unwrap();

        let mut editors = ac.members_of("editor").unwrap();
        editors.sort();
        assert_eq!(editors, vec![BOB.into()]); // Carol was revoked
        assert_eq!(ac.members_of("viewer").unwrap(), vec![BOB.into()]);
        assert!(ac.members_of("ghost").unwrap().is_empty());
        // `editor` must not prefix-match a longer role name.
        ac.grant("editorial", CAROL.into()).unwrap();
        assert_eq!(ac.members_of("editor").unwrap(), vec![BOB.into()]);
    }

    #[test]
    #[serial]
    fn project_roles_grants_op_masks_on_data() {
        use std::collections::BTreeSet;

        use borsh::{BorshDeserialize, BorshSerialize};

        use crate::collections::{LwwRegister, Op, PermissionedStorage, ProtocolAuthorizer};
        use crate::entities::OpMask;

        #[derive(BorshSerialize, BorshDeserialize)]
        struct St {
            ac: AccessControl,
            data: PermissionedStorage<LwwRegister<String>, ProtocolAuthorizer>,
        }

        const ROLE_MASKS: &[(&str, OpMask)] = &[
            ("editor", OpMask::WRITE),
            ("moderator", OpMask::WRITE.union(OpMask::DELETE)),
        ];

        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut s = Root::new(|| St {
            ac: AccessControl::new(ALICE.into()),
            data: PermissionedStorage::new(BTreeSet::from([ALICE.into()]), false),
        });

        s.ac.grant("editor", BOB.into()).unwrap();
        s.ac.grant("moderator", CAROL.into()).unwrap();

        {
            let st = &mut *s;
            st.ac.project_onto(ROLE_MASKS, &mut st.data).unwrap();
        }

        // Roles now confer the right ops on `data`, enforced via the mask map.
        assert!(s.data.can(&BOB.into(), Op::Write), "editor may write");
        assert!(
            !s.data.can(&BOB.into(), Op::Delete),
            "editor may NOT delete"
        );
        assert!(
            s.data.can(&CAROL.into(), Op::Delete),
            "moderator may delete"
        );
        assert!(s.data.can(&ALICE.into(), Op::Admin), "admin keeps FULL");

        // Revoke Bob's editor role and re-project → he loses write on `data`.
        s.ac.revoke("editor", &BOB.into()).unwrap();
        {
            let st = &mut *s;
            st.ac.project_onto(ROLE_MASKS, &mut st.data).unwrap();
        }
        assert!(
            !s.data.can(&BOB.into(), Op::Write),
            "revoked editor loses write"
        );
        assert!(s.data.can(&CAROL.into(), Op::Delete), "moderator unchanged");
    }
}
