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
mod group_created;
mod group_deleted;
mod group_reparented;
mod member_joined;
mod member_joined_open;
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
        } => group_created::apply(ctx, op, *group_id, *parent_id, *restricted),
        RootOp::GroupDeleted {
            root_group_id,
            cascade_group_ids,
            cascade_context_ids,
        } => group_deleted::apply(
            ctx,
            op,
            *root_group_id,
            cascade_group_ids,
            cascade_context_ids,
        ),
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } => group_reparented::apply(ctx, op, *child_group_id, *new_parent_id),
        RootOp::AdminChanged { new_admin } => admin_changed::apply(ctx, op, *new_admin),
        RootOp::PolicyUpdated { .. } => policy_updated::apply(ctx, op),
        RootOp::MemberJoined {
            member,
            signed_invitation,
        } => member_joined::apply(ctx, op, member, signed_invitation, None),
        RootOp::MemberJoinedAt {
            member,
            signed_invitation,
            joined_at,
        } => member_joined::apply(ctx, op, member, signed_invitation, Some(*joined_at)),
        RootOp::MemberJoinedOpen { member, group_id } => {
            member_joined_open::apply(ctx, op, *member, *group_id)
        }
        // Self-authorizing namespace genesis. SECURITY residual (#2932): a
        // self-consistent forged genesis on a BARE namespace is not blocked here
        // — see the SECURITY note in `namespace_created::apply`.
        RootOp::NamespaceCreated { founder } => namespace_created::apply(ctx, op, *founder),
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
    }
}
