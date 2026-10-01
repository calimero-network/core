//! Per-op apply handlers for `RootOp` variants (#2481).
//!
//! Sibling of `ops/group` (#2304). Each variant of
//! `calimero_context_client::local_governance::RootOp` lives in its
//! own module under `ops/namespace/`, exposing a `pub(crate) fn
//! apply(ctx, …fields) -> EyreResult<()>`. The dispatcher
//! [`dispatch_root_op`] is a thin `match` that routes by variant —
//! moving the per-variant logic out of `NamespaceGovernance` into
//! reviewable per-op files.
//!
//! Side effects that the outer `apply_signed_op` orchestrates (the
//! `Group { encrypted, .. }` decrypt-and-apply flow) stay on
//! `NamespaceGovernance` because they need access to crate-internal
//! state the per-op handlers don't have a clean way to reach.

pub(crate) mod context;

mod admin_changed;
pub(crate) mod group_created;
mod group_deleted;
mod group_reparented;
mod member_joined;
pub(crate) mod member_joined_open;
mod member_joined_via_tee;
mod namespace_created;
mod policy_updated;

pub(crate) use context::NamespaceApplyCtx;

use calimero_context_client::local_governance::{RootOp, SignedNamespaceOp};
use eyre::Result as EyreResult;

/// Apply a `RootOp` against `ctx`. Thin router — variant-specific
/// logic lives in the per-op submodules.
pub(crate) fn dispatch_root_op(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    root: &RootOp,
) -> EyreResult<()> {
    match root {
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
            admin,
            salt,
        } => group_created::apply(
            ctx,
            op,
            group_id.to_bytes(),
            parent_id.to_bytes(),
            *restricted,
            *admin,
            salt,
        ),
        RootOp::GroupDeleted {
            root_group_id,
            cascade_group_ids,
            cascade_context_ids,
        } => {
            let cascade_group_ids: Vec<[u8; 32]> =
                cascade_group_ids.iter().map(|g| g.to_bytes()).collect();
            let cascade_context_ids: Vec<[u8; 32]> =
                cascade_context_ids.iter().map(|c| *c.digest()).collect();
            group_deleted::apply(
                ctx,
                op,
                root_group_id.to_bytes(),
                &cascade_group_ids,
                &cascade_context_ids,
            )
        }
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } => group_reparented::apply(ctx, op, child_group_id.to_bytes(), new_parent_id.to_bytes()),
        // Owner-level: only inside `RootGuarded`, with the account root's proof.
        RootOp::AdminChanged { .. } => Err(crate::OwnerGuardRefusal::ProofRequired {
            kind: "admin_changed",
        }
        .into()),
        RootOp::RootGuarded { op: inner, proof } => root_guarded(ctx, op, inner, proof),
        RootOp::PolicyUpdated { .. } => policy_updated::apply(ctx, op),
        RootOp::MemberJoinedViaTeeAttestation {
            group_id,
            member,
            quote_hash: _,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
            account,
        } => member_joined_via_tee::apply(
            ctx,
            op,
            *group_id,
            member,
            &crate::membership::TeeAttestationClaims {
                mrtd,
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                tcb_status,
            },
            role,
            account,
        ),
        // Both join arms read the endorsement off the ENVELOPE, not the op
        // body. It is not covered by the joiner's signature, which is what
        // lets an admitter attach consent to an op it did not author — see
        // `SignedNamespaceOp::admitter_endorsement`. Neither arm gets to
        // decide whether one is required; that is the gate's call, and it
        // requires one from both.
        RootOp::MemberJoined {
            member,
            signed_invitation,
            account,
        } => member_joined::apply(
            ctx,
            op,
            member,
            signed_invitation,
            None,
            account,
            op.admitter_endorsement.as_deref(),
        ),
        RootOp::MemberJoinedAt {
            member,
            signed_invitation,
            joined_at,
            account,
        } => member_joined::apply(
            ctx,
            op,
            member,
            signed_invitation,
            Some(*joined_at),
            account,
            op.admitter_endorsement.as_deref(),
        ),
        RootOp::MemberJoinedOpen {
            member,
            group_id,
            account,
        } => member_joined_open::apply(ctx, op, *member, group_id.to_bytes(), account),
        // Self-authorizing namespace genesis: the id must be the one
        // `(founder, salt)` derives, so it can only found its signer's own ids.
        RootOp::NamespaceCreatedV2 {
            founder,
            account,
            salt,
        } => namespace_created::apply(ctx, op, *founder, account, salt),
        // `KeyDelivery` has no state mutation of its own here: the actual
        // key-unwrap/store side effect is orchestrated by the outer
        // `apply_signed_op` match in `namespace/governance.rs`, which owns the
        // crate-internal state the per-op handlers can't reach. This arm is an
        // intentional no-op so the match can stay EXHAUSTIVE.
        RootOp::KeyDelivery { .. } => Ok(()),
        // `RootOp` is deliberately NOT `#[non_exhaustive]` (see its definition in
        // `calimero-governance-types`): the match is exhaustive so ADDING a
        // variant fails to compile here until it gets an explicit handler, rather
        // than silently no-op'ing while `apply_signed_op` still advances the DAG
        // head — which would drop the op from application fleet-wide. Do NOT add a
        // `_` wildcard.
        RootOp::OnBehalf {
            op: inner,
            delegation,
        } => on_behalf(ctx, op, inner, delegation),
    }
}

