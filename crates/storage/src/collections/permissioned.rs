//! Policy-parameterised access-controlled storage.
//!
//! [`PermissionedStorage<T, A>`] is a thin policy layer over
//! [`WriterSetCell<T>`](super::shared::WriterSetCell): it reuses the proven
//! writer-set storage primitive — value held in its own `SharedMember`-stamped
//! entity, writes signed and **verified at merge** against the writer set — and
//! adds an [`Authorizer`] `A` that decides, at the API surface, whether the
//! current executor may perform a given [`Op`].
//!
//! # Why a policy layer, not a new enforcement mechanism
//!
//! The security boundary is unchanged: it is the merge-time signature check in
//! [`Interface`](crate::interface::Interface) against the entity's writer set.
//! A malicious peer that hand-crafts a delta around the WASM still has its write
//! rejected at merge if the signer is not a writer — exactly as for a bare
//! `WriterSetCell`. The [`Authorizer`] is **fail-fast UX sugar**: it lets a
//! method reject an unauthorised caller early with a clear error, using the same
//! writer set the merge check resolves. It must therefore only express policies
//! that reduce to what merge enforces (writer-set membership today); a policy
//! merge cannot replay would be advisory only.
//!
//! Keeping the policy in the type parameter is what lets the components derive
//! from one base: the group-writable form [`SharedStorage<T>`] is the default
//! `PermissionedStorage<T, WriterSetAcl>`, and a single-owner cell is
//! [`Ownable<T>`] = `PermissionedStorage<T, OwnerAcl>`. (`WriterSetCell` is the
//! underlying storage mechanism these all wrap — not used directly by apps.)
//!
//! # Security model — what you MUST store inside the wrapper
//!
//! Access control holds only for data stored *inside* a `PermissionedStorage` /
//! `Ownable` (or another writer-set-guarded type). A value kept in a plain field
//! — a bare `String`, a `u64`, an unguarded collection — rides root state and is
//! writable by **any** context member; the merge-time check guards only the
//! guarded entities. A method that calls `only_owner()?` and then mutates a
//! plain field therefore provides **no** protection for that field. Rule of
//! thumb: every piece of mutable state an [`Authorizer`] is meant to protect
//! must live in the wrapper.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, Mergeable, StorageStrategy};
use super::shared::WriterSetCell;
use super::StoreError;
use crate::entities::{ChildInfo, Data, Element, SignatureData};
use crate::env;
use crate::interface::StorageError;
use crate::store::MainStorage;

/// The class of operation a caller wants to perform on guarded storage.
///
/// Carried by [`Authorizer::authorize`] so a future operation-aware backend
/// (per-principal op masks resolved at the causal cut, enforced at merge) can
/// distinguish, e.g., "write but not delete" without changing this API. The
/// default [`WriterSetAcl`]/[`OwnerAcl`] policies treat every op the same
/// (membership ⇒ authorised), because merge currently enforces membership only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// Read the value. Note: reads are not merge-enforceable in a replicated
    /// store (every node already holds the bytes); this exists for advisory
    /// API gating only.
    Read,
    /// Create or modify the value / a collection entry.
    Write,
    /// Remove the value / a collection entry.
    Delete,
    /// Administrative change: rotate the writer set, grant/revoke.
    Admin,
}

/// Decides whether a principal may perform an [`Op`], given the resource's
/// current writer set.
///
/// Implementors are zero-sized policy markers carried as the `A` type parameter
/// of [`PermissionedStorage`]. `authorize` is a **pure function** of
/// `(who, op, writers)` — no storage reads — so the same predicate is valid at
/// the API call-site and mirrors what the merge-time check enforces. A policy
/// that cannot be reduced to the writer set (and, later, an op mask) merge can
/// replay is advisory only and must be documented as such.
pub trait Authorizer {
    /// Is `who` permitted to perform `op` on a resource guarded by `writers`?
    fn authorize(who: &PublicKey, op: Op, writers: &BTreeSet<PublicKey>) -> bool;
}

/// Membership policy: any writer may perform any op. This is exactly what the
/// merge-time signature check enforces, so the API gate and the merge boundary
/// agree by construction. The default policy for group-writable storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriterSetAcl;

impl Authorizer for WriterSetAcl {
    fn authorize(who: &PublicKey, _op: Op, writers: &BTreeSet<PublicKey>) -> bool {
        writers.contains(who)
    }
}

