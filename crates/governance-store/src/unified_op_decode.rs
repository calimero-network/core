//! Decode a `SignedNamespaceOp` (+ its decrypted `GroupOp`) into a unified
//! [`Op`] for the causal log — the shared decode the apply path, the projection
//! backfill, and the atomic op-store write all route through.
//!
//! This lives in `governance-store` (not in `calimero-context`) so the governance
//! apply itself can build the decoded op and persist it to the unified op-store on
//! the SAME store handle as the gov-DAG write, making the two writes atomic (the
//! op-store can never lag the gov-DAG). `calimero-context` re-exports these so the
//! existing projection callers keep compiling unchanged.

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_dag::CausalDelta;
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_op_adapter::{payload_from_group_op, payload_from_root_op};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;

use calimero_context_client::local_governance::GroupOp;

/// Assemble an [`Op`] that **mirrors a source-DAG op**: its `id` and `parents`
/// are the source delta's own id/parents, *not* a fresh [`Op::compute_id`]. This
/// is deliberate — it makes the projection's op graph share an id space with the
/// source DAGs, so a live decision's cut (e.g. a delta's `governance_dag_heads`,
/// which are governance-op ids) maps directly onto the projection and
/// `ScopeProjections::acl_view_at` resolves the same ancestry the source DAG
/// would. The source ids are themselves content-addressed + identical on every
/// node, so the projection's `(hlc, op_id)` LWW stays deterministic.
fn build_op(
    id: [u8; 32],
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    payload: OpPayload,
) -> Op {
    Op::from_parts(
        id,
        scope,
        parents.to_vec(),
        author,
        hlc,
        payload,
        [0u8; 32],
        [0u8; 64],
    )
}

/// Convert a namespace governance op into the unified [`Op`] graph node it
/// occupies — **always** a node, never `None`: membership ops carry their
/// payload, and every other op (non-membership Root op, encrypted/undecryptable
/// Group op, key transport) folds to [`OpPayload::Noop`]. The node MUST still
/// exist so an ancestry walk can traverse *through* it; dropping it would
/// truncate the walk and orphan every membership op behind it.
///
/// Governance ops are keyed under the **namespace** scope, not per-group. The
/// live system keeps ONE governance DAG per namespace and a data write cites
/// namespace-wide `governance_dag_heads`, so membership has to resolve over the
/// whole namespace ancestry (a per-group log truncates the walk at the first
/// cross-scope node — that was the bug). Membership for a specific group is read
/// out of the folded view's `groups[group]`; the per-scope-DAG split is a
/// post-cutover concern.
///
/// `id`/`hlc`/`parents` are the governance **delta's own** id, hlc, and parents
/// (its `parent_op_hashes`) so the projection mirrors the governance DAG and the
/// cut maps onto it (see [`build_op`]). `decrypted_group_op` is the cleartext
/// `GroupOp` for a `NamespaceOp::Group` (via
/// [`crate::decrypt_group_op`]), or `None` when it couldn't be decrypted — in
/// which case the node is still recorded as `Noop`.
#[must_use]
pub fn op_from_namespace_op(
    signed: &SignedNamespaceOp,
    decrypted_group_op: Option<&GroupOp>,
    id: [u8; 32],
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
) -> Op {
    let payload = match &signed.op {
        // `MemberJoinedOpen` is an open-subgroup inheritance-join PROOF, not a
        // direct membership: live's apply requires `check_path == Inherited` and
        // writes NO persistent `GroupMember` row, re-deriving the membership from
        // the anchor each time (so it is revoked when the anchor's membership is
        // removed, and restored on rejoin). Folding it as a direct `MemberAdded`
        // would make it permanent and survive anchor removal (the over-grant). Fold
        // it as a `Noop` graph node; the inheritance walk in
        // `AclView::is_member_at_cut` derives the membership from the foldable
        // anchor membership + visibility + cap (default cap via base fact), so it
        // tracks the anchor both ways.
        NamespaceOp::Root(RootOp::MemberJoinedOpen { .. }) => OpPayload::Noop,
        NamespaceOp::Root(root) => {
            payload_from_root_op(root, signed.signer).unwrap_or(OpPayload::Noop)
        }
        NamespaceOp::Group { group_id, .. } => decrypted_group_op
            .and_then(|g| payload_from_group_op(*group_id, g))
            .unwrap_or(OpPayload::Noop),
        // `NamespaceOp` is `#[non_exhaustive]`; an unknown future op folds as a
        // `Noop` graph node (same as an undecryptable/unfoldable op above),
        // preserving causal structure without inventing a payload.
        _ => OpPayload::Noop,
    };
    build_op(
        id,
        ScopeId::from(signed.namespace_id.to_bytes()),
        signed.signer,
        hlc,
        parents,
        payload,
    )
}

/// Build a [`CausalDelta`] from a [`SignedNamespaceOp`] for insertion into the
/// namespace governance DAG.
///
/// Also the source of the `id`/`hlc`/`parents` coordinates
/// [`op_from_namespace_op`] needs to mirror the governance DAG.
pub fn signed_namespace_op_to_delta(
    op: &SignedNamespaceOp,
) -> Result<CausalDelta<SignedNamespaceOp>, eyre::Error> {
    let delta_id = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    Ok(CausalDelta::new(
        delta_id,
        op.parent_op_hashes.clone(),
        op.clone(),
        HybridTimestamp::default(),
        // C5.S3b removed the op-level state_hash; the unified op-store delta has no
        // meaningful `expected_root_hash` to carry from a governance op.
        [0u8; 32],
    ))
}