/// An owner-level root op carrying the account root's authorisation. The root
/// op sibling of `ops::group::root_guarded`; see [`crate::owner_guard`].
fn root_guarded(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    inner: &RootOp,
    proof: &calimero_account::SignedOwnerOp,
) -> EyreResult<()> {
    use crate::owner_guard::{advance_owner_op_counter, check_root_proof, GuardedOp};
    use crate::OwnerGuardRefusal;

    let (Some(kind), RootOp::AdminChanged { new_admin }) = (inner.owner_op_kind(), inner) else {
        eyre::bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: calimero_context_client::local_governance::NamespaceOp::Root(inner.clone())
                .op_kind_label(),
        });
    };
    if ctx.principal().is_some() {
        eyre::bail!(OwnerGuardRefusal::NotAGuardedKind {
            inner: "a delegated op",
        });
    }
    let namespace =
        calimero_context_config::types::ContextGroupId::from(ctx.namespace_id().to_bytes());
    let Some(account) = ctx
        .permissions_for(namespace)
        .account_for_signer(&op.signer)?
    else {
        eyre::bail!(OwnerGuardRefusal::SignerUnbound);
    };
    let guarded = GuardedOp {
        namespace,
        group: namespace,
        kind,
        digest: inner.owner_op_digest()?,
    };
    check_root_proof(ctx.store(), account, guarded, proof)?;
    admin_changed::apply(ctx, op, *new_admin, account)?;
    advance_owner_op_counter(ctx.store(), &namespace)
}

/// Seat the relay a member founded a namespace through, so it can serve it.
///
/// The genesis binds only the founder; the relay is in no row of a namespace
/// that did not exist a moment ago. So bind its device from the certificate the
/// delegation already carries (verified at the gate), seat it as a `Member`
/// holding `CAN_AUTHOR_ON_BEHALF` — its standing to act for members, and
/// nothing more — and record it as the founding relay, the one account that may
/// then admit itself as the namespace's first TEE (`FoundingRelayAttested`).
/// Part of the apply, so every replica seats it identically.
fn seat_founding_relay(
    store: &calimero_store::Store,
    namespace_group: &calimero_context_config::types::ContextGroupId,
    delegation: &calimero_account::GovernanceDelegation,
    warrant: &calimero_account::VerifiedGovernanceWarrant,
) -> EyreResult<()> {
    let relay = warrant.executor;
    let proof = &delegation.executor_proof;
    let bindings = crate::AccountBindingRepository::new(store);
    if let Err(rejected) = bindings.apply_link(
        namespace_group,
        &proof.genesis,
        &proof.chain,
        &proof.statement,
        crate::JOIN_SCOPE_EPOCH,
    )? {
        eyre::bail!("the founding relay's device credential is inadmissible: {rejected:?}");
    }
    let membership = crate::MembershipRepository::new(store);
    if membership.role_of(namespace_group, &relay)?.is_none() {
        membership.add_member(
            namespace_group,
            &relay,
            calimero_primitives::context::GroupMemberRole::Member,
        )?;
    }
    crate::CapabilitiesRepository::new(store).set_member_capability(
        namespace_group,
        &relay,
        calimero_context_config::MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
    )?;
    crate::NamespaceFoundingRepository::new(store).record_founding_relay(namespace_group, &relay)
}

