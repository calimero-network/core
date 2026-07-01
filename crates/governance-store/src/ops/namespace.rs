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
        } => group_created::apply(
            ctx,
            op,
            group_id.to_bytes(),
            parent_id.to_bytes(),
            *restricted,
        ),
        RootOp::GroupDeleted {
            root_group_id,
            cascade_group_ids,
            cascade_context_ids,
        } => {
            let cascade_group_ids: Vec<[u8; 32]> =
                cascade_group_ids.iter().map(|g| g.to_bytes()).collect();
            let cascade_context_ids: Vec<[u8; 32]> =
                cascade_context_ids.iter().map(|c| *c.as_ref()).collect();
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
            member_joined_open::apply(ctx, op, *member, group_id.to_bytes())
        }
        // Self-authorizing namespace genesis. SECURITY residual (#2932): a
        // self-consistent forged genesis on a BARE namespace is not blocked here
        // — see the SECURITY note in `namespace_created::apply`.
        RootOp::NamespaceCreated { founder } => namespace_created::apply(ctx, op, *founder),
        // `RootOp` is `#[non_exhaustive]` in `calimero-governance-types`,
        // so the wildcard is required at compile time. New variants land
        // here as `Ok(())` (silent no-op) until wired up explicitly —
        // reviewers must grep for `RootOp::` in this file when reviewing
        // governance-types variant additions.
        _ => Ok(()),
    }
}
