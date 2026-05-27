//! `GroupOp::CascadeTargetApplicationSet` apply handler. Extracted from
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
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
) -> EyreResult<()> {
    let signer = ctx.signer;
    let group_id = ctx.group_id;
    let store = ctx.store;

    // Walk the descendant tree (incl. signed group) and apply the
    // settings mutation to every descendant whose current `app_key`
    // matches `from_app_key`. Heterogeneous descendants (`app_key !=
    // from_app_key`) are silently skipped per spec § 3.2 — that
    // skip is also the optimistic-concurrency guard for two cascade
    // ops racing against the same subtree (spec § 5).
    //
    // The walk is read-only and cycle/depth-bounded; see the
    // `crate::cascade::walk_for_predicate` doc-comment.
    let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

    // Pre-scan: verify the signer would pass the per-descendant
    // `require_manage_application` check on EVERY matched
    // descendant before issuing any writes. Without this, a
    // descendant deep in the cascade with a stricter capability
    // configuration (e.g. Restricted subgroup where the
    // namespace-level admin signer is not a direct admin) would
    // cause the `set_target_application` `?` mid-loop to abort
    // the whole apply AFTER prior descendants have already been
    // mutated, leaving the store in a partial-cascade state on
    // both emitter and receiver paths.
    for entry in &entries {
        if !entry.matched {
            continue;
        }
        let entry_permissions = PermissionChecker::new(store, entry.group_id);
        if !entry_permissions.can_manage_application(signer)? {
            bail!(
                "cascade target-application set: signer {} lacks MANAGE_APPLICATION on \
                 descendant {}; aborting before any writes to keep cascade atomic",
                signer,
                hex::encode(entry.group_id.to_bytes())
            );
        }
    }

    let mut any_applied = false;
    for entry in entries {
        if !entry.matched {
            tracing::debug!(
                target: "calimero::cascade",
                group_id = %hex::encode(entry.group_id.to_bytes()),
                from_app_key = %hex::encode(from_app_key),
                "CascadeTargetApplicationSet: skip (app_key mismatch)"
            );
            continue;
        }

        // Reuse the existing single-group settings mutation, scoped
        // to each matched descendant. The pre-scan above already
        // verified `signer` holds `MANAGE_APPLICATION` on every
        // matched descendant, so the `?` here is unreachable in
        // production — kept as a defensive backstop in case the
        // store mutates between the scan and the apply (which
        // can't happen on the single-threaded namespace actor
        // path, but is cheap to leave in place).
        let entry_settings = GroupSettingsService::new(store, entry.group_id);
        entry_settings.set_target_application(signer, app_key, target_application_id)?;

        // Per-context InProgress status + per-context migration
        // propagator dispatch are intentionally NOT performed here:
        // `apply_group_op_mutations` is a sync store-only function
        // (no `ContextClient`, no `NodeClient`, no actor `Context`),
        // and `propagate_upgrade` is an async actor-spawned
        // routine. The cascade-emitting RPC handler
        // (`handlers/upgrade_group.rs`, PR-2 Task 6) is responsible
        // for spawning a `propagate_upgrade` per matched descendant
        // group it cascaded over, mirroring how it already spawns
        // one for the signed root on the single-group path.
        //
        // Peers receiving this op via gossip apply the settings
        // mutation here and then rely on the local write-gate
        // (PR-2 Task 7) to refuse user-initiated writes against
        // contexts whose group's `target_application_id` has been
        // cascaded ahead of their local execution state.

        any_applied = true;
    }
    if !any_applied {
        tracing::debug!(
            target: "calimero::cascade",
            signed_group = %hex::encode(group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            "CascadeTargetApplicationSet: no descendants matched"
        );
    }
    // Cascade variants don't produce per-op divergence reports —
    // the only producers today are MemberRemoved/MemberLeft.
    // Fall through to the function-tail `Ok((true, divergence))`
    // exit (rather than an early `return`) so the cascade arms
    // share the same handled-flag convention as every other
    // arm: the variant WAS recognised; a no-match outcome is a
    // successful no-op, not unknown-variant. Returning
    // `handled = false` here would make the caller
    // `apply_local_signed_group_op` bail with
    // "unsupported group op variant for local apply", which
    // also breaks the concurrent-cascade safety case (loser
    // cascade arrives with `from_app_key` no longer matching
    // anything and is intended to be silently swallowed) AND
    // the audit-log persistence path in
    // `namespace/governance.rs` (only persists when
    // `handled == true`).
    ctx.divergence = None;
    Ok(())
}
