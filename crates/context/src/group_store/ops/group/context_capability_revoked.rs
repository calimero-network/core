//! `GroupOp::ContextCapabilityRevoked` apply handler. Extracted from
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
    context_id: &ContextId,
    member: &PublicKey,
    capability: &u8,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    ctx.permissions
        .require_manage_members(signer, "revoke context capability")?;
    let current = CapabilitiesRepository::new(store)
        .context_member_capability(group_id, context_id, member)?
        .unwrap_or(0);
    CapabilitiesRepository::new(store).set_context_member(
        group_id,
        context_id,
        member,
        current & !capability,
    )?;
    Ok(())
}
