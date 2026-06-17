//! Substrate for the unified causal log: convert the live apply stream into
//! [`Op`]s and maintain one [`ScopeState`] projection per scope.
//!
//! This is **additive** — nothing routes a decision or a convergence check
//! through it yet. It is the building block the apply paths feed while the
//! unified op-log is brought up alongside the separate data / governance /
//! rotation stores: maintain one projection per scope, and (later) derive its
//! convergence root by folding the projection's ACL + governance hashes onto
//! the storage layer's existing Merkle entities root, so a hash-neutral
//! writer/membership rotation moves the root.
//!
//! What feeds it, and why: the projection's value is the **ACL + governance**
//! planes (the data plane's entities come from the storage Merkle, so feeding
//! `Put`/`Delete` here would only duplicate state in memory). This module
//! starts with the **governance** plane — the cleartext namespace ops, whose
//! `signer` is a deterministic cross-node author and whose target scope is
//! resolvable from the op — applied at the namespace governance handler.

use std::collections::HashMap;

use calimero_governance_types::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_op_adapter::{payload_from_action, payload_from_root_op, set_writers_payload};
use calimero_primitives::identity::PublicKey;
use calimero_projection::ScopeState;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_storage::rotation_log::RotationLogEntry;

/// Build an [`Op`] from its scope, author, and causal coordinates.
fn build_op(
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    payload: OpPayload,
) -> Op {
    let id = Op::compute_id(scope, parents, &author, &hlc, &payload);
    Op {
        id,
        scope,
        parents: parents.to_vec(),
        author,
        hlc,
        payload,
        expected_scope_root: [0u8; 32],
        signature: [0u8; 64],
    }
}

/// Convert one data delta's worth of storage [`Action`]s into the unified
/// [`Op`]s representing the same writes, all sharing the delta's causal
/// coordinates. One op per state-changing action (`Action::Compare` is a sync
/// hint, dropped). Not fed into the live projection yet (the data plane's
/// entities come from the storage Merkle); kept for the eventual data-plane
/// unification.
#[must_use]
pub fn actions_to_ops(
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    actions: &[Action],
) -> Vec<Op> {
    actions
        .iter()
        .filter_map(|action| {
            payload_from_action(action)
                .map(|payload| build_op(scope, author, hlc, parents, payload))
        })
        .collect()
}

/// The scope a [`RootOp`] belongs to, or `None` if it isn't fed into the
/// projection yet.
///
/// - membership joins land in the **target group's** scope (the group id is
///   explicit in the op / its admin-signed invitation);
/// - namespace admin / policy changes land in the **namespace-root** scope;
/// - structural scope-tree ops (`GroupCreated` / `GroupReparented` /
///   `GroupDeleted`) and key transport (`KeyDelivery`) are deferred — placing a
///   subgroup-tree edit in the right scope is a later step, and key delivery
///   isn't authorization state.
fn scope_for_root_op(op: &RootOp, namespace_id: [u8; 32]) -> Option<ScopeId> {
    match op {
        RootOp::MemberJoinedOpen { group_id, .. } => Some(ScopeId::from(*group_id)),
        RootOp::MemberJoined {
            signed_invitation, ..
        } => Some(ScopeId::from(
            signed_invitation.invitation.group_id.to_bytes(),
        )),
        RootOp::AdminChanged { .. } | RootOp::PolicyUpdated { .. } => {
            Some(ScopeId::from(namespace_id))
        }
        _ => None,
    }
}

/// Convert a writer-set rotation ([`RotationLogEntry`]) into the unified
/// `SetWriters` [`Op`] for `object` in `scope`, or `None` for an unsigned
/// bootstrap entry (no author to attribute it to — those are skipped exactly as
/// the rotation-log append path skips them).
///
/// The author is the rotation's `signer` (deterministic across nodes) and the
/// hlc is the rotation's `delta_hlc`. Parents are left empty: the rotation log
/// is a per-object sequence resolved by `(hlc, signer)` today, and the
/// projection's per-object `(hlc, op_id)` LWW reproduces that ordering without
/// needing the causal edges (the equivalence is covered by
/// `op-adapter::acl_plane_matches_resolve_local_*`).
///
/// This is the ACL-plane **conversion**; feeding it from the live apply stream
/// is a later step — the raw rotation entries are produced in the storage
/// layer, below the projection, so the independent feed needs storage to
/// surface applied rotations rather than re-deriving them from the resolver.
#[must_use]
pub fn op_from_rotation_entry(object: Id, scope: ScopeId, entry: &RotationLogEntry) -> Option<Op> {
    let author = entry.signer?;
    let payload = set_writers_payload(object, entry);
    Some(build_op(scope, author, entry.delta_hlc, &[], payload))
}

