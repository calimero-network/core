//! `GroupOp::ContextRegistered` apply handler. Extracted from
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
    context_id: &ContextId,
    application_id: &ApplicationId,
    service_name: &Option<String>,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    ctx.context_registration
        .register(&ctx.permissions, signer, context_id, application_id)?;
    if let Some(name) = service_name {
        set_context_service_name(store, context_id, name)?;
    }
    // Signal any waiters (e.g. `join_context` racing against gossipsub
    // propagation) that the context→group mapping has just been
    // persisted. See `crate::registration_notify` for rationale.
    crate::registration_notify::notify(*context_id);
    crate::op_events::notify(crate::op_events::OpEvent::ContextRegistered {
        group_id: group_id.to_bytes(),
        context_id: *context_id,
    });
    Ok(())
}
