//! Projection-backed [`AtCutAuthorizer`] for the apply gates (F5 #28 stage 3b/4).
//!
//! Realizes the dependency inversion opened by the governance-store seam: the
//! apply gates call the `AtCutAuthorizer` trait (defined in `governance-store`),
//! and this implementation resolves the decision against the unified projection at
//! the op's causal cut, falling back to the live resolver (`None`) when the cited
//! ancestry isn't fully folded.
//!
//! It folds an EPHEMERAL projection from the store — the same mechanism the
//! read-side query gates use ([`ScopeProjections::ephemeral_projection`]) — so it
//! needs no shared projection state threaded into the apply path. The fold is cached
//! per authorizer (i.e. per apply): one op may hit several gates (admin, capability,
//! last-admin) but touches one namespace, so the DAG is walked once. The fold is
//! bounded by `MAX_BACKFILL_OPS`; governance ops are infrequent, and P6 sync
//! unification replaces the ephemeral fold with the maintained projection.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use calimero_account::{AccountId, DeviceId};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::metrics::{record_at_cut_undecidable, UndecidableCause};
use calimero_governance_store::{
    AtCutAuthorizer, AtCutMembershipPath, GroupRows, NamespaceDagService, StandingReads,
};
use calimero_op::{Op, ScopeId};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_projection::Acting;
use calimero_store::Store;

use crate::scope_projection::{authority_base, ScopeProjections};

/// The cached ephemeral fold: the folded projection plus the namespace id + heads
/// `ScopeProjections::ephemeral_projection` returns (kept so the shape matches even
/// though the at-cut reads take the op's own `parents`, not these heads).
type FoldedProjection = (ScopeProjections, [u8; 32], Vec<[u8; 32]>);

/// An [`AtCutAuthorizer`] that resolves each gate against an ephemeral projection of
/// the op's namespace, at the op's causal cut. Constructed at the namespace DAG
/// applier (the one call site); `pub(crate)` because it's an implementation detail,
/// not API other crates should depend on.
pub(crate) struct EphemeralProjectionAuthorizer<'a> {
    store: &'a Store,
    /// Per-apply fold cache (see module doc). Keyed by `group`: every gate for one op
    /// passes the same group, so after the first fold the rest are cache hits; a
    /// different group re-folds. `Mutex` (not `RefCell`) because the trait is
    /// `Send + Sync` and drives a spawned `async fn apply` future.
    cache: Mutex<Option<(ContextGroupId, Arc<FoldedProjection>)>>,
}

impl<'a> EphemeralProjectionAuthorizer<'a> {
    pub(crate) fn new(store: &'a Store) -> Self {
        Self {
            store,
            cache: Mutex::new(None),
        }
    }

    /// Fold the ephemeral projection for `group`'s namespace ONCE and cache it, so
    /// the several gates a single op hits reuse the fold instead of re-walking the
    /// DAG each time. `None` = the fold couldn't be built (store/DAG-head fault) —
    /// warned, and the gate defers to live; the expected "ancestry not yet folded"
    /// case instead surfaces as `None` from the at-cut read on a successful fold.
    fn folded(&self, group: &ContextGroupId) -> Option<Arc<FoldedProjection>> {
        let mut slot = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((cached_group, folded)) = slot.as_ref() {
            if cached_group == group {
                return Some(Arc::clone(folded));
            }
        }
        let Some(folded) = ScopeProjections::ephemeral_projection(self.store, group) else {
            tracing::warn!(
                group = ?group,
                "apply-auth: ephemeral projection unavailable (store/DAG-head fault); gate defers to live"
            );
            return None;
        };
        let arc = Arc::new(folded);
        *slot = Some((*group, Arc::clone(&arc)));
        Some(arc)
    }
}

/// Judges void ops against the projection and leaves every authority gate to the live
/// resolver. For replays that never ran the at-cut gates, so they keep their old answers.
pub struct VoidJudge<'a>(EphemeralProjectionAuthorizer<'a>);

impl<'a> VoidJudge<'a> {
    #[must_use]
    pub fn new(store: &'a Store) -> Self {
        Self(EphemeralProjectionAuthorizer::new(store))
    }
}

