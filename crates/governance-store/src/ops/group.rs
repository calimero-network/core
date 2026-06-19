//! Per-op apply handlers for `GroupOp` variants (#2304).
//!
//! Each variant of `GroupOp` lives in its own module under
//! `ops/group/`, exposing a `pub(crate) fn apply(ctx, ...fields) ->
//! EyreResult<()>`. The dispatcher in this file [`dispatch`] is a
//! thin `match` that routes by variant — moving the giant
//! per-variant logic out of `mod.rs` into reviewable per-op files.
//!
//! Why plain functions and not a trait? Rust enum variants are
//! constructors, not types — `impl Trait for GroupOp::MemberAdded`
//! isn't legal. A trait approach would need a shadow struct per
//! variant just to satisfy the type system, with no run-time
//! benefit; plain `fn apply` matches the match-arm shape directly.

pub(crate) mod context;

mod cascade_group_migration_set;
mod cascade_target_application_set;
mod cascade_upgrade;
mod context_capability_granted;
mod context_capability_revoked;
mod context_detached;
mod context_metadata_set;
mod context_registered;
mod default_capabilities_set;
mod group_delete;
mod group_metadata_set;
mod group_migration_set;
mod member_added;
mod member_capability_set;
mod member_joined_via_tee_attestation;
mod member_left;
mod member_metadata_set;
mod member_removed;
mod member_role_set;
mod member_set_auto_follow;
mod noop;
mod subgroup_visibility_set;
mod target_application_set;
mod tee_admission_policy_set;
mod transfer_ownership;
mod upgrade_policy_set;

pub(crate) use context::GroupApplyCtx;

use calimero_context_client::local_governance::GroupOp;
use eyre::Result as EyreResult;

/// Apply a `GroupOp` against `ctx`. Returns `Ok(true)` if the variant
/// was recognized and applied; `Ok(false)` if the variant is one this
/// dispatcher does not handle (caller decides whether to error or
/// log).
///
/// This is a thin router — all variant-specific logic lives in the
/// per-op submodules.
pub(crate) fn dispatch(ctx: &mut GroupApplyCtx<'_>, op: &GroupOp) -> EyreResult<bool> {
    match op {
        GroupOp::Noop => noop::apply(ctx)?,
        GroupOp::MemberAdded { member, role } => member_added::apply(ctx, member, role)?,
        GroupOp::MemberRemoved {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        } => member_removed::apply(
            ctx,
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        )?,
        GroupOp::MemberLeft {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        } => member_left::apply(
            ctx,
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        )?,
        GroupOp::MemberRoleSet { member, role } => member_role_set::apply(ctx, member, role)?,
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => member_capability_set::apply(ctx, member, capabilities)?,
        GroupOp::DefaultCapabilitiesSet { capabilities } => {
            default_capabilities_set::apply(ctx, capabilities)?
        }
        GroupOp::UpgradePolicySet { policy } => upgrade_policy_set::apply(ctx, policy)?,
        GroupOp::TargetApplicationSet {
            app_key,
            target_application_id,
        } => target_application_set::apply(ctx, app_key, target_application_id)?,
        GroupOp::ContextRegistered {
            context_id,
            application_id,
            service_name,
            ..
        } => context_registered::apply(ctx, context_id, application_id, service_name)?,
        GroupOp::ContextDetached { context_id } => context_detached::apply(ctx, context_id)?,
        GroupOp::SubgroupVisibilitySet { mode } => subgroup_visibility_set::apply(ctx, mode)?,
        GroupOp::GroupMetadataSet { name, data } => group_metadata_set::apply(ctx, name, data)?,
        GroupOp::MemberMetadataSet { member, name, data } => {
            member_metadata_set::apply(ctx, member, name, data)?
        }
        GroupOp::ContextMetadataSet {
            context_id,
            name,
            data,
        } => context_metadata_set::apply(ctx, context_id, name, data)?,
        GroupOp::GroupDelete => group_delete::apply(ctx)?,
        GroupOp::GroupMigrationSet { migration } => group_migration_set::apply(ctx, migration)?,
        GroupOp::ContextCapabilityGranted {
            context_id,
            member,
            capability,
        } => context_capability_granted::apply(ctx, context_id, member, capability)?,
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member,
            capability,
        } => context_capability_revoked::apply(ctx, context_id, member, capability)?,
        GroupOp::TeeAdmissionPolicySet { .. } => tee_admission_policy_set::apply(ctx)?,
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash: _,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
        } => member_joined_via_tee_attestation::apply(
            ctx,
            member,
            &crate::membership::TeeAttestationClaims {
                mrtd: mrtd.as_str(),
                rtmr0: rtmr0.as_str(),
                rtmr1: rtmr1.as_str(),
                rtmr2: rtmr2.as_str(),
                rtmr3: rtmr3.as_str(),
                tcb_status: tcb_status.as_str(),
            },
            role,
        )?,
        GroupOp::MemberSetAutoFollow {
            target,
            auto_follow_contexts,
            auto_follow_subgroups,
        } => {
            member_set_auto_follow::apply(ctx, target, auto_follow_contexts, auto_follow_subgroups)?
        }
        GroupOp::TransferOwnership { new_owner } => transfer_ownership::apply(ctx, new_owner)?,
        // Deprecated legacy two-op cascade variants (see GroupOp docs):
        // superseded by `CascadeUpgrade`, but their apply arms are retained for
        // one release so in-flight / replayed ops from pre-upgrade peers still apply.
        GroupOp::CascadeTargetApplicationSet {
            from_app_key,
            app_key,
            target_application_id,
        } => cascade_target_application_set::apply(
            ctx,
            from_app_key,
            app_key,
            target_application_id,
        )?,
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => cascade_group_migration_set::apply(ctx, from_app_key, migration)?,
        GroupOp::CascadeUpgrade {
            from_app_key,
            app_key,
            target_application_id,
            migration,
            cascade_hlc,
        } => cascade_upgrade::apply(
            ctx,
            from_app_key,
            app_key,
            target_application_id,
            migration,
            *cascade_hlc,
        )?,
        // `GroupOp` is `#[non_exhaustive]` from a different crate,
        // so the wildcard is required by the compiler. When a new
        // variant is added in `calimero-governance-types`, it lands
        // here as `handled = false`, which the caller turns into
        // `ApplyError::UnsupportedOp` at runtime — not a compile
        // error. Reviewers should grep for `GroupOp::` in this file
        // when reviewing governance-types variant additions.
        _ => return Ok(false),
    }
    Ok(true)
}
