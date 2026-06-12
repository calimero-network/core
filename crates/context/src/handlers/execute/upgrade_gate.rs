//! Cascade-upgrade write-gate decisions for context execution: whether a
//! group-upgrade status blocks writes, whether a committed write should be
//! rejected mid-upgrade, the lazy-on-access migration trigger, and the
//! producing-app-key resolver. Extracted from the execute handler.

use calimero_governance_store::MetaRepository;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, UpgradePolicy};
use calimero_store::Store;
use tracing::{debug, info};

/// `true` when a group-upgrade status blocks ALL writes (user calls and
/// state-ops alike): only `GroupUpgradeStatus::InProgress` blocks. Lazy
/// upgrades write `Completed` directly and the eager propagator bypasses the
/// execute gate, so neither can deadlock on this.
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

/// Whether this context, under a LazyOnAccess group, needs an upgrade or
/// migration. Returns `(target_application_id, migrate_method,
/// target_app_key)` when one should run. The caller must load bytecode by
/// `target_app_key` (bundle ids are version-stable) — the application row
/// may still hold the OLD wasm.
pub(super) fn maybe_lazy_upgrade(
    datastore: &Store,
    context_id: &ContextId,
    current_application_id: &ApplicationId,
) -> Option<(ApplicationId, Option<String>, [u8; 32])> {
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
        // IDs match — bundle ids are version-stable, so this is either a
        // pending migration or a pending code-only bytecode bump. One rule
        // covers both: the context is up to date iff its activation marker
        // equals the group's recorded target blob. A zero app_key carries no
        // bytecode signal to compare against, so nothing can be detected.
        if meta.app_key == [0u8; 32] {
            return None;
        }
        let activated = crate::activation::activated_blob(datastore, context_id);
        if activated == Some(meta.app_key) {
            return None; // bytecode + migration current — context is up to date
        }
        // Fall through: activation (migration and/or bytecode swap) pending.
    }

    info!(
        %context_id,
        ?group_id,
        %current_application_id,
        target_app=%meta.target_application_id,
        "lazy upgrade triggered for context"
    );

    Some((meta.target_application_id, migrate_method, meta.app_key))
}

/// The blob-derived app key the sender executes under (`GroupMeta.app_key`
/// of the owning group) — the schema discriminator stamped onto state-delta
/// broadcasts so receivers can fence stale-schema deltas. `None` for
/// non-group contexts.
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
