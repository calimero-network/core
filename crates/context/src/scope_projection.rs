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

use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_op_adapter::{payload_from_root_op, set_writers_payload};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_projection::ScopeState;
use calimero_storage::address::Id;
use calimero_storage::collections::decode_rotation_log_entry_child;
use calimero_storage::index::EntityIndex;
use calimero_storage::interface::Interface;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_storage::rotation_log::{RotationLog, RotationLogEntry};
use calimero_storage::store::{Key as StorageKey, MainStorage};
use calimero_store::key::ContextState;

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

/// Read a Shared anchor's rotation log directly from the datastore (no WASM
/// storage env), by walking its hashed-collection children. `None` if the
/// anchor has no rotation collection yet (never rotated / not a Shared anchor).
///
/// This is the post-apply read the live ACL feed uses to recover the **raw**
/// rotation entries (with their signer), the independent source the projection
/// folds — as opposed to the resolver's already-merged output.
///
/// TODO(consolidate): mirrors `load_rotation_log_direct` in
/// `crates/node/src/delta_store.rs`; both should share one Store-backed reader
/// in `calimero-storage` before this becomes authoritative at cutover.
#[must_use]
pub fn load_rotation_log_direct(
    client: &ContextClient,
    context_id: ContextId,
    anchor: Id,
) -> Option<RotationLog> {
    let map_id = Interface::<MainStorage>::rotation_log_child_id(anchor);
    let handle = client.datastore_handle();
    // Read a state value. A store error is surfaced as a warning (distinct from
    // a legitimately-absent key) so a transient I/O fault doesn't silently make
    // the ACL shadow feed skip an anchor's rotations.
    let read = |key: StorageKey| -> Option<Vec<u8>> {
        let state_key = ContextState::new(context_id, key.to_bytes());
        match handle.get(&state_key) {
            Ok(state) => state.map(|s| s.value.into_boxed().into_vec()),
            Err(err) => {
                tracing::warn!(
                    %context_id, anchor = ?anchor, %err,
                    "rotation-log read failed; ACL shadow feed skips this anchor"
                );
                None
            }
        }
    };

    let index = borsh::from_slice::<EntityIndex>(&read(StorageKey::Index(map_id))?).ok()?;
    let mut entries = Vec::new();
    if let Some(children) = index.children() {
        for child in children {
            let Some(bytes) = read(StorageKey::Entry(child.id())) else {
                continue;
            };
            match decode_rotation_log_entry_child(&bytes) {
                Some(entry) => entries.push(entry),
                // A child that doesn't decode yields a partial log; warn rather
                // than skip in silence (matches the node-crate reader).
                None => tracing::warn!(
                    %context_id, anchor = ?anchor, child = ?child.id(),
                    "rotation-log child failed to decode; ACL shadow feed may be incomplete"
                ),
            }
        }
    }
    // Canonical order; the caller filters to this delta's id, so ordering is not
    // load-bearing here (it mirrors the node-crate reader's shape).
    entries.sort_by(|a, b| a.delta_id.cmp(&b.delta_id));
    Some(RotationLog {
        snapshot: None,
        entries,
    })
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
/// Unbounded for now: only governance/ACL ops feed it today, so growth tracks
/// the (small) number of live scopes and their op history; eviction (gated like
/// the other per-context caches) and persistence come with the broader wiring.
#[derive(Debug, Default)]
pub struct ScopeProjections {
    /// Folded current state per scope — the fast path for current-view reads.
    states: HashMap<ScopeId, ScopeState>,
    /// Retained op-log per scope, enabling causal-cut resolution
    /// ([`Self::acl_view_at`]) — the **causal-honor** view the cutover's
    /// `authorize` decides against (the state as of an op's own parents, never
    /// the receiver's current state). Grows with governance/ACL ops; bounding
    /// lands with the cutover.
    logs: HashMap<ScopeId, Vec<Op>>,
}

impl ScopeProjections {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a single op into its own scope's projection and op-log.
    pub fn ingest_op(&mut self, op: &Op) {
        self.states.entry(op.scope).or_default().apply(op);
        self.logs.entry(op.scope).or_default().push(op.clone());
    }

    /// The **causal-honor** authorization view of `scope` at the cut named by
    /// `parents`: fold only the ops in the ancestry of `parents` (never ops
    /// after the cut), over the retained op-log. `None` if `scope` hasn't been
    /// fed. This is the view the cutover's `authorize` decides against — the
    /// projection's answer to "was this author permitted *as of its own causal
    /// position*", independent of what the receiver has since applied.
    #[must_use]
    pub fn acl_view_at(
        &self,
        scope: &ScopeId,
        parents: &[[u8; 32]],
    ) -> Option<calimero_authz::AclView> {
        let log = self.logs.get(scope)?;
        Some(ScopeState::acl_view_at(log, parents))
    }

    /// The role the projection records for `member` in `group` within `scope`,
    /// or `None` if absent (member not present, or the scope hasn't been fed).
    /// Used by the shadow-compare to check a freshly-applied membership op
    /// against the live resolver, one member at a time.
    #[must_use]
    pub fn role_of(
        &self,
        scope: &ScopeId,
        group: &ContextGroupId,
        member: &PublicKey,
    ) -> Option<GroupMemberRole> {
        self.states
            .get(scope)?
            .acl_view()
            .groups
            .get(group)?
            .get(member)
            .cloned()
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
    use calimero_storage::entities::OpMask;
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
    fn acl_view_at_honors_the_causal_cut_over_the_retained_log() {
        let scope = ScopeId::from([0u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let admin = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);

        let build = |ns: u64, parents: Vec<[u8; 32]>, payload: OpPayload| -> Op {
            let h = hlc(ns);
            let id = Op::compute_id(scope, &parents, &admin, &h, &payload);
            Op {
                id,
                scope,
                parents,
                author: admin,
                hlc: h,
                payload,
                expected_scope_root: [0u8; 32],
                signature: [0u8; 64],
            }
        };

        let add = build(
            10,
            vec![],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let remove = build(20, vec![add.id], OpPayload::MemberRemoved { group, member });

        let mut reg = ScopeProjections::new();
        reg.ingest_op(&add);
        reg.ingest_op(&remove);

        // Cut at the add (pre-removal ancestry): the member is present — a write
        // authored here stays authorized even though we've since seen the remove.
        let pre = reg.acl_view_at(&scope, &[add.id]).expect("scope fed");
        assert_eq!(
            pre.groups.get(&group).and_then(|m| m.get(&member)),
            Some(&GroupMemberRole::Member),
        );

        // Cut at the remove (its ancestry includes both): the member is gone.
        let post = reg.acl_view_at(&scope, &[remove.id]).expect("scope fed");
        assert_eq!(post.groups.get(&group).and_then(|m| m.get(&member)), None);

        // Unknown scope ⇒ no view.
        assert!(reg
            .acl_view_at(&ScopeId::from([0xEE; 32]), &[add.id])
            .is_none());
    }

    #[test]
    fn registry_records_membership_per_scope() {
        let ns = [0x11; 32];
        let signer = PublicKey::from([1u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let group = ContextGroupId::from([0x33; 32]);
        let other_scope = ScopeId::from([0xEE; 32]);

        let join = op_from_signed_namespace_op(
            &signed_root(
                ns,
                signer,
                RootOp::MemberJoinedOpen {
                    member,
                    group_id: group.to_bytes(),
                },
            ),
            hlc(10),
            &[],
        )
        .unwrap();

        let mut reg = ScopeProjections::new();
        // Before the join: no projection for the group scope.
        assert_eq!(reg.role_of(&join.scope, &group, &member), None);
        reg.ingest_op(&join);
        // After: the member is recorded in the group's scope...
        assert_eq!(
            reg.role_of(&join.scope, &group, &member),
            Some(GroupMemberRole::Member),
        );
        // ...and only that scope (isolation).
        assert_eq!(reg.role_of(&other_scope, &group, &member), None);
    }
}