/// Single-owner policy. Enforcement is identical to [`WriterSetAcl`] at merge
/// (membership in a one-key writer set); the distinction is the API surface
/// ([`Ownable`]'s `owner`/`transfer_ownership`) and the single-writer invariant
/// its constructors maintain.
#[derive(Clone, Copy, Debug, Default)]
pub struct OwnerAcl;

impl Authorizer for OwnerAcl {
    fn authorize(who: &PublicKey, op: Op, writers: &BTreeSet<PublicKey>) -> bool {
        // Same predicate as `WriterSetAcl` (membership); delegate so the two
        // cannot drift if the rule ever changes. The single-owner distinction is
        // an API-surface + constructor invariant, not a different merge rule.
        WriterSetAcl::authorize(who, op, writers)
    }
}

/// Access-controlled storage: a [`WriterSetCell<T>`] plus an [`Authorizer`]
/// policy `A`. The group-writable form (default `A = WriterSetAcl`) behaves like
/// `WriterSetCell`; specialised policies derive components such as [`Ownable`].
#[derive(BorshSerialize, BorshDeserialize)]
pub struct PermissionedStorage<T, A = WriterSetAcl>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    A: Authorizer,
{
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: WriterSetCell<T, MainStorage>,
    /// Zero-sized policy marker; never serialised.
    #[borsh(skip)]
    _policy: PhantomData<A>,
}

impl<T, A> core::fmt::Debug for PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + core::fmt::Debug,
    A: Authorizer,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PermissionedStorage")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T, A> PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    A: Authorizer,
{
    /// Create access-controlled storage with the given initial writer set. Use
    /// for nested fields; the `#[app::state]` macro canonicalises the id via
    /// [`reassign_deterministic_id`](Self::reassign_deterministic_id) after
    /// `init`.
    pub fn new(writers: BTreeSet<PublicKey>, frozen: bool) -> Self {
        Self {
            inner: WriterSetCell::new(writers, frozen),
            _policy: PhantomData,
        }
    }

    /// Create with a deterministic id derived from `field_name`. Use for
    /// top-level state fields constructed outside the `#[app::state]` macro.
    pub fn new_with_field_name(
        field_name: &str,
        writers: BTreeSet<PublicKey>,
        frozen: bool,
    ) -> Self {
        Self {
            inner: WriterSetCell::new_with_field_name(field_name, writers, frozen),
            _policy: PhantomData,
        }
    }

    /// Canonicalise the wrapper id to one derived from `field_name`. Called by
    /// the `#[app::state]` macro after `init()` so every node derives the same
    /// id for a wrapper created via [`new`](Self::new) (random id).
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        self.inner.reassign_deterministic_id(field_name);
    }

    /// Whether `who` may perform `op` under policy `A`, against the current
    /// writer set. Pure; no side effects.
    pub fn can(&self, who: &PublicKey, op: Op) -> bool {
        A::authorize(who, op, &self.inner.writers())
    }

    /// Fail-fast guard: `Ok(())` if the current executor may perform `op`,
    /// else `ActionNotAllowed`. This is UX sugar — the authoritative check is at
    /// merge. Apps may call it for an early, clear rejection before doing work.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the current executor is not authorised for `op`
    /// under policy `A`.
    pub fn guard(&self, op: Op) -> Result<(), StoreError> {
        let me: PublicKey = env::executor_id().into();
        if self.can(&me, op) {
            Ok(())
        } else {
            Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "Executor is not authorised for this operation".to_owned(),
            )))
        }
    }

    /// Read the value (anyone can read).
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compat.
    pub fn get(&self) -> Result<&T, StoreError> {
        self.inner.get()
    }

    /// Replace the value. Enforced at merge: the executor must be a current
    /// writer or the signed write is rejected by peers.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not in the writer set.
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        // Apply the policy at the API surface so a custom `Authorizer` that
        // restricts `Op::Write` more narrowly than plain membership is honoured
        // — the whole point of the seam. `WriterSetCell::insert` then performs
        // the authoritative membership check that peers re-verify at merge.
        self.guard(Op::Write)?;
        self.inner.insert(value)
    }

    /// The current writer set, resolved from verified local sources.
    pub fn writers(&self) -> BTreeSet<PublicKey> {
        self.inner.writers()
    }

    /// Whether the writer set is frozen (rotation permanently rejected).
    pub fn is_frozen(&self) -> bool {
        self.inner.is_frozen()
    }

    /// The signature on the most recently applied rotation, if any.
    pub fn signature(&self) -> Option<SignatureData> {
        self.inner.signature()
    }

    /// Rotate the writer set. Must be called by a current writer; rejected if
    /// frozen or if `new_writers` is empty. Authenticated at merge.
    ///
    /// # Errors
    /// `ActionNotAllowed` if frozen, if `new_writers` is empty, or if the
    /// executor is not a current writer.
    pub fn rotate_writers(&mut self, new_writers: BTreeSet<PublicKey>) -> Result<(), StoreError> {
        // Single API-surface policy gate (honours a custom `Authorizer`).
        // `WriterSetCell::rotate_writers` is authoritative: it re-checks
        // membership and enforces the frozen / non-empty rules.
        self.guard(Op::Admin)?;
        self.inner.rotate_writers(new_writers)
    }
}

