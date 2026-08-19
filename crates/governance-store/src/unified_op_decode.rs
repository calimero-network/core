//! Decode a `SignedNamespaceOp` (+ its decrypted `GroupOp`) into a unified
//! [`Op`] for the causal log — the shared decode the apply path, the projection
//! backfill, and the atomic op-store write all route through.
//!
//! This lives in `governance-store` (not in `calimero-context`) so the governance
//! apply itself can build the decoded op and persist it to the unified op-store on
//! the SAME store handle as the gov-DAG write, making the two writes atomic (the
//! op-store can never lag the gov-DAG). `calimero-context` re-exports these so the
//! existing projection callers keep compiling unchanged.

use calimero_account::{AccountId, DeviceId};
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_dag::CausalDelta;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_op_adapter::{payload_from_group_op, payload_from_root_op};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;

use calimero_context_client::local_governance::GroupOp;

/// The account and device `sign_pk` speaks for in `namespace`, as the live
/// bindings record it.
///
/// The one resolution every producer of a projected op should use, so they cannot
/// disagree about who authored it. Returns `None` for a key with no live binding —
/// a joiner whose own join is what creates one, or pre-credential data — and the
/// caller then gets the stand-in rather than a fabricated claim to a real account.
///
/// **For one op.** A producer attributing *many* ops in the same namespace should
/// build [`signer_bindings_in`] once and read that instead — this resolves against
/// a fresh scan of the binding column every call.
pub fn signer_binding_for(
    store: &calimero_store::Store,
    namespace: &calimero_context_config::types::ContextGroupId,
    sign_pk: &PublicKey,
) -> Option<(AccountId, DeviceId)> {
    crate::AccountBindingRepository::new(store)
        .binding_for_sign_pk(namespace, sign_pk)
        .ok()
        .flatten()
        .map(|binding| (binding.account, binding.device))
}

