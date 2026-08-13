//! `GroupOp::CascadeUpgrade` apply handler.
//!
//! Sets `target_application_id`, `app_key`, AND `migration` in a
//! SINGLE walk per matched descendant, so there is no intra-cascade
//! ordering dependency a receiver can split across two applies. It also
//! stamps a sticky `cascade_hlc` fence onto each matched descendant's
//! upgrade record: identical on every node that applies the op (the
//! initiator stamps it once), it is the boundary the state-delta HLC fence
//! reads. The field is never cleared to `None` (it survives a `Completed`
//! record); a later cascade legitimately advances it to its own newer
//! `cascade_hlc`.

use super::context::GroupApplyCtx;
use crate::{
    GroupSettingsService, MetaRepository, MetadataRepository, NamespaceRepository,
    UpgradesRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::key::{
    ApplicationMeta, GroupUpgradeStatus, GroupUpgradeValue, NamespaceGovHead,
};
use eyre::Result as EyreResult;

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    from_app_key: &[u8; 32],
    app_key: &[u8; 32],
    target_application_id: &ApplicationId,
    to_state_version: u32,
    migration: &Option<Vec<u8>>,
    cascade_hlc: HybridTimestamp,
) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Authorize the cascade ONCE, against the ROOT signed group, using the
    // at-cut apply-auth resolver (`ctx.permissions()`) — the deterministic,
    // replicated decision the rest of governance folds. The cascade's authority
    // is the root admin decision (already enforced at emit via
    // `MembershipRepository::require_admin`), NOT per-descendant caps.
    //
    // The old code re-derived `can_manage_application(signer)` per matched
    // descendant from LIVE store reads. Descendant caps come from concurrent
    // `MemberCapabilitySet` / `DefaultCapabilitiesSet` / `MemberRoleSet` ops,
    // folded at different times per peer, so a peer that had folded a concurrent
    // cap-revoke on a matched descendant BAILED the whole op (Err before the
    // nonce was recorded — no partial apply, not self-healing) while a peer that
    // hadn't APPLIED to root + all descendants: a permanent divergence on
    // `target_application_id` / `app_key` / migration. Gating once at the root
    // removes that per-descendant divergence on BOTH the namespace-envelope and
    // standalone paths.
    ctx.permissions()
        .require_manage_application(signer, "cascade upgrade")?;

    // Walk the descendant tree (incl. signed group) and apply the atomic
    // mutation to every descendant whose current `app_key` matches
    // `from_app_key`. Heterogeneous descendants are silently skipped —
    // that skip is also the optimistic-concurrency guard for two cascade
    // ops racing the same subtree.
    let entries = crate::cascade::walk_for_predicate(store, *group_id, *from_app_key)?;

    // The migration's expand-entry governance position: the namespace gov-head
    // sequence as it stands when this CascadeUpgrade applies (BEFORE the op's own
    // head advance, which `apply_signed_op` runs after mutations). This is the
    // governance cut immediately preceding the migration — the same
    // `NamespaceGovHead.sequence` space the migration heartbeat reports as
    // `synced_up_to_hlc` (see `MigrationEmitter::refresh_hlc`). The
    // migration-status rollup pins the cohort by comparing those two SEQUENCES
    // like-for-like; `cascade_hlc` (an NTP64 HLC) must never serve as that pin.
    // A genuinely-absent head leaves `cascade_seq` `None` (no pin), but a read
    // error MUST propagate: swallowing it with `.ok()` would let the same op
    // pin `None` on a node that hit a transient store error while peers pinned
    // `Some(seq)`, diverging the cohort comparison across the cluster.
    let ns = NamespaceRepository::new(store).resolve(group_id)?;
    let cascade_seq = store
        .handle()
        .get(&NamespaceGovHead::new(ns.to_bytes()))?
        .map(|head| head.sequence);

    // The signed group's pre-cascade target, for the announcement's `from`
    // side. Read before the walk below rewrites it. Non-fatal like every other
    // read the announcement needs: it is observational, so it must not be able
    // to fail the apply and diverge this replica.
    let previous_application_id = MetaRepository::new(store)
        .load(group_id)
        .ok()
        .flatten()
        .map(|meta| meta.target_application_id);

    // Semver strings for the per-descendant record, resolved from this node's
    // own application rows: nothing on the wire carries them, and the empty
    // strings this used to write are what a receiver's completion banner
    // renders. Same local lookup, same `unknown` fallback, as the announcement.
    let version_of = |id: &ApplicationId| {
        store
            .handle()
            .get(&ApplicationMeta::new(*id))
            .ok()
            .flatten()
            .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version))
    };
    let to_version = version_of(target_application_id);
    let from_version = previous_application_id
        .as_ref()
        .map_or_else(|| "unknown".to_owned(), version_of);

    let mut any_applied = false;
    let mut local_contexts_total: u32 = 0;
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
        // AND migration in one go, so no receiver can split the cascade across
        // two applies. UNCHECKED writes: the cascade was authorized once
        // against the root admin above, so per-descendant authority is not re-derived
        // here (doing so from live caps is the cross-replica divergence this
        // fix removes).
        let entry_settings = GroupSettingsService::new(store, gid);
        entry_settings.set_target_application_unchecked(app_key, target_application_id)?;
        entry_settings.set_group_migration_unchecked(migration)?;

        // Stamp the sticky cascade fence onto the per-group upgrade
        // record. Load-or-default: a descendant with no prior upgrade
        // record gets a fresh `Completed` record carrying the fence and
        // the cascade migration bytes. `cascade_hlc` is never cleared.
        let repo = UpgradesRepository::new(store);
        let mut value = repo.load(&gid)?.unwrap_or_else(|| GroupUpgradeValue {
            from_version: from_version.clone(),
            to_version: to_version.clone(),
            migration: migration.clone(),
            initiated_at: 0,
            initiated_by: *signer,
            status: GroupUpgradeStatus::Completed {
                completed_at: None,
                fleet_completed_at: None,
            },
            cascade_hlc: None,
            cascade_seq: None,
            to_state_version,
        });
        // Overwrite on an existing record too: a stale value from a prior
        // upgrade reads as a satisfied target, which is a false green. The
        // versions go the same way - the completion event reads `to_version`
        // off this record, so a stale one banners the previous migration.
        value.to_state_version = to_state_version;
        value.from_version = from_version.clone();
        value.to_version = to_version.clone();
        // Reflect THIS cascade's migration bytes on an existing record too, so
        // the record's `migration` matches the `GroupMeta.migration` we just
        // wrote (the authoritative source the migrate runs from); otherwise an
        // existing record from a prior upgrade would carry stale migration bytes.
        value.migration = migration.clone();
        value.cascade_hlc = Some(cascade_hlc);
        value.cascade_seq = cascade_seq;
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
        local_contexts_total = local_contexts_total.saturating_add(
            MetadataRepository::new(store)
                .count_contexts(&gid)
                .unwrap_or_default() as u32,
        );
    }
    if !any_applied {
        tracing::debug!(
            target: "calimero::cascade",
            signed_group = %hex::encode(group_id.to_bytes()),
            from_app_key = %hex::encode(from_app_key),
            "CascadeUpgrade: no descendants matched"
        );
    }
    // One announcement for the whole cascade, keyed on the signed group the
    // bridge resolves to a namespace root. `to_state_version` rides on the op:
    // it is the initiator's resolved value, so every node reports the same
    // target even when it has not fetched the bytecode yet.
    if let Some(previous_application_id) = previous_application_id.filter(|_| any_applied) {
        ctx.queue_migration_started(
            &previous_application_id,
            target_application_id,
            Some(to_state_version),
            local_contexts_total,
        );
    }
    // Cascade variants never produce per-op divergence reports — the
    // `divergence` field stays `None` from `GroupApplyCtx::new`. A
    // no-match outcome is a successful no-op, so the caller still sees
    // `handled = true`.
    Ok(())
}
