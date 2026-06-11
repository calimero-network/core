//! The per-context activation marker: which bytecode blob this context last
//! ACTIVATED (a migration commit, or a code-only swap). One fact replaces the
//! two legacy markers — the method-name row written after a migrate and the
//! `blob:<hex>` synthetic row written after a code-only activation — so the
//! sync gate, the lazy trigger, and the migration rollup all share a single
//! up-to-date rule: `marker == group.app_key`.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MigrationsRepository;
use calimero_primitives::context::ContextId;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use tracing::debug;

/// The blob this context last activated, if the v2 marker is set.
pub fn activated_blob(store: &Store, context_id: &ContextId) -> Option<[u8; 32]> {
    store
        .handle()
        .get(&calimero_store::key::ContextActivatedBlob::new(*context_id))
        .ok()
        .flatten()
        .map(|v| v.blob)
}

/// Record that `context_id` now executes `blob` (migration committed or
/// code-only activation applied). Best-effort: a failed write means the
/// context re-runs its (idempotent) activation on next access.
pub fn record_activation(store: &Store, context_id: &ContextId, blob: [u8; 32]) {
    let mut handle = store.handle();
    if let Err(err) = handle.put(
        &calimero_store::key::ContextActivatedBlob::new(*context_id),
        &calimero_store::types::ContextActivatedBlob { blob },
    ) {
        debug!(%context_id, %err, "failed to record activation marker");
    }
}

/// The legacy `blob:<hex>` synthetic marker value for code-only activations
/// (pre-v2 shape, kept for one release so mixed fleets converge).
pub fn legacy_blob_marker(app_key: &[u8; 32]) -> String {
    format!("blob:{}", hex::encode(app_key))
}

/// Activation state for a context, folding legacy markers forward on first
/// read: returns the activated blob, consulting (in order) the v2 marker,
/// then the legacy per-context migration row — which counts as "activated at
/// the group's current app_key" when it matches either the group's recorded
/// migrate method or the legacy `blob:` marker for the current app_key.
/// A successful fold WRITES the v2 marker so subsequent reads are one get.
pub fn activated_blob_folding_legacy(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    meta: &GroupMetaValue,
) -> Option<[u8; 32]> {
    if let Some(blob) = activated_blob(store, context_id) {
        return Some(blob);
    }
    let legacy = MigrationsRepository::new(store)
        .last_migration(group_id, context_id)
        .ok()
        .flatten()?;
    let matches_method = meta
        .migration
        .as_ref()
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
        .is_some_and(|method| legacy == method);
    let matches_blob = legacy == legacy_blob_marker(&meta.app_key);
    if matches_method || matches_blob {
        // Return the equality answer either way, but never PERSIST a zero
        // marker: a legacy randomly-seeded/zero app_key carries no real blob
        // identity, and a stored zero would later read as "executes blob 0".
        if meta.app_key != [0u8; 32] {
            record_activation(store, context_id, meta.app_key);
        }
        return Some(meta.app_key);
    }
    // A stale legacy marker (older method / older blob) carries no usable
    // blob information — the context predates the current upgrade.
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn meta(app_key: [u8; 32], migration: Option<&str>) -> GroupMetaValue {
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::UpgradePolicy;
        use calimero_primitives::identity::PublicKey;
        GroupMetaValue {
            app_key,
            target_application_id: ApplicationId::from([0xEE; 32]),
            upgrade_policy: UpgradePolicy::LazyOnAccess,
            created_at: 1_700_000_000,
            admin_identity: PublicKey::from([0xAD; 32]),
            owner_identity: PublicKey::from([0xAD; 32]),
            migration: migration.map(|s| s.as_bytes().to_vec()),
            auto_join: true,
        }
    }

    #[test]
    fn marker_roundtrip() {
        let store = store();
        let ctx = ContextId::from([1u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), None);
        record_activation(&store, &ctx, [7u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([7u8; 32]));
        // Moves forward on re-activation.
        record_activation(&store, &ctx, [8u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([8u8; 32]));
    }

    #[test]
    fn folds_legacy_method_marker_forward() {
        let store = store();
        let gid = ContextGroupId::from([2u8; 32]);
        let ctx = ContextId::from([3u8; 32]);
        let m = meta([9u8; 32], Some("migrate_v1_to_v2"));
        MigrationsRepository::new(&store)
            .set_last_migration(&gid, &ctx, "migrate_v1_to_v2")
            .expect("set legacy marker");

        assert_eq!(
            activated_blob_folding_legacy(&store, &gid, &ctx, &m),
            Some([9u8; 32])
        );
        // Fold persisted the v2 marker.
        assert_eq!(activated_blob(&store, &ctx), Some([9u8; 32]));
    }

    #[test]
    fn folds_legacy_blob_marker_forward() {
        let store = store();
        let gid = ContextGroupId::from([4u8; 32]);
        let ctx = ContextId::from([5u8; 32]);
        let m = meta([0xAB; 32], None);
        MigrationsRepository::new(&store)
            .set_last_migration(&gid, &ctx, &legacy_blob_marker(&[0xAB; 32]))
            .expect("set legacy marker");

        assert_eq!(
            activated_blob_folding_legacy(&store, &gid, &ctx, &m),
            Some([0xAB; 32])
        );
    }

    #[test]
    fn stale_legacy_marker_does_not_fold() {
        let store = store();
        let gid = ContextGroupId::from([6u8; 32]);
        let ctx = ContextId::from([7u8; 32]);
        // Group has moved on: method recorded is the OLD release's.
        let m = meta([0xCD; 32], Some("migrate_v2_to_v3"));
        MigrationsRepository::new(&store)
            .set_last_migration(&gid, &ctx, "migrate_v1_to_v2")
            .expect("set legacy marker");

        assert_eq!(activated_blob_folding_legacy(&store, &gid, &ctx, &m), None);
        assert_eq!(activated_blob(&store, &ctx), None);
    }
}