/// Every signing key `namespace` can attribute an op to, mapped to the account and
/// device it speaks for.
///
/// Look a signer up with `bindings.get(&sign_pk).copied()`; an absent key means
/// exactly what [`signer_binding_for`]'s `None` means, so a caller reading this map
/// reaches the same stand-in for the same ops.
pub type SignerBindings = std::collections::BTreeMap<PublicKey, (AccountId, DeviceId)>;
/// [`signer_binding_for`] for every signer at once, in a single scan.
///
/// Hoist this out of any loop that attributes more than one op: the per-op form
/// rescans the whole binding column each time, so *n* ops over a group with *d*
/// devices cost *n × d* reads for one answer set that does not change as the loop
/// runs.
///
/// An unreadable store yields an EMPTY map rather than an error, which degrades the
/// same way the per-op form's `.ok()` does — every signer resolves to the stand-in.
/// Callers here are shadow/backfill producers with no channel to fail into.
pub fn signer_bindings_in(
    store: &calimero_store::Store,
    namespace: &calimero_context_config::types::ContextGroupId,
) -> SignerBindings {
    crate::AccountBindingRepository::new(store)
        .live_bindings_by_sign_pk(namespace)
        .unwrap_or_default()
        .into_iter()
        .map(|(sign_pk, binding)| (sign_pk, (binding.account, binding.device)))
        .collect()
}

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
    authorship: Authorship,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    payload: OpPayload,
) -> Op {
    Op::from_parts(
        id,
        scope,
        parents.to_vec(),
        authorship,
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
/// The authorship an op **carries**, when it carries one.
///
/// A join or a device link names its own account and device in a root-signed
/// [`DeviceCert`]. Reading it here is what lets the projected op be attributed
/// to the principal that actually authored it, instead of to a stand-in derived
/// from the signing key — and a derived stand-in is why the
/// `MemberJoinedWithDevice` arm of `calimero_authz::authorize` cannot currently
/// run the two cross-checks its `DeviceLinked` sibling does.
///
/// The certificate is *read*, not trusted: `authorize` verifies it against the
/// account plane at the cut before any of this is believed. Reading it here
/// only decides who the op claims to be, which is exactly what that check then
/// confirms.
///
/// `None` for every other op — those carry no credential, and their author is
/// resolved from the folded view by `account_for_author` rather than from the
/// op itself.
fn carried_authorship(
    op: &NamespaceOp,
    decrypted_group_op: Option<&GroupOp>,
    signer: PublicKey,
) -> Option<Authorship> {
    let cert = match op {
        NamespaceOp::Root(
            RootOp::MemberJoined { account, .. }
            | RootOp::MemberJoinedOpen { account, .. }
            | RootOp::MemberJoinedAt { account, .. }
            | RootOp::NamespaceCreated { account, .. }
            | RootOp::MemberJoinedViaTeeAttestation { account, .. },
        ) => &account.cert,
        // A device link rides an ENCRYPTED group op, so its certificate is only
        // legible once the op decrypts. An undecryptable one folds to a `Noop`
        // and keeps the stand-in — it carries no readable claim to attribute to.
        NamespaceOp::Group { .. } => match decrypted_group_op? {
            GroupOp::AccountDeviceLinked { cert, .. } => cert,
            _ => return None,
        },
        _ => return None,
    };
    Some(Authorship {
        account: cert.account,
        device: cert.device,
        device_key: signer,
    })
}

#[must_use]
pub fn op_from_namespace_op(
    signed: &SignedNamespaceOp,
    decrypted_group_op: Option<&GroupOp>,
    id: [u8; 32],
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
) -> Op {
    op_from_namespace_op_with_binding(signed, decrypted_group_op, None, id, hlc, parents)
}

/// [`op_from_namespace_op`], told who the signer is.
///
/// Every production producer should use this one. The bare version attributes an
/// op with no carried credential to a stand-in derived from its signing key, and
/// a stand-in is not the account any row is keyed by — so a fold that compares
/// the two (`AccountKeysRotated` does) silently drops the op.
#[must_use]
pub fn op_from_namespace_op_with_binding(
    signed: &SignedNamespaceOp,
    decrypted_group_op: Option<&GroupOp>,
    signer_binding: Option<(AccountId, DeviceId)>,
    id: [u8; 32],
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
) -> Op {
    let payload = match &signed.op {
        // `MemberJoinedOpen` is an open-subgroup inheritance-join PROOF, not a
        // direct membership: live's apply requires `check_path == Inherited` and
        // writes NO persistent `GroupMember` row, re-deriving the membership from
        // the anchor each time (so it is revoked when the anchor's membership is
        // removed, and restored on rejoin). Folding its MEMBERSHIP as a direct
        // `MemberAdded` would make it permanent and survive anchor removal (the
        // over-grant); the inheritance walk in `AclView::is_member_at_cut` derives
        // it from the foldable anchor membership + visibility + cap (default cap
        // via base fact) instead, so it tracks the anchor both ways.
        //
        // Its CREDENTIAL is a different matter and is folded. The encoder returns
        // a bare `DeviceLinked` for exactly this op, so what used to be a
        // graph-only node now carries the device and nothing else — membership
        // still inherited, binding still recorded. Leaving it a `Noop` was what
        // let the apply path write a binding the projection never saw, which
        // re-keys the joiner's writer principal on one plane only.
        NamespaceOp::Root(root) => payload_from_root_op(root).unwrap_or(OpPayload::Noop),
        NamespaceOp::Group { group_id, .. } => decrypted_group_op
            .and_then(|g| payload_from_group_op(*group_id, g))
            .unwrap_or(OpPayload::Noop),
        // `NamespaceOp` is `#[non_exhaustive]`; an unknown future op folds as a
        // `Noop` graph node (same as an undecryptable/unfoldable op above),
        // preserving causal structure without inventing a payload.
        _ => OpPayload::Noop,
    };
    // Three answers, in the only order that can work.
    //
    // The op's own certificate first: a joiner has no binding yet — its join is
    // what creates one — so only the credential it carries can name it.
    //
    // Then the binding the CALLER resolved, which covers an established device
    // whose op carries no credential. `AccountKeysRotated` is that case, and
    // getting a stand-in there meant the fold compared a real `handoff.account`
    // against a derived one and silently dropped every rotation.
    //
    // And last, an explicit `unattributed` for a key nothing knows — a value no
    // genesis can produce, so every gate that compares the author against a real
    // principal fails closed without needing a case for it.
    //
    // Resolving the binding in the CALLER is what makes it safe. Both producers
    // have already APPLIED this op, and an op cannot apply unless the signer's
    // binding is present — so every node that folds it resolves the same account.
    // Resolving inside the fold would not be safe: the fold walks raw logs in
    // arrival order, so reading a binding there answers "has the link folded
    // yet" and splits the root by delivery order.
    let authorship = carried_authorship(&signed.op, decrypted_group_op, signed.signer)
        .or_else(|| {
            signer_binding.map(|(account, device)| Authorship {
                account,
                device,
                device_key: signed.signer,
            })
        })
        .unwrap_or_else(|| Authorship::unattributed(signed.signer));
    build_op(
        id,
        ScopeId::from(signed.namespace_id.to_bytes()),
        authorship,
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
    let delta = CausalDelta::new(
        delta_id,
        op.parent_op_hashes.clone(),
        op.clone(),
        HybridTimestamp::default(),
    );

    // Mark a structural root so the DAG will accept its empty parent list
    // (#3126). `DagStore::can_apply` treats no-parents-and-not-Genesis as
    // malformed, because `all()` over an empty list is vacuously true and such a
    // delta would otherwise apply instantly as a disconnected head, skipping
    // missing-parent detection.
    //
    // EVERY parentless op on this plane is marked, not just `NamespaceCreated`,
    // and that is not laziness — it is what the head convention forces.
    // `NamespaceGovernanceDag::read_head_record` returns an EMPTY `parent_hashes`
    // whenever no `NamespaceGovHead` is persisted, so an op signed by a node that
    // has not yet applied the namespace genesis is legitimately parentless while
    // being any variant at all. "Empty parents" therefore does not mean "genesis"
    // here; it means "signed against an empty local head", which a not-yet-caught-up
    // node does routinely. Narrowing this to `NamespaceCreated` strands those ops
    // and governance never converges — verified the hard way by the
    // `group-join-mesh-not-ready` e2e, whose whole premise is a node joining before
    // the mesh is ready.
    //
    // So the guard's real reach is the STATE plane, where the convention is
    // unambiguous: that write path spells genesis as the `[0; 32]` sentinel and its
    // receivers always build `DeltaKind::Regular`, so a parentless state delta is
    // always malformed and is now rejected. That is also where the guard matters
    // most — a state delta's parents are only bound by the content address, so
    // stripping them was a live tampering vector (#3540), whereas a governance op's
    // `parent_op_hashes` sit inside its signed content and cannot be altered by a
    // peer at all. What remains open on this plane is a buggy or malicious
    // *authorized signer*, which no structural check can catch.
    //
    // Closing that too means making a node with no head sign against an explicit
    // `[0; 32]` root instead of an empty list — a wire change that also moves the
    // founder-gate signal in `namespace_created.rs`, which is the trade #3126
    // proposed and this PR declines. It stays open, deliberately and now visibly.
    //
    // No authority is granted either way: `parent_op_hashes` being empty remains
    // the founder-gate signal it always was, checked before the signer per #596,
    // and a forged `NamespaceCreated` is still stopped there. This kind is a
    // structural statement only, derived locally from signed content rather than
    // read off the wire.
    if delta.parents.is_empty() {
        return Ok(delta.into_genesis());
    }

    Ok(delta)
}
