//! `GroupOp::MemberSetAutoFollow` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

#![allow(unused_imports)]

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
    target: &PublicKey,
    auto_follow_contexts: &bool,
    auto_follow_subgroups: &bool,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    // Admin-or-self: admin can toggle flags for any member, a
    // member can toggle their own. Non-admin, non-self attempts
    // are rejected.
    if !ctx.permissions.is_admin(signer)? && signer != target {
        bail!(MembershipError::AutoFollowAuthFailed);
    }
    // Target must already be a group member.
    if MembershipRepository::new(store)
        .role_of(group_id, target)?
        .is_none()
    {
        bail!(MembershipError::NotMember {
            group_id: format!("{group_id:?}"),
            identity: format!("{target:?}"),
        });
    }
    let flags = calimero_store::key::AutoFollowFlags {
        contexts: *auto_follow_contexts,
        subgroups: *auto_follow_subgroups,
    };
    MembershipRepository::new(store).set_auto_follow(group_id, target, flags)?;
    crate::op_events::notify(crate::op_events::OpEvent::AutoFollowSet {
        group_id: group_id.to_bytes(),
        member: *target,
        contexts: *auto_follow_contexts,
        subgroups: *auto_follow_subgroups,
    });
    Ok(())
}