impl AtCutAuthorizer for VoidJudge<'_> {
    fn is_admin_at_cut(&self, _: &ContextGroupId, _: &PublicKey, _: &[[u8; 32]]) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_at_cut(
        &self,
        _: &ContextGroupId,
        _: &PublicKey,
        _: u32,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_account_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: u32,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_account_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_last_admin_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn membership_path_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<AtCutMembershipPath> {
        None
    }

    fn effective_role_at_cut(
        &self,
        _: &ContextGroupId,
        _: &AccountId,
        _: &[[u8; 32]],
    ) -> Option<Option<GroupMemberRole>> {
        None
    }

    fn context_rotation_group_at_cut(
        &self,
        _: &ContextGroupId,
        _: &ContextId,
        _: &[[u8; 32]],
    ) -> Option<Option<ContextGroupId>> {
        None
    }

    // No replay predates this gate, so reading the cut changes no answer a replay gave.
    fn device_epoch_superseded_at_cut(
        &self,
        group: &ContextGroupId,
        account: &AccountId,
        device: &DeviceId,
        device_epoch: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        self.0
            .device_epoch_superseded_at_cut(group, account, device, device_epoch, parents)
    }

    fn forget(&self) {
        self.0.forget();
    }

    fn op_is_void(&self, group: &ContextGroupId, capability: u32, op: &Op) -> Option<bool> {
        self.0.op_is_void(group, capability, op)
    }

    fn voided_ops(
        &self,
        group: &ContextGroupId,
        applied: Option<&Op>,
        held: &[([u8; 32], ContextGroupId)],
    ) -> Option<BTreeSet<[u8; 32]>> {
        self.0.voided_ops(group, applied, held)
    }

    fn group_rows(&self, group: &ContextGroupId, applied: Option<&Op>) -> Option<GroupRows> {
        self.0.group_rows(group, applied)
    }
}

