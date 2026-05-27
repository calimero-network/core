//! `GroupOp::CascadeTargetApplicationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{GroupSettingsService, PermissionChecker};
use calimero_primitives::application::ApplicationId;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

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
                descendant_app_key = %hex::encode(entry.app_key),
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
    // The `divergence` field on the ctx stays as initialised
    // (`None` from `GroupApplyCtx::new`); no need to reset it
    // explicitly. A no-match outcome is a successful no-op, not
    // unknown-variant, so the caller still sees `handled = true`.
    Ok(())
}
