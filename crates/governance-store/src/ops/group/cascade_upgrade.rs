//! `GroupOp::CascadeUpgrade` apply handler (PR-3).
//!
//! Atomic replacement for the legacy two-op cascade path
//! (`CascadeTargetApplicationSet` + `CascadeGroupMigrationSet`). Both
//! legacy ops keyed their descendant walk on the SAME
//! `from_app_key == descendant.app_key` predicate, so a receiver that
//! applied target-set FIRST rewrote every descendant's `app_key` away
//! from `from_app_key`, leaving the later migration-set predicate
//! matching nothing and silently dropping `migration` (xilosada review
//! of core#2507, item #3 — see the characterization test in
//! `crates/context/tests/cascade_atomic_apply.rs`).
//!
//! This op sets `target_application_id`, `app_key`, AND `migration` in a
//! SINGLE walk per matched descendant, so there is no intra-cascade
//! ordering dependency the receiver can split. It also stamps a sticky
//! `cascade_hlc` fence onto each matched descendant's upgrade record:
//! identical on every node that applies the op (the initiator stamps it
//! once), it is the boundary the state-delta HLC fence reads. The field is
//! never cleared to `None` (it survives a `Completed` record); a later cascade
//! legitimately advances it to its own newer `cascade_hlc`.

use super::context::GroupApplyCtx;
use crate::{GroupSettingsService, PermissionChecker, UpgradesRepository};
use calimero_primitives::application::ApplicationId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
    migration: &Option<Vec<u8>>,
    cascade_hlc: HybridTimestamp,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Walk the descendant tree (incl. signed group) and apply the atomic
    // mutation to every descendant whose current `app_key` matches
    // `from_app_key`. Heterogeneous descendants are silently skipped —
    // that skip is also the optimistic-concurrency guard for two cascade
    // ops racing the same subtree. See the legacy
    // `cascade_target_application_set` module for the longer rationale.
    let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

    // Pre-scan: verify the signer would pass the per-descendant
    // `require_manage_application` check on EVERY matched descendant
    // before issuing any writes, so a stricter descendant deep in the
    // cascade can't abort the apply mid-loop after earlier descendants
    // were already mutated (partial-cascade state on both emitter and
    // receiver paths).
    for entry in &entries {
        if !entry.matched {
            continue;
        }
        let entry_permissions = PermissionChecker::new(store, entry.group_id);
        if !entry_permissions.can_manage_application(signer)? {
            bail!(
                "cascade upgrade: signer {} lacks MANAGE_APPLICATION on \
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
                "CascadeUpgrade: skip (app_key mismatch)"
            );
            continue;
        }

        let gid = entry.group_id;

        // Atomic per-descendant mutation: target_application_id + app_key
        // AND migration in one go, eliminating the legacy two-op ordering
        // hazard. The pre-scan above already verified `signer` holds
        // `MANAGE_APPLICATION` on every matched descendant, so these `?`s
        // are unreachable in production — kept as defensive backstops.
        let entry_settings = GroupSettingsService::new(store, gid);
        entry_settings.set_target_application(signer, app_key, target_application_id)?;
        entry_settings.set_group_migration(signer, migration)?;

        // Stamp the sticky cascade fence onto the per-group upgrade
        // record. Load-or-default: a descendant with no prior upgrade
        // record gets a fresh `Completed` record carrying the fence and
        // the cascade migration bytes. `cascade_hlc` is never cleared.
        let repo = UpgradesRepository::new(store);
        let mut value = repo.load(&gid)?.unwrap_or_else(|| GroupUpgradeValue {
            from_version: String::new(),
            to_version: String::new(),
            migration: migration.clone(),
            initiated_at: 0,
            initiated_by: *signer,
            status: GroupUpgradeStatus::Completed { completed_at: None },
            cascade_hlc: None,
        });
        // Reflect THIS cascade's migration bytes on an existing record too, so
        // the record's `migration` matches the `GroupMeta.migration` we just
        // wrote (the authoritative source the migrate runs from); otherwise an
        // existing record from a prior upgrade would carry stale migration bytes.
        value.migration = migration.clone();
        value.cascade_hlc = Some(cascade_hlc);
        repo.save(&gid, &value)?;

        tracing::info!(
            target: "calimero::cascade",
            group_id = %hex::encode(gid.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            app_key = %hex::encode(app_key),
            %target_application_id,
            migration_bytes_len = migration.as_ref().map(|m| m.len()).unwrap_or(0),
            "CascadeUpgrade: applied"
        );

        any_applied = true;
    }
    if !any_applied {
        tracing::debug!(
            target: "calimero::cascade",
            signed_group = %hex::encode(group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            "CascadeUpgrade: no descendants matched"
        );
    }
    // Cascade variants never produce per-op divergence reports — the
    // `divergence` field stays `None` from `GroupApplyCtx::new`. A
    // no-match outcome is a successful no-op, so the caller still sees
    // `handled = true`.
    Ok(())
}
