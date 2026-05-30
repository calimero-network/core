//! `GroupOp::CascadeGroupMigrationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{GroupSettingsService, PermissionChecker};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    migration: &Option<Vec<u8>>,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Mirror of `CascadeTargetApplicationSet` but for migration
    // bytes only. ASYMMETRY: this variant does NOT mark contexts
    // `InProgress` or kick the per-context migration propagator —
    // only the paired `CascadeTargetApplicationSet` op kicks
    // contexts into migration. The cascade-emitting RPC handler
    // (PR-2 Task 6) emits both ops in the same governance round
    // when the operator requested a cascade-with-migration, so the
    // status + propagator effects fire exactly once per cascade
    // round (driven by the target-application op, not this one).
    let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

    // Pre-scan: same atomicity guard as the target-application
    // arm — see the longer rationale comment there.
    for entry in &entries {
        if !entry.matched {
            continue;
        }
        let entry_permissions = PermissionChecker::new(store, entry.group_id);
        if !entry_permissions.can_manage_application(signer)? {
            bail!(
                "cascade group-migration set: signer {} lacks MANAGE_APPLICATION on \
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
                descendant_app_key = %hex::encode(entry.app_key),
                "CascadeGroupMigrationSet: skip (app_key mismatch)"
            );
            continue;
        }
        let entry_settings = GroupSettingsService::new(store, entry.group_id);
        entry_settings.set_group_migration(signer, migration)?;
        // Match-success log — symmetric with the
        // `CascadeTargetApplicationSet` apply log. Migration bytes
        // size is recorded instead of the bytes themselves to keep
        // logs compact.
        tracing::info!(
            target: "calimero::cascade",
            group_id = %hex::encode(entry.group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            migration_bytes_len = migration.as_ref().map(|m| m.len()).unwrap_or(0),
            "CascadeGroupMigrationSet: applied"
        );
        any_applied = true;
    }
    if !any_applied {
        tracing::debug!(
            target: "calimero::cascade",
            signed_group = %hex::encode(group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            "CascadeGroupMigrationSet: no descendants matched"
        );
    }
    // See the corresponding comment in `cascade_target_application_set`
    // — cascade variants never produce divergence reports.
    Ok(())
}