/// Seat the relay that created a subgroup for a member in that subgroup, so it
/// can serve it.
///
/// Without this a relay creates a subgroup it is not in: it never learns the
/// contexts registered there, and every write the member sends through it is
/// refused. It is the executor the AUTHOR signed the warrant for, so the author
/// has already consented to exactly this relay acting for them here.
///
/// A relay that is a TEE at the namespace root is seated in the subgroup with
/// that same TEE role, and no capability row: a `RelayTee` relays by its role
/// (`warrant_gate::executor_standing`), so it needs no `CAN_AUTHOR_ON_BEHALF`.
/// This keeps TEE roles coming from attestation alone. The root row IS the
/// attestation verdict — minted only by `MemberJoinedViaTeeAttestation` or
/// `FoundingRelayAttested`, each verified on every peer at apply, and locked to
/// the TEE roles thereafter — and copying it into the subgroup is exactly what
/// the attestation fan-in (`tee_subgroup_admit`) would do. That fan-in cannot
/// seat THIS relay: it runs on the node holding the new subgroup's key, which is
/// the relay itself, and it may vouch in a `Restricted` subgroup only for a
/// member of it; in a namespace founded through a relay there is no admin node
/// to run it either. A `ReadOnlyTee` never reaches here — it may not relay, so
/// the delegation gate refused the op already.
///
/// Any other relay is seated as a `Member` holding `CAN_AUTHOR_ON_BEHALF` — its
/// standing to act for members, and nothing more. Part of the apply, so every
/// replica seats it identically.
fn seat_creating_relay(
    store: &calimero_store::Store,
    namespace_group: &calimero_context_config::types::ContextGroupId,
    subgroup: &calimero_context_config::types::ContextGroupId,
    warrant: &calimero_account::VerifiedGovernanceWarrant,
) -> EyreResult<()> {
    let membership = crate::MembershipRepository::new(store);
    let root_role = membership.role_of(namespace_group, &warrant.executor)?;
    if let Some(tee_role) = root_role.filter(|role| role.is_tee()) {
        if membership.role_of(subgroup, &warrant.executor)?.is_none() {
            membership.add_member(subgroup, &warrant.executor, tee_role)?;
        }
        return Ok(());
    }
    if membership.role_of(subgroup, &warrant.executor)?.is_none() {
        membership.add_member(
            subgroup,
            &warrant.executor,
            calimero_primitives::context::GroupMemberRole::Member,
        )?;
    }
    let caps = crate::CapabilitiesRepository::new(store);
    let held = caps
        .member_capability(subgroup, &warrant.executor)?
        .unwrap_or(0);
    caps.set_member_capability(
        subgroup,
        &warrant.executor,
        held | calimero_context_config::MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
    )?;
    Ok(())
}

/// A member's root op, published by a relay: admitted by
/// [`crate::delegation_gate`], then dispatched as if the author had signed it —
/// the same cut, the author's key as the signer, and the author's account as
/// the acting principal every gate resolves that key to.
fn on_behalf(
    ctx: &mut NamespaceApplyCtx<'_>,
    op: &SignedNamespaceOp,
    inner: &RootOp,
    delegation: &calimero_account::GovernanceDelegation,
) -> EyreResult<()> {
    let store = ctx.store();
    let namespace_group =
        calimero_context_config::types::ContextGroupId::from(ctx.namespace_id().to_bytes());
    let (warrant, principal) = crate::delegation_gate::check_root_delegation(
        store,
        &ctx.permissions_for(namespace_group),
        &namespace_group,
        &op.signer,
        inner,
        delegation,
    )?;

    if let RootOp::GroupCreated { group_id, .. } = inner {
        if crate::MetaRepository::new(store).load(group_id)?.is_some() {
            return Err(
                crate::delegation_gate::DelegationRefusal::GroupAlreadyExists(group_id.to_string())
                    .into(),
            );
        }
    }
    let mut as_author = op.clone();
    as_author.signer = principal.key;
    let (parents, authorizer) = ctx.apply_auth();
    let mut inner_ctx = NamespaceApplyCtx::new(store, ctx.namespace_id(), parents, authorizer)
        .with_principal(Some(principal));
    dispatch_root_op(&mut inner_ctx, &as_author, inner)?;
    if let RootOp::GroupCreated { group_id, .. } = inner {
        seat_creating_relay(
            store,
            &namespace_group,
            &group_id.to_bytes().into(),
            &warrant,
        )?;
    }
    if matches!(inner, RootOp::NamespaceCreatedV2 { .. }) {
        seat_founding_relay(store, &namespace_group, delegation, &warrant)?;
    }
    crate::delegation_gate::spend_delegation_nonce(store, &namespace_group, &warrant)?;

    for event in inner_ctx.take_events() {
        ctx.queue_event(event);
    }
    Ok(())
}