impl AtCutAuthorizer for EphemeralProjectionAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // No cut (empty `parents`) ⇒ no causal context to resolve against; defer to
        // live rather than judge against an empty/genesis view (which could falsely
        // reject a capability-holder whose grant isn't in the empty fold). Belt-and-
        // braces: group ops carry their enclosing namespace op's parents, which are
        // empty only for the namespace genesis itself.
        if parents.is_empty() {
            return None;
        }
        // `None` here = the cited ancestry isn't fully folded; the gate defers to
        // live (quiet — a normal mid-backfill state, not an error).
        self.folded(group)?
            .0
            .is_admin_at_cut(self.store, *group, signer, parents)
    }

    fn is_admin_or_capability_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        capability: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .is_admin_or_capability_at_cut(self.store, *group, signer, capability, parents)
    }

    fn is_admin_or_capability_account_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        capability: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .is_admin_or_capability_account_at_cut(self.store, *group, member, capability, parents)
    }

    fn is_admin_account_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .is_admin_account_at_cut(self.store, *group, member, parents)
    }

    fn is_last_admin_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .is_last_admin_at_cut(self.store, *group, member, parents)
    }

    fn membership_path_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        parents: &[[u8; 32]],
    ) -> Option<AtCutMembershipPath> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        let path = self
            .folded(group)?
            .0
            .membership_path_at_cut(self.store, *group, member, parents)?;
        // Project the role/anchor detail away to the kind the gate needs.
        Some(match path {
            calimero_authz::MemberPathAtCut::None => AtCutMembershipPath::None,
            calimero_authz::MemberPathAtCut::Direct { .. } => AtCutMembershipPath::Direct,
            calimero_authz::MemberPathAtCut::Inherited { .. } => AtCutMembershipPath::Inherited,
        })
    }

    fn device_epoch_superseded_at_cut(
        &self,
        group: &ContextGroupId,
        account: &AccountId,
        device: &DeviceId,
        device_epoch: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?.0.device_epoch_superseded_at_cut(
            self.store,
            *group,
            account,
            device,
            device_epoch,
            parents,
        )
    }

    fn effective_role_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        parents: &[[u8; 32]],
    ) -> Option<Option<GroupMemberRole>> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .effective_role_at_cut(self.store, *group, member, parents)
    }

    fn context_rotation_group_at_cut(
        &self,
        group: &ContextGroupId,
        context: &ContextId,
        parents: &[[u8; 32]],
    ) -> Option<Option<ContextGroupId>> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .context_rotation_group_at_cut(self.store, *group, context, parents)
    }

    fn op_is_void(&self, group: &ContextGroupId, capability: u32, op: &Op) -> Option<bool> {
        let folded = self.folded(group)?;
        let (projection, namespace_id, _) = &*folded;
        projection.op_is_void(
            &ScopeId::from(*namespace_id),
            authority_base(self.store, *namespace_id)?,
            op,
            Some(Acting {
                group: *group,
                capability,
            }),
        )
    }

    fn forget(&self) {
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn voided_ops(
        &self,
        group: &ContextGroupId,
        applied: Option<&Op>,
        held: &[([u8; 32], ContextGroupId)],
    ) -> Option<BTreeSet<[u8; 32]>> {
        let folded = self.folded(group)?;
        let (projection, namespace_id, _) = &*folded;
        projection.voided_with(
            &ScopeId::from(*namespace_id),
            authority_base(self.store, *namespace_id)?,
            applied,
            held,
        )
    }

    fn group_rows(&self, group: &ContextGroupId, applied: Option<&Op>) -> Option<GroupRows> {
        let folded = self.folded(group)?;
        let (projection, namespace_id, _) = &*folded;
        // The heads as the apply left them, which the fold predates.
        let heads = NamespaceDagService::new(self.store, (*namespace_id).into())
            .read_head_record()
            .ok()?
            .parent_hashes;
        projection.group_rows_with(
            &ScopeId::from(*namespace_id),
            authority_base(self.store, *namespace_id)?,
            group,
            &heads,
            applied,
        )
    }

    fn can_resolve_cut(&self, group: &ContextGroupId, parents: &[[u8; 32]]) -> bool {
        // An empty cut is a genesis op: there is no causal context, so the gates defer
        // to live and that IS the right answer. Say `true` — nothing to be undecided
        // about.
        if parents.is_empty() {
            return true;
        }
        // A real cut. We can only decide it if the fold exists AND has the cited
        // ancestry; otherwise the gates must refuse rather than answer from live,
        // which resolves a different cut entirely.
        //
        // The two halves fail for unrelated reasons, so they are counted apart.
        // `folded()` returning `None` is a store fault, not a history gap, and
        // `can_resolve_cut` cannot see it — it is never reached — so the cause is
        // recorded here or nowhere. Note the asymmetry with the predicates above:
        // they read a failed fold as "defer to live", while this refuses. The
        // refusal is what wins in practice, since the apply gates consult this
        // before trusting a live answer.
        let Some(folded) = self.folded(group) else {
            record_at_cut_undecidable(UndecidableCause::FoldUnavailable);
            return false;
        };
        folded.0.can_resolve_cut(self.store, *group, parents)
    }

    fn standing_reads_at_cut<'s>(
        &'s self,
        group: &ContextGroupId,
        parents: &[[u8; 32]],
    ) -> Option<Box<dyn StandingReads + 's>> {
        // Empty cut ⇒ defer to live (see `is_admin_at_cut`).
        if parents.is_empty() {
            return None;
        }
        let reads = self
            .folded(group)?
            .0
            .standing_reads_at_cut(self.store, *group, parents)?;
        Some(Box::new(reads))
    }

    fn cut_covers_at_cut(
        &self,
        group: &ContextGroupId,
        parents: &[[u8; 32]],
        floor: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        self.folded(group)?
            .0
            .cut_covers_floor(self.store, *group, parents, floor)
    }
}

/// An [`AtCutAuthorizer`] over the node's **maintained** projection, for the
/// cut a state delta cites.
///
/// The governance apply folds an ephemeral projection per op
/// ([`EphemeralProjectionAuthorizer`]); a state delta cannot afford that on
/// the hot receive path, and does not need to: the receive path has already
/// refreshed the maintained projection for the delta's governance cut, to
/// resolve the author's membership there. This reads the same fold, so the
/// delegated gate and the membership check see one view.
///
/// Only the reads a delegated delta's admission asks are answered; the
/// governance-op predicates abstain (`None`), which is correct for them too —
/// a state delta publishes no governance op.
pub struct ProjectionAuthorizer<'a> {
    projections: &'a RwLock<ScopeProjections>,
    store: &'a Store,
}

