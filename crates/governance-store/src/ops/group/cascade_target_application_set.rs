//! `GroupOp::CascadeTargetApplicationSet` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::GroupSettingsService;
use calimero_primitives::application::ApplicationId;
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Authorize the cascade ONCE, against the ROOT signed group, using the
    // at-cut apply-auth resolver (`ctx.permissions()`) — the same deterministic,
    // replicated decision the rest of governance folds. The cascade's authority
    // is the root admin decision (already enforced at emit via
    // `MembershipRepository::require_admin`), NOT per-descendant caps.
    //
    // The old code instead re-derived `can_manage_application(signer)` per matched
    // descendant from LIVE store reads. Descendant caps come from `MemberCapabilitySet`
    // / `DefaultCapabilitiesSet` / `MemberRoleSet` ops concurrent to the cascade,
    // folded at different times per peer, so a peer that had folded a concurrent
    // cap-revoke on a matched descendant BAILED the whole op while a peer that
    // hadn't APPLIED it everywhere — a permanent, non-self-healing divergence on
    // `target_application_id` / `app_key`. Gating once at the root removes that
    // per-descendant divergence on BOTH the namespace-envelope and standalone paths.
    ctx.permissions()
        .require_manage_application(signer, "cascade target application")?;

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

        // Reuse the single-group settings mutation, scoped to each matched
        // descendant, via the UNCHECKED write: the cascade was already
        // authorized once against the root admin above. Re-deriving live
        // per-descendant authority here is exactly the cross-replica
        // divergence this fix removes.
        let entry_settings = GroupSettingsService::new(store, entry.group_id);
        entry_settings.set_target_application_unchecked(app_key, target_application_id)?;
        // Match-success log. Mirrors the skip-log fields so the
        // emitter (which both applies locally AND publishes the op)
        // and the receivers (which apply on gossip-receive) both
        // produce a consistent per-descendant trail of which
        // descendant flipped, what `from_app_key` predicate matched
        // it, and what `app_key` it landed on. Without this, the
        // receiver's successful apply was silent and the only way
        // to verify the cascade reached a remote peer was a
        // post-hoc `get_group_info` round-trip.
        tracing::info!(
            target: "calimero::cascade",
            group_id = %hex::encode(entry.group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            app_key = %hex::encode(app_key),
            %target_application_id,
            "CascadeTargetApplicationSet: applied"
        );

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
