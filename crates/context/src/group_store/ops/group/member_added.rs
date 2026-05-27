//! `GroupOp::MemberAdded` apply handler. Extracted from
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
    role: &GroupMemberRole,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    if *role == GroupMemberRole::ReadOnlyTee {
        bail!(MembershipError::ReadOnlyTeeViaAttestationOnly);
    }
    ctx.permissions
        .require_manage_members(signer, "add member")?;
    ctx.permissions.require_admin_to_add_admin(signer, role)?;
    MembershipRepository::new(store).add_member(group_id, member, role.clone())?;
    // Clear any stale deny-list entry — re-adding a previously
    // removed member transparently restores their network-level
    // access. Idempotent on a member who was never denied.
    DenyListRepository::new(store).clear(group_id, member)?;
    // Restore per-context `ContextIdentity` rows that
    // `cascade_remove_member_from_group_tree` deleted on a prior
    // `MemberRemoved`. The local-rejoiner anti-spoof gate is
    // enforced inside `restore_member_context_identities` — on
    // peers (admin or other members applying this op) it is a
    // no-op. Idempotent on first-time adds: the joiner's later
    // `join_context` sees an existing row and skips.
    restore_member_context_identities(store, group_id, member)?;
    crate::op_events::notify(crate::op_events::OpEvent::MemberAdded {
        group_id: group_id.to_bytes(),
        member: *member,
        role: role.clone(),
    });
    // #2422 Option 2: synthesize an `AutoFollowSet` event whenever
    // a freshly-written member row has `auto_follow.contexts` set
    // (the post-#2422 default). The auto-follow handler subscribes
    // to `AutoFollowSet` (not `MemberAdded`), so without this the
    // joiner would correctly auto-follow FUTURE
    // `OpEvent::ContextRegistered` events but never backfill
    // contexts that already existed in the group at join time —
    // which is the user-reported regression (Ronit/Fran 2026-05-20).
    // The handler short-circuits via `NotForSelf` on every node
    // except the joiner, so the cascade only fires once per
    // membership change.
    emit_auto_follow_set_if_enabled(store, group_id, member)?;
    Ok(())
}
