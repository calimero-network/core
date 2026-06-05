//! Cascade-upgrade write-gate decisions for context execution: whether a
//! group-upgrade status blocks writes, whether a committed write should be
//! rejected mid-upgrade, the lazy-on-access migration trigger, and the
//! producing-app-key resolver. Extracted from the execute handler.

use calimero_governance_store::{MetaRepository, MigrationsRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, UpgradePolicy};
use calimero_store::Store;
use tracing::{debug, info};

/// Returns `true` when a group-upgrade status should block ALL writes
/// (both user calls and state-op writes such as `__calimero_sync_next`).
///
/// Only `GroupUpgradeStatus::InProgress` blocks.  `Completed` (with or
/// without a timestamp) never blocks.  This is the single source of truth
/// for the cascade-upgrade write-gate decision.
///
/// # Safety invariants
///
/// * `LazyOnAccess` upgrades write `Completed` directly (never `InProgress`),
///   so this fn never returns `true` during a lazy migration.
/// * The eager propagator's own writes go through `UpdateApplicationRequest`
///   → `handlers::update_application`, which bypasses the execute gate
///   entirely — no deadlock is possible.
/// * Sync-pipeline (`__calimero_sync_next`) failures during `InProgress` are
///   retried by the periodic sync cycle once the upgrade reaches `Completed`.
pub(super) fn upgrade_blocks_write(status: &calimero_store::key::GroupUpgradeStatus) -> bool {
    matches!(
        status,
        calimero_store::key::GroupUpgradeStatus::InProgress { .. }
    )
}

/// Whether the cascade write-gate should fire, given the `migration_v2` flag.
///
/// Equal to `!migration_v2 && upgrade_blocks_write(status)`: with the flag OFF
/// the group-wide `InProgress` freeze applies; with it ON the freeze is lifted
/// (absorb-don't-drop keeps stragglers safe instead).
pub(super) fn should_block(
    migration_v2: bool,
    status: &calimero_store::key::GroupUpgradeStatus,
) -> bool {
    !migration_v2 && upgrade_blocks_write(status)
}

/// Post-execution write-gate decision: during an in-progress upgrade a pure read
/// (`produced_write == false`) is served from the pre-migration root; a
/// side-effecting call is refused. Write-intent is derived post-execution (a
/// committed `root_hash` or queued `xcalls`) because no read-vs-write flag exists
/// upstream (`ExecuteRequest`, RPC, SDK, ABI).
pub(super) fn upgrade_rejects_committed_write(block_writes: bool, produced_write: bool) -> bool {
    block_writes && produced_write
}

/// Checks if a context belongs to a group with LazyOnAccess policy and
/// needs an upgrade or migration.
///
/// Returns `(target_application_id, migrate_method, group_id)` when an
/// upgrade should be performed.  The `group_id` is included so the caller
/// can record a per-context migration marker after a successful run.
pub(super) fn maybe_lazy_upgrade(
    datastore: &Store,
    context_id: &ContextId,
    current_application_id: &ApplicationId,
) -> Option<(
    ApplicationId,
    Option<String>,
    calimero_context_config::types::ContextGroupId,
)> {
    use calimero_governance_store;

    // 1. Check if context belongs to a group
    let group_id = match calimero_governance_store::get_group_for_context(datastore, context_id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None, // not in a group
        Err(err) => {
            debug!(%err, %context_id, "failed to check group for context during lazy upgrade");
            return None;
        }
    };

    // 2. Load group metadata
    let meta = match MetaRepository::new(datastore).load(&group_id) {
        Ok(Some(m)) => m,
        Ok(None) => return None, // group deleted?
        Err(err) => {
            debug!(%err, ?group_id, "failed to load group meta during lazy upgrade");
            return None;
        }
    };

    // 3. Check policy is LazyOnAccess
    if !matches!(meta.upgrade_policy, UpgradePolicy::LazyOnAccess) {
        return None;
    }

    // 4. Extract migration method from group meta (set during upgrade)
    let migrate_method = meta
        .migration
        .as_ref()
        .and_then(|bytes| String::from_utf8(bytes.clone()).ok());

    // 5. Compare current vs target application
    if *current_application_id == meta.target_application_id {
        // IDs match — only proceed if there is a pending migration that
        // hasn't been applied to this context yet.
        let Some(ref method) = migrate_method else {
            return None; // no migration, context is already up to date
        };

        // Check per-context marker set after a successful migration run.
        let already_applied = MigrationsRepository::new(datastore)
            .last_migration(&group_id, context_id)
            .ok()
            .flatten()
            .map(|last| last == *method)
            .unwrap_or(false);

        if already_applied {
            return None; // migration was already applied to this context
        }
        // Fall through: migration is pending.
    }

    info!(
        %context_id,
        ?group_id,
        %current_application_id,
        target_app=%meta.target_application_id,
        "lazy upgrade triggered for context"
    );

    Some((meta.target_application_id, migrate_method, group_id))
}

/// The blob-derived app key the sender is executing under — `GroupMeta.app_key`
/// for the context's owning group (`app_key = blob_id(bytecode)` at group
/// creation / upgrade time).  This is the schema-version discriminator that
/// changes on every app upgrade; `application_id` is version-stable and
/// cannot distinguish v1 from v2 of the same application.
///
/// Returns `Some(app_key)` for group-context deltas; `None` for non-group
/// contexts (no owning group) or when the group meta row cannot be loaded
/// (store error is propagated to the caller as `Err`).
///
/// Stamped onto the state-delta broadcast so receivers can fence
/// stale-schema deltas after a cascade migration.  The fence itself lives
/// in Tasks 8/9 — this function is the testable store-boundary helper.
pub(super) fn resolve_producing_app_key(
    datastore: &Store,
    context_id: &ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    let Some(gid) = calimero_governance_store::get_group_for_context(datastore, context_id)? else {
        return Ok(None);
    };
    Ok(MetaRepository::new(datastore)
        .load(&gid)?
        .map(|m| m.app_key))
}