impl<T, A> PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + Data,
    A: Authorizer,
{
    /// Mutable access to a collection value for in-place editing. Every entry
    /// inserted through it inherits the wrapper's `SharedMember` domain and is
    /// guarded at merge. Only collections (which implement [`Data`]) get this;
    /// a scalar value is edited via [`insert`](Self::insert).
    ///
    /// # Errors
    /// Currently infallible; the `Result` is preserved for forward compat.
    pub fn get_mut(&mut self) -> Result<&mut T, StoreError> {
        self.inner.get_mut()
    }
}

// `Data` so a `PermissionedStorage` can be nested in `#[app::state]`; the
// wrapper entity is the inner `WriterSetCell`'s element.
impl<T, A> Data for PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    A: Authorizer,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        self.inner.collections()
    }

    fn element(&self) -> &Element {
        self.inner.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.inner.element_mut()
    }
}

// Root-state merge is a no-op, exactly as for `WriterSetCell`: the value is a
// separate entity (merged per-entity) and the writer set converges via the
// verified rotation log. Delegated so the semantics stay identical.
impl<T, A> Mergeable for PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    A: Authorizer,
{
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.inner.merge(&other.inner)
    }
}

impl<T, A> CrdtMeta for PermissionedStorage<T, A>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
    A: Authorizer,
{
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

/// A single-owner storage cell: [`PermissionedStorage`] under the [`OwnerAcl`]
/// policy. Enforcement rides the same merge-time writer-set check as any
/// `WriterSetCell`; ownership is a one-key writer set, and **transfer is a
/// signed, authenticated rotation** — the capability `UserStorage` lacks.
pub type Ownable<T> = PermissionedStorage<T, OwnerAcl>;

/// Group-writable storage: any member of the writer set may read and write, the
/// set is rotatable by a current writer, and every write is verified at merge
/// against it. This is [`PermissionedStorage`] under the default [`WriterSetAcl`]
/// policy — `SharedStorage<T>` and `PermissionedStorage<T, WriterSetAcl>` are the
/// same type. The ergonomic name most apps use.
pub type SharedStorage<T> = PermissionedStorage<T, WriterSetAcl>;

impl<T> PermissionedStorage<T, OwnerAcl>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default,
{
    /// New owned cell whose sole writer (owner) is `owner`.
    pub fn new_owned_by(owner: PublicKey) -> Self {
        Self::new(BTreeSet::from([owner]), false)
    }

    /// New owned cell owned by the current executor (the common case in `init`).
    pub fn new_owned_by_caller() -> Self {
        let me: PublicKey = env::executor_id().into();
        Self::new_owned_by(me)
    }

    /// The current owner, if any (the single writer).
    pub fn owner(&self) -> Option<PublicKey> {
        let mut writers = self.writers().into_iter();
        let first = writers.next();
        debug_assert!(
            writers.next().is_none(),
            "Ownable must hold at most one writer; construct via new_owned_by*"
        );
        first
    }

    /// Whether `who` is the owner.
    pub fn is_owner(&self, who: &PublicKey) -> bool {
        self.writers().contains(who)
    }

    /// Fail-fast owner guard. NOT the security boundary — merge is.
    ///
    /// # Errors
    /// `ActionNotAllowed` if the executor is not the owner.
    pub fn only_owner(&self) -> Result<(), StoreError> {
        self.guard(Op::Admin)
    }

    /// Transfer ownership to `new_owner` via an authenticated writer-set
    /// rotation. A non-owner's forged transfer is rejected at merge.
    ///
    /// # Errors
    /// `ActionNotAllowed` if frozen or the executor is not the current owner.
    pub fn transfer_ownership(&mut self, new_owner: PublicKey) -> Result<(), StoreError> {
        // Owner-gated through `rotate_writers`, which guards `Op::Admin` — for
        // `OwnerAcl` that is exactly the owner check. One gate, no redundancy.
        self.rotate_writers(BTreeSet::from([new_owner]))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use borsh::{BorshDeserialize, BorshSerialize};
    use calimero_primitives::identity::PublicKey;
    use serial_test::serial;

    use super::{Op, Ownable, PermissionedStorage};
    use crate::collections::crdt_meta::{MergeError, Mergeable};
    use crate::collections::Root;
    use crate::entities::Data;
    use crate::{collections::compute_collection_id, env};

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    /// Max-wins Mergeable test value (a valid CRDT).
    #[derive(BorshSerialize, BorshDeserialize, Default, Debug, PartialEq, Clone, Copy)]
    struct TestVal(u64);

    impl Mergeable for TestVal {
        fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
            if other.0 > self.0 {
                self.0 = other.0;
            }
            Ok(())
        }
    }

    fn pk(bytes: [u8; 32]) -> PublicKey {
        bytes.into()
    }

    fn writers(keys: &[[u8; 32]]) -> BTreeSet<PublicKey> {
        keys.iter().copied().map(pk).collect()
    }

    #[test]
    #[serial]
    fn new_with_field_name_is_deterministic() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let _root: Root<TestVal> = Root::new(TestVal::default);

        let expected = compute_collection_id(None, "doc");
        let a =
            PermissionedStorage::<TestVal>::new_with_field_name("doc", writers(&[ALICE]), false);
        assert_eq!(a.element().id(), expected);
    }

    #[test]
    #[serial]
    fn reassign_canonicalises_random_id() {
        // A wrapper built with the random-id `new()` must relocate to the
        // field-derived id when the macro calls `reassign_deterministic_id`,
        // or two nodes would mint different ids for the same field and diverge.
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut p = Root::new(|| PermissionedStorage::<TestVal>::new(writers(&[ALICE]), false));
        p.reassign_deterministic_id("doc");
        assert_eq!(p.element().id(), compute_collection_id(None, "doc"));
    }

    #[test]
    #[serial]
    fn writer_can_write_non_writer_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut p = Root::new(|| PermissionedStorage::<TestVal>::new(writers(&[ALICE]), false));

        // Writer succeeds.
        p.insert(TestVal(1)).unwrap();
        assert_eq!(p.get().unwrap(), &TestVal(1));

        // Non-writer is rejected at the API gate (and would be at merge).
        env::set_executor_id(BOB);
        assert!(p.insert(TestVal(2)).is_err());
        assert!(!p.can(&pk(BOB), Op::Write));
        assert!(p.guard(Op::Write).is_err());
    }

    #[test]
    #[serial]
    fn ownable_reports_owner_and_gates() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let p = Root::new(Ownable::<TestVal>::new_owned_by_caller);

        assert_eq!(p.owner(), Some(pk(ALICE)));
        assert!(p.is_owner(&pk(ALICE)));
        assert!(!p.is_owner(&pk(BOB)));
        assert!(p.only_owner().is_ok());

        env::set_executor_id(BOB);
        assert!(p.only_owner().is_err());
    }

    #[test]
    #[serial]
    fn transfer_ownership_rotates_writer_set() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);
        let mut p = Root::new(|| Ownable::<TestVal>::new_owned_by(pk(ALICE)));
        p.insert(TestVal(1)).unwrap();

        // Owner transfers to Bob (authenticated rotation).
        p.transfer_ownership(pk(BOB)).unwrap();
        assert_eq!(p.owner(), Some(pk(BOB)));

        // Bob is now the writer; Alice is not.
        env::set_executor_id(BOB);
        p.insert(TestVal(2)).unwrap();
        env::set_executor_id(ALICE);
        assert!(p.insert(TestVal(3)).is_err());
    }
}
