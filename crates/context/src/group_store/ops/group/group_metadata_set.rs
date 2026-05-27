//! `GroupOp::GroupMetadataSet` apply handler. Extracted from
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
    name: &Option<String>,
    data: &BTreeMap<String, String>,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    ctx.permissions.require_can_manage_metadata(signer)?;
    validate_metadata_payload(name.as_deref(), data).map_err(|e| eyre::eyre!(e))?;
    MetadataRepository::new(store).set_group(
        group_id,
        &MetadataRecord {
            name: name.clone(),
            data: data.clone(),
            updated_at: now_millis(),
            updated_by: *signer,
        },
    )?;
    Ok(())
}