/// Convert a cleartext namespace governance op into the unified [`Op`] for the
/// scope it affects, or `None` if it isn't represented in the projection yet.
///
/// The author is the op's `signer` (the same identity on every node, so the
/// content-addressed `op_id` — and therefore the projection's LWW order — is
/// deterministic across the cluster). `hlc` and `parents` come from the delta
/// the op rides. Encrypted group-scoped ops (`NamespaceOp::Group`) are not
/// represented: their payload is unreadable without the group key.
#[must_use]
pub fn op_from_signed_namespace_op(
    signed: &SignedNamespaceOp,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
) -> Option<Op> {
    let NamespaceOp::Root(root) = &signed.op else {
        return None;
    };
    let scope = scope_for_root_op(root, signed.namespace_id)?;
    let payload = payload_from_root_op(root)?;
    Some(build_op(scope, signed.signer, hlc, parents, payload))
}

/// In-memory registry of unified-op [`ScopeState`] projections, keyed by
/// [`ScopeId`].
///
/// Keyed by **scope**, not context: a scope is the unit of convergence — a
/// context's data lives in its own scope, a group's membership in the group's
/// scope, and authorization for a context resolves against its group's scope.
/// The apply paths feed each op into the scope it belongs to.
///
/// Unbounded for now: only governance ops feed it today, so growth tracks the
/// (small) number of live scopes; eviction (gated like the other per-context
/// caches) and persistence come with the broader wiring.
#[derive(Debug, Default)]
pub struct ScopeProjections {
    states: HashMap<ScopeId, ScopeState>,
}