impl<'a> ProjectionAuthorizer<'a> {
    /// Read `projections` for the cuts asked about.
    #[must_use]
    pub const fn new(projections: &'a RwLock<ScopeProjections>, store: &'a Store) -> Self {
        Self { projections, store }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, ScopeProjections> {
        // A poisoned lock only means a panic elsewhere; the fold still answers.
        self.projections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl AtCutAuthorizer for ProjectionAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _capability: u32,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_account_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _capability: u32,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_account_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_last_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn membership_path_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<AtCutMembershipPath> {
        None
    }

    fn effective_role_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<Option<GroupMemberRole>> {
        None
    }

    fn context_rotation_group_at_cut(
        &self,
        _group: &ContextGroupId,
        _context: &ContextId,
        _parents: &[[u8; 32]],
    ) -> Option<Option<ContextGroupId>> {
        None
    }

    fn can_resolve_cut(&self, group: &ContextGroupId, parents: &[[u8; 32]]) -> bool {
        parents.is_empty() || self.read().can_resolve_cut(self.store, *group, parents)
    }

    fn standing_reads_at_cut<'s>(
        &'s self,
        group: &ContextGroupId,
        parents: &[[u8; 32]],
    ) -> Option<Box<dyn StandingReads + 's>> {
        if parents.is_empty() {
            return None;
        }
        // The reads own their folded view, so the lock is not held past here.
        let reads = self
            .read()
            .standing_reads_at_cut(self.store, *group, parents)?;
        Some(Box::new(reads))
    }

