//! `GroupOp::MemberJoinedViaTeeAttestation` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

#![allow(unused_imports)]

use super::super::super::contexts::restore_member_context_identities;
use super::super::super::{
    emit_auto_follow_set_if_enabled, now_millis, set_context_service_name,
    verify_post_apply_state_hashes,
};
use super::context::GroupApplyCtx;
use crate::group_store::{
    cascade_remove_member_from_group_tree, delete_group_local_rows, enumerate_group_contexts,
    get_group_for_context, MAX_NAMESPACE_DEPTH,
};
use crate::group_store::{
    ApplyError, CapabilitiesError, CapabilitiesRepository, ContextRegistrationError,
    ContextRegistrationService, DenyListRepository, GroupKeyring, GroupSettingsService,
    KeyringError, MembershipError, MembershipPolicy, MembershipRepository, MetaError,
    MetaRepository, MetadataRepository, MigrationsRepository, NamespaceError, NamespaceRepository,
    PermissionChecker, SigningKeysError, SigningKeysRepository, UpgradesRepository,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_primitives::metadata::{validate_metadata_payload, MetadataRecord};
use eyre::{bail, Result as EyreResult};
use std::collections::BTreeMap;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    member: &PublicKey,
    mrtd: &str,
    rtmr0: &str,
    rtmr1: &str,
    rtmr2: &str,
    rtmr3: &str,
    tcb_status: &str,
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    if *role != GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::TeeRoleMustBeReadOnly);
    }
    ctx.membership_policy
        .require_tee_attestation_verifier_membership(signer)?;
    let policy = ctx.membership_policy.read_required_tee_admission_policy()?;
    ctx.membership_policy.validate_tee_attestation_allowlists(
        &policy, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, tcb_status,
    )?;
    ctx.membership_policy.admit_member_if_absent(member, role)?;
    // Same rationale as `MemberAdded`: a TEE rejoining after a
    // prior removal should have their deny-list entry cleared.
    DenyListRepository::new(store).clear(group_id, member)?;
    crate::op_events::notify(crate::op_events::OpEvent::TeeMemberAdmitted {
        group_id: group_id.to_bytes(),
        member: *member,
    });
    // #2422 Option 2: TEE attestation goes through
    // `admit_member_if_absent` → `add_group_member`, which writes
    // the new default `{contexts: true, subgroups: false}`. The
    // fleet-join sidecar (`crates/server/src/admin/handlers/tee/
    // fleet_join.rs`) then issues an explicit `SetMemberAutoFollow
    // {true, true}` op, which fires its own `AutoFollowSet`. That
    // creates a second cascade — both join_context attempts are
    // idempotent (see auto_follow.rs:101-107), so the only cost
    // is two rate-limiter tokens. Documented and accepted.
    emit_auto_follow_set_if_enabled(store, group_id, member)?;
    Ok(())
}