impl ScopeProjections {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold `ops` into `scope`'s projection (creating it if absent). Apply is
    /// per-slot last-writer-wins, so the order ops are ingested doesn't matter.
    pub fn ingest<'a>(&mut self, scope: ScopeId, ops: impl IntoIterator<Item = &'a Op>) {
        let state = self.states.entry(scope).or_default();
        for op in ops {
            state.apply(op);
        }
    }

    /// Fold a single op into its own scope's projection.
    pub fn ingest_op(&mut self, op: &Op) {
        self.states.entry(op.scope).or_default().apply(op);
    }

    /// `scope`'s convergence root: the projection's ACL + governance folded onto
    /// the supplied storage Merkle `entities_root`. `None` if `scope` has no
    /// projection yet. `entities_root` MUST be the storage layer's Merkle root
    /// (see [`ScopeState::scope_root_with_entities`]).
    #[must_use]
    pub fn scope_root(&self, scope: &ScopeId, entities_root: [u8; 32]) -> Option<[u8; 32]> {
        self.states
            .get(scope)
            .map(|state| state.scope_root_with_entities(entities_root))
    }

    /// Read-only access to a scope's projection (for shadow comparison /
    /// authorization once more of the apply path feeds this).
    #[must_use]
    pub fn get(&self, scope: &ScopeId) -> Option<&ScopeState> {
        self.states.get(scope)
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU128;

    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation,
    };
    use calimero_op::OpPayload;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_storage::entities::{Metadata, OpMask};
    use calimero_storage::logical_clock::{Timestamp, ID, NTP64};

    use super::*;

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    fn signed_root(namespace_id: [u8; 32], signer: PublicKey, op: RootOp) -> SignedNamespaceOp {
        SignedNamespaceOp {
            version: 1,
            namespace_id,
            parent_op_hashes: Vec::new(),
            state_hash: [0u8; 32],
            signer,
            nonce: 0,
            op: NamespaceOp::Root(op),
            signature: [0u8; 64],
        }
    }

    #[test]
    fn actions_convert_to_matching_ops() {
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let e1 = Id::new([0xA1; 32]);
        let actions = vec![
            Action::Add {
                id: e1,
                data: vec![1, 2, 3],
                ancestors: Vec::new(),
                metadata: Metadata::default(),
            },
            Action::Compare { id: e1 },
        ];
        let ops = actions_to_ops(scope, author, hlc(10), &[], &actions);
        assert_eq!(ops.len(), 1, "Compare is dropped");
        assert_eq!(
            ops[0].payload,
            OpPayload::Put {
                entity: e1,
                value: vec![1, 2, 3]
            }
        );
    }

    #[test]
    fn namespace_op_open_join_maps_to_group_scope() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = [0x33; 32];

        let op = op_from_signed_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoinedOpen {
                    member,
                    group_id: group,
                },
            ),
            hlc(10),
            &[],
        )
        .expect("open-join is in-model");

        assert_eq!(
            op.scope,
            ScopeId::from(group),
            "join lands in the group scope"
        );
        assert_eq!(op.author, signer, "author is the op signer (deterministic)");
        assert_eq!(
            op.payload,
            OpPayload::MemberAdded {
                group: ContextGroupId::from(group),
                member,
                role: GroupMemberRole::Member,
            }
        );
    }

    #[test]
    fn namespace_op_invitation_join_decodes_group_and_role() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = ContextGroupId::from([0x44; 32]);
        let signed_invitation = SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: [0xA1; 32].into(),
                group_id: group,
                expiration_timestamp: 1_700_000_000,
                secret_salt: [0x33; 32],
                invited_role: 0, // Admin
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: None,
            app_key: None,
        };

        let op = op_from_signed_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoined {
                    member,
                    signed_invitation,
                },
            ),
            hlc(10),
            &[],
        )
        .expect("invitation join is in-model");

        assert_eq!(op.scope, ScopeId::from(group.to_bytes()));
        assert_eq!(
            op.payload,
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Admin,
            }
        );
    }

    #[test]
    fn namespace_admin_change_maps_to_namespace_scope() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let op = op_from_signed_namespace_op(
            &signed_root(ns, signer, RootOp::AdminChanged { new_admin: signer }),
            hlc(10),
            &[],
        )
        .expect("admin change is in-model");
        assert_eq!(
            op.scope,
            ScopeId::from(ns),
            "admin change is on the namespace root scope"
        );
    }

    #[test]
    fn structural_scope_tree_op_is_deferred() {
        // A subgroup-create maps to a payload, but its scope-tree placement is
        // deferred, so it is not fed into the projection yet (scope gating).
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        assert!(op_from_signed_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::GroupCreated {
                    group_id: [0x33; 32],
                    parent_id: [0x22; 32],
                },
            ),
            hlc(10),
            &[],
        )
        .is_none());
    }

    #[test]
    fn rotation_entry_maps_to_set_writers_op() {
        let scope = ScopeId::from([0u8; 32]);
        let object = Id::new([0xB0; 32]);
        let signer = PublicKey::from([1u8; 32]);
        let writer = PublicKey::from([9u8; 32]);
        let writers: std::collections::BTreeMap<PublicKey, OpMask> =
            [(writer, OpMask::FULL)].into_iter().collect();

        let entry = RotationLogEntry {
            delta_id: [7u8; 32],
            delta_hlc: hlc(5),
            signer: Some(signer),
            signature: None,
            signed_payload: None,
            new_writers: writers.clone(),
            writers_nonce: 1,
        };

        let op = op_from_rotation_entry(object, scope, &entry).expect("signed rotation maps");
        assert_eq!(op.author, signer, "author is the rotation signer");
        assert_eq!(op.hlc, hlc(5));
        assert_eq!(op.payload, OpPayload::SetWriters { object, writers });

        // Unsigned bootstrap entries have no author and are skipped.
        let unsigned = RotationLogEntry {
            signer: None,
            ..entry
        };
        assert!(op_from_rotation_entry(object, scope, &unsigned).is_none());
    }

    #[test]
    fn registry_is_per_scope_and_membership_moves_the_root() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = [0x33; 32];
        let storage_root = [0x42u8; 32];

        let join = op_from_signed_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoinedOpen {
                    member,
                    group_id: group,
                },
            ),
            hlc(10),
            &[],
        )
        .unwrap();

        let mut reg = ScopeProjections::new();
        // Empty group scope first, to compare.
        assert!(reg
            .scope_root(&ScopeId::from(group), storage_root)
            .is_none());
        reg.ingest_op(&join);
        let after = reg
            .scope_root(&ScopeId::from(group), storage_root)
            .expect("group scope present after join");
        // The empty-projection root over the same storage root, for contrast.
        let empty = ScopeState::default().scope_root_with_entities(storage_root);
        assert_ne!(after, empty, "a membership op moves the scope root");
    }
}