    fn cut_covers_at_cut(
        &self,
        group: &ContextGroupId,
        parents: &[[u8; 32]],
        floor: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        self.read()
            .cut_covers_floor(self.store, *group, parents, floor)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::MetaRepository;
    use calimero_op::{Authorship, Op, OpPayload, ScopeId};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;

    const NS: [u8; 32] = [0x3C; 32];
    const OWNER: u8 = 0x10;
    const ALICE: u8 = 0x11;
    const SAM: u8 = 0x12;
    const XAVIER: u8 = 0x14;
    const ZED: u8 = 0x16;

    fn acct(n: u8) -> AccountId {
        AccountId::from([n; 32])
    }

    fn gov(author: u8, parents: &[&Op], payload: OpPayload) -> Op {
        Op::new(
            ScopeId::from(NS),
            parents.iter().map(|p| p.id()).collect(),
            Authorship {
                account: acct(author),
                device: calimero_account::DeviceId::from([author; 32]),
                device_key: PublicKey::from([author; 32]),
            },
            HybridTimestamp::default(),
            payload,
            [0u8; 32],
            [0u8; 64],
        )
    }

    fn add(author: u8, parents: &[&Op], member: u8, role: GroupMemberRole) -> Op {
        gov(
            author,
            parents,
            OpPayload::MemberAdded {
                group: ContextGroupId::from(NS),
                member: acct(member),
                role,
            },
        )
    }

    /// A namespace whose log holds two admins, Alice's removal of Sam, and the
    /// op Sam sent from the cut before it.
    fn namespace() -> (Store, Op, Op, Op) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let root = ContextGroupId::from(NS);
        MetaRepository::new(&store)
            .save(
                &root,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: calimero_primitives::application::ApplicationId::from(
                            [0xCC; 32],
                        ),
                        bytecode_id: [0xBB; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1_700_000_000,
                    admin_identity: acct(OWNER),
                    owner_identity: acct(OWNER),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("root meta");
        let alice = add(OWNER, &[], ALICE, GroupMemberRole::Admin);
        let sam = add(OWNER, &[&alice], SAM, GroupMemberRole::Admin);
        let removal = gov(
            ALICE,
            &[&sam],
            OpPayload::MemberRemoved {
                group: root,
                member: acct(SAM),
            },
        );
        for op in [&alice, &sam, &removal] {
            crate::unified_op_store::persist_op(&store, op).expect("persist");
        }
        (store, sam, alice, removal)
    }

    #[test]
    fn an_op_is_judged_void_against_the_stored_log() {
        let (store, sam, _, _) = namespace();
        let root = ContextGroupId::from(NS);
        let authorizer = EphemeralProjectionAuthorizer::new(&store);

        let from_the_old_cut = add(SAM, &[&sam], XAVIER, GroupMemberRole::Admin);
        assert_eq!(
            authorizer.op_is_void(&root, 0, &from_the_old_cut),
            Some(true)
        );
        let by_alice = add(ALICE, &[&sam], ZED, GroupMemberRole::Member);
        assert_eq!(authorizer.op_is_void(&root, 0, &by_alice), Some(false));
    }

    #[test]
    fn the_void_set_and_the_rows_are_read_over_the_log_plus_the_op_just_applied() {
        let (store, sam, alice, removal) = namespace();
        let root = ContextGroupId::from(NS);
        let authorizer = EphemeralProjectionAuthorizer::new(&store);

        let from_the_old_cut = add(SAM, &[&sam], XAVIER, GroupMemberRole::Admin);
        let voided = authorizer
            .voided_ops(&root, Some(&from_the_old_cut), &[])
            .expect("the log folds");
        assert_eq!(voided, BTreeSet::from([from_the_old_cut.id()]));

        // The rows at the heads the apply left: the removal and Sam's op.
        let dag = NamespaceDagService::new(&store, NS.into());
        for (seq, (op, parents)) in [
            (&alice, vec![]),
            (&sam, vec![alice.id()]),
            (&removal, vec![sam.id()]),
            (&from_the_old_cut, vec![sam.id()]),
        ]
        .into_iter()
        .enumerate()
        {
            dag.advance_dag_head(op.id(), &parents, seq as u64 + 1)
                .expect("advance the head");
        }
        let rows = authorizer
            .group_rows(&root, Some(&from_the_old_cut))
            .expect("the log folds");
        assert!(rows.members.contains_key(&acct(ALICE)));
        assert!(!rows.members.contains_key(&acct(SAM)), "removed");
        assert!(
            !rows.members.contains_key(&acct(XAVIER)),
            "added by a void op"
        );
    }

    #[test]
    fn a_projection_backfilled_with_its_store_reads_the_owner_from_it() {
        let (store, _sam, alice, _removal) = namespace();
        let root = ContextGroupId::from(NS);

        // Alice removes the owner while the owner, concurrently, adds Zed.
        let removes_owner = gov(
            ALICE,
            &[&alice],
            OpPayload::MemberRemoved {
                group: root,
                member: acct(OWNER),
            },
        );
        let by_owner = add(OWNER, &[&alice], ZED, GroupMemberRole::Member);
        let heads = [removes_owner.id(), by_owner.id()];
        let ops = || vec![alice.clone(), removes_owner.clone(), by_owner.clone()];
        let zed_is_member = |proj: &ScopeProjections| {
            proj.acl_view_at(&ScopeId::from(NS), &heads)
                .expect("fed")
                .groups
                .get(&root)
                .is_some_and(|members| members.contains_key(&acct(ZED)))
        };

        let mut with_base = ScopeProjections::new();
        with_base.apply_backfill_with_base(&store, NS, ops());
        assert!(zed_is_member(&with_base), "the owner's ops are never void");

        let mut without = ScopeProjections::new();
        without.apply_backfill(NS, ops());
        assert!(
            !zed_is_member(&without),
            "a projection fed without the store's facts does not know the owner"
        );
    }

    #[test]
    fn the_void_judge_answers_void_questions_and_leaves_every_gate_to_live() {
        let (store, sam, _alice, removal) = namespace();
        let root = ContextGroupId::from(NS);
        let judge = VoidJudge::new(&store);

        let from_the_old_cut = add(SAM, &[&sam], XAVIER, GroupMemberRole::Admin);
        assert_eq!(
            judge.op_is_void(&root, 0, &from_the_old_cut),
            Some(true),
            "judged against the projection"
        );
        assert!(judge
            .voided_ops(&root, Some(&from_the_old_cut), &[])
            .is_some());
        let signer = PublicKey::from([ALICE; 32]);
        assert_eq!(
            judge.is_admin_at_cut(&root, &signer, &[removal.id()]),
            None,
            "a gate is live's to answer"
        );
    }
}
