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

/// What the lazy-upgrade path should do for a stale context under a
/// LazyOnAccess group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LazyUpgradeAction {
    /// Context has no activation marker (never activated anything): a
    /// single jump to the group's current target, method from the
    /// group-level hint. Sound ONLY for marker-less contexts — the hint
    /// describes the group's most recent hop, which may not be a marker-ed
    /// context's next one.
    SingleJump {
        target_application_id: ApplicationId,
        migrate_method: Option<String>,
        target_app_key: [u8; 32],
    },
    /// Context has an activation marker: replay the group's upgrade ladder
    /// from that bound blob, each hop's method resolved from the two
    /// blobs' embedded ABIs. The group-level migration hint is never
    /// executed on this arm.
    Replay { bound: [u8; 32] },
}

/// Whether this context, under a LazyOnAccess group, needs an upgrade or
/// migration, and via which mode. The caller must load bytecode by blob
/// key (bundle ids are version-stable) — the application row may still
/// hold the OLD wasm.
pub(super) fn maybe_lazy_upgrade(
    datastore: &Store,
    context_id: &ContextId,
    current_application_id: &ApplicationId,
) -> Option<LazyUpgradeAction> {
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

    // 4. The activation marker decides both staleness and the mode below.
    let activated = crate::activation::activated_blob(datastore, context_id);

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
        marker = activated.is_some(),
        "lazy upgrade triggered for context"
    );

    Some(match activated {
        Some(bound) => LazyUpgradeAction::Replay { bound },
        // No activation marker. The context never migrated (a commit would have
        // stamped one), so the bytecode blob its application row points at IS
        // its real current version. Replay the ladder hop-by-hop FROM that
        // version rather than single-jumping the group's latest-hop edge: a
        // context several versions behind must run v1->v2 then v2->v3, never the
        // latest edge (e.g. `migrate_v2_to_v3`) against older state — which
        // mis-decodes and panics. The call site seeds the activation marker to
        // this blob before replaying, which also binds execution to it, so a
        // blocked hop strands the context on its real version instead of running
        // the target's bytecode on un-migrated state.
        None => match crate::hlc_fence::loaded_reader_app_key(datastore, context_id) {
            Ok(Some(current)) if current != meta.app_key => {
                LazyUpgradeAction::Replay { bound: current }
            }
            // Current version unresolvable (no row), or it already equals the
            // group target. The latter still needs the single jump: the gate
            // only reaches this arm because activation is pending (no marker at
            // the target), and for a bundle (stable application id) a local
            // install bumps the shared application row to the target blob while
            // the migration is still pending — so `loaded_reader == target`
            // does NOT mean migrated. Returning None here would run the target
            // bytecode against un-migrated state.
            _ => LazyUpgradeAction::SingleJump {
                target_application_id: meta.target_application_id,
                migrate_method: meta
                    .migration
                    .as_ref()
                    .and_then(|bytes| String::from_utf8(bytes.clone()).ok()),
                target_app_key: meta.app_key,
            },
        },
    })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;

    use super::*;

    const APP_KEY_OLD: [u8; 32] = [0x01; 32];
    const APP_KEY_NEW: [u8; 32] = [0x02; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn target_app() -> ApplicationId {
        ApplicationId::from([0xAA; 32])
    }

    fn seed_group(store: &Store, ctx: &ContextId, policy: UpgradePolicy) -> ContextGroupId {
        let gid = ContextGroupId::from([0x60; 32]);
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextGroupRef::new((**ctx).into()),
                &gid.to_bytes(),
            )
            .unwrap();
        let admin = calimero_primitives::identity::PublicKey::from([0x07; 32]);
        calimero_governance_store::MetaRepository::new(store)
            .save(
                &gid,
                &GroupMetaValue {
                    app_key: APP_KEY_NEW,
                    target_application_id: target_app(),
                    upgrade_policy: policy,
                    created_at: 0,
                    admin_identity: admin,
                    owner_identity: admin,
                    migration: Some(b"migrate_v2_to_v3".to_vec()),
                    auto_join: false,
                },
            )
            .unwrap();
        gid
    }

    // The wrong-hop hole (PR-4 regression guard): a marker-ed context must
    // NEVER receive the group-level migration method — the hint describes
    // the group's most recent hop, while this context may be several rungs
    // below it. Running that method against older state mis-decodes or
    // corrupts. Marker-ed contexts replay the ladder instead.
    #[test]
    fn marker_ed_context_replays_and_never_carries_the_group_method() {
        let store = store();
        let ctx = ContextId::from([0x50; 32]);
        let _gid = seed_group(&store, &ctx, UpgradePolicy::LazyOnAccess);
        crate::activation::record_activation(&store, &ctx, APP_KEY_OLD);

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(action, LazyUpgradeAction::Replay { bound: APP_KEY_OLD });
    }

    /// Seed a context's application row so `loaded_reader_app_key` resolves the
    /// context's current bytecode blob to `blob` (an installed-but-never-migrated
    /// version). `app_id` keys the row; for a bundle it equals the group target.
    fn seed_app_row(store: &Store, ctx: &ContextId, app_id: ApplicationId, blob: [u8; 32]) {
        use calimero_store::types::{ApplicationMeta as ApplicationMetaValue, ContextMeta};
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ApplicationMeta::new(app_id),
                &ApplicationMetaValue::new(
                    calimero_store::key::BlobMeta::new(blob.into()),
                    0,
                    String::new().into_boxed_str(),
                    Box::new([]),
                    calimero_store::key::BlobMeta::new([0u8; 32].into()),
                    calimero_store::types::PackageInfo {
                        package: String::new().into_boxed_str(),
                        version: String::new().into_boxed_str(),
                        signer_id: String::new().into_boxed_str(),
                    },
                ),
            )
            .unwrap();
        handle
            .put(
                &calimero_store::key::ContextMeta::new(*ctx),
                &ContextMeta::new(
                    calimero_store::key::ApplicationMeta::new(app_id),
                    [0u8; 32],
                    vec![],
                    None,
                ),
            )
            .unwrap();
    }

    // Regression guard for the marker-less multi-version-behind hole: a fresh
    // joiner (no activation marker) whose group has advanced several versions
    // must REPLAY the ladder from its current row version, NOT single-jump the
    // group's latest-hop edge against older state (which mis-decodes + panics).
    #[test]
    fn marker_less_context_with_current_row_replays_from_its_version() {
        let store = store();
        let ctx = ContextId::from([0x51; 32]);
        let _gid = seed_group(&store, &ctx, UpgradePolicy::LazyOnAccess);
        // Context installed (never migrated) at APP_KEY_OLD; group target is
        // APP_KEY_NEW (bundle: same application id, different blob).
        seed_app_row(&store, &ctx, target_app(), APP_KEY_OLD);

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(action, LazyUpgradeAction::Replay { bound: APP_KEY_OLD });
    }

    // A marker-less context whose current version is unresolvable (no row, so
    // `loaded_reader_app_key` falls back to the group target) keeps the single
    // jump: the gate only reaches this arm because activation is pending, and
    // `loaded_reader == target` does NOT prove migration ran (a bundle install
    // bumps the shared row ahead of the marker). Returning None here would run
    // target bytecode on un-migrated state.
    #[test]
    fn marker_less_context_without_resolvable_row_keeps_the_single_jump() {
        let store = store();
        let ctx = ContextId::from([0x51; 32]);
        let _gid = seed_group(&store, &ctx, UpgradePolicy::LazyOnAccess);

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(
            action,
            LazyUpgradeAction::SingleJump {
                target_application_id: target_app(),
                migrate_method: Some("migrate_v2_to_v3".to_owned()),
                target_app_key: APP_KEY_NEW,
            }
        );
    }

    #[test]
    fn up_to_date_marker_returns_none() {
        let store = store();
        let ctx = ContextId::from([0x52; 32]);
        let _gid = seed_group(&store, &ctx, UpgradePolicy::LazyOnAccess);
        crate::activation::record_activation(&store, &ctx, APP_KEY_NEW);

        assert_eq!(maybe_lazy_upgrade(&store, &ctx, &target_app()), None);
    }

    #[test]
    fn non_lazy_policy_returns_none() {
        let store = store();
        let ctx = ContextId::from([0x53; 32]);
        let _gid = seed_group(&store, &ctx, UpgradePolicy::Automatic);
        crate::activation::record_activation(&store, &ctx, APP_KEY_OLD);

        assert_eq!(maybe_lazy_upgrade(&store, &ctx, &target_app()), None);
    }

    #[test]
    fn non_group_context_returns_none() {
        let store = store();
        let ctx = ContextId::from([0x54; 32]);
        assert_eq!(maybe_lazy_upgrade(&store, &ctx, &target_app()), None);
    }
}
