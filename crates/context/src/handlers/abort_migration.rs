use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{AbortMigrationRequest, AbortMigrationResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    enumerate_group_contexts, MembershipRepository, MetaRepository, MigrationsRepository,
    UpgradesRepository,
};
use calimero_store::key;
use eyre::bail;
use tracing::info;

use crate::ContextManager;

/// Logically abort an in-flight namespace migration (Task 6d.4).
///
/// Flips the group's pending migration target **back** to the pre-migration
/// application id and drops the pending `migration` marker so any not-yet-applied
/// lazy context stops migrating on its next access — `maybe_lazy_upgrade` no
/// longer triggers because there is no pending migration and the target matches
/// the contexts' current application id again.
///
/// This is a **logical** abort: there is NO byte snapshot and NO restore — the
/// v1 root was never mutated for not-yet-applied contexts, so nothing needs
/// un-doing. An already-committed v2 context is **not** recalled (that would be
/// the replicated-delta recall this train explicitly does not do — spec §7
/// invariant 5); this RPC only stops the rollout going forward.
///
/// Pure (store read/write only) so it can be exercised without standing up an
/// actor. Idempotent: a group with no pending migration is a no-op `Ok` with
/// `aborted: false`, never an error.
pub fn abort_group_migration(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
) -> eyre::Result<AbortMigrationResponse> {
    let meta_repo = MetaRepository::new(store);
    let Some(mut meta) = meta_repo.load(namespace_id)? else {
        // No group metadata at all — nothing to abort. Idempotent no-op.
        return Ok(AbortMigrationResponse {
            namespace_id: *namespace_id,
            aborted: false,
        });
    };

    let upgrades_repo = UpgradesRepository::new(store);
    let pending_upgrade = upgrades_repo.load(namespace_id)?;

    // Nothing pending if there is no migration marker on the group meta AND no
    // upgrade record carrying a migration. Idempotent no-op.
    let has_pending_meta_migration = meta.migration.is_some();
    let has_pending_upgrade_migration = pending_upgrade
        .as_ref()
        .map(|u| u.migration.is_some())
        .unwrap_or(false);
    if !has_pending_meta_migration && !has_pending_upgrade_migration {
        return Ok(AbortMigrationResponse {
            namespace_id: *namespace_id,
            aborted: false,
        });
    }

    // Recover the pre-migration application id from a context that is still on
    // it. Each not-yet-migrated context still runs its v1 `ContextMeta.application`,
    // so pointing the group target back at that id stops the lazy switch.
    //
    // In a partially-migrated (mixed-state) group some contexts may already have
    // committed to the v2 target while others are still v1. We must NOT pick the
    // target (v2) id: a still-v1 context whose `application` (v1) != target (v2)
    // takes `maybe_lazy_upgrade`'s IDs-mismatch branch and still lazy-swaps to v2
    // — defeating the abort. So we select a context whose `application` differs
    // from the current (v2) target, i.e. one still on the pre-migration id.
    //
    // If no such context is materialized we leave `target_application_id` as-is
    // and only drop the migration marker (which alone makes `maybe_lazy_upgrade`
    // return `None` on the IDs-match branch).
    let handle = store.handle();
    let contexts = enumerate_group_contexts(store, namespace_id, 0, usize::MAX)?;
    let pre_migration_app_id = contexts.into_iter().find_map(|context_id| {
        handle
            .get(&key::ContextMeta::new(context_id))
            .ok()
            .flatten()
            .map(|cm| cm.application.application_id())
            .filter(|app_id| *app_id != meta.target_application_id)
    });
    if let Some(app_id) = pre_migration_app_id {
        meta.target_application_id = app_id;
    }
    meta.migration = None;
    meta_repo.save(namespace_id, &meta)?;

    // Drop the pending upgrade record (the migration marker) so a future
    // `get_migration_status` / lazy-upgrade pass sees no in-flight migration.
    upgrades_repo.delete(namespace_id)?;

    // Clear per-context "last migration" markers for the group so a later,
    // intentional re-issue of the same migration is not suppressed as
    // already-applied.
    MigrationsRepository::new(store).delete_all_for_group(namespace_id)?;

    info!(
        ?namespace_id,
        target_app = %meta.target_application_id,
        "migration logically aborted: target flipped back, pending migration dropped \
         (already-committed v2 contexts are NOT recalled)"
    );

    Ok(AbortMigrationResponse {
        namespace_id: *namespace_id,
        aborted: true,
    })
}

impl Handler<AbortMigrationRequest> for ContextManager {
    type Result = ActorResponse<Self, <AbortMigrationRequest as Message>::Result>;

    fn handle(
        &mut self,
        AbortMigrationRequest { namespace_id }: AbortMigrationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // Admin-capability gate: mirror the `/groups/:id/upgrade*` routes —
            // the node's namespace identity must be an admin of the namespace.
            let Some((node_identity, _)) = self.node_namespace_identity(&namespace_id) else {
                bail!("node has no group identity configured");
            };
            MembershipRepository::new(&self.datastore)
                .require_admin(&namespace_id, &node_identity)?;
            abort_group_migration(&self.datastore, &namespace_id)
        })();
        ActorResponse::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        register_context_in_group, MetaRepository, MigrationsRepository, UpgradesRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{self, GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
    use calimero_store::types::ContextMeta as ContextMetaValue;
    use calimero_store::Store;

    use super::abort_group_migration;

    const V1_APP: [u8; 32] = [0x11; 32];
    const V2_APP: [u8; 32] = [0x22; 32];

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn group_meta(target: [u8; 32], migration: Option<Vec<u8>>) -> GroupMetaValue {
        let pk = PublicKey::from([0xAB; 32]);
        GroupMetaValue {
            app_key: V1_APP,
            target_application_id: ApplicationId::from(target),
            upgrade_policy: UpgradePolicy::LazyOnAccess,
            created_at: 1_700_000_000,
            admin_identity: pk,
            owner_identity: pk,
            migration,
            auto_join: false,
        }
    }

    fn upgrade_value(migration: Option<Vec<u8>>) -> GroupUpgradeValue {
        GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration,
            initiated_at: 1_700_000_000,
            initiated_by: PublicKey::from([0xAB; 32]),
            status: GroupUpgradeStatus::Completed { completed_at: None },
            cascade_hlc: None,
            cascade_seq: None,
        }
    }

    /// Write a `ContextMeta` for `context_id` pointing at `app` so the abort can
    /// recover the pre-migration app id from the group's contexts.
    fn install_context(
        store: &Store,
        group_id: &ContextGroupId,
        context_id: &ContextId,
        app: [u8; 32],
    ) {
        register_context_in_group(store, group_id, context_id).expect("register context");
        let mut handle = store.handle();
        handle
            .put(
                &key::ContextMeta::new(*context_id),
                &ContextMetaValue::new(
                    key::ApplicationMeta::new(ApplicationId::from(app)),
                    [0u8; 32],
                    Vec::new(),
                    None,
                ),
            )
            .expect("put context meta");
    }

    /// Convenience: install a context still on the v1 app id.
    fn install_v1_context(store: &Store, group_id: &ContextGroupId, context_id: &ContextId) {
        install_context(store, group_id, context_id, V1_APP);
    }

    /// After an admin abort, the group target flips back to the contexts'
    /// pre-migration v1 app id and the pending migration marker is dropped, so a
    /// subsequently-accessed context would NOT lazy-migrate.
    #[test]
    fn abort_flips_target_back_and_drops_migration() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xF2; 32]);
        let context_id = ContextId::from([0xF1; 32]);

        install_v1_context(&store, &group_id, &context_id);
        // Group is mid-migration: target points at v2 and a migration marker is set.
        MetaRepository::new(&store)
            .save(
                &group_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save meta");
        UpgradesRepository::new(&store)
            .save(&group_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save upgrade");
        // A per-context marker exists (e.g. from an earlier run of the same
        // migration on a sibling context). Abort must clear it so a deliberate
        // re-issue of the migration is not suppressed as already-applied. Without
        // this write the final `is_none()` assertion would be vacuous.
        MigrationsRepository::new(&store)
            .set_last_migration(&group_id, &context_id, "migrate_v1_v2")
            .expect("set last migration");

        let resp = abort_group_migration(&store, &group_id).expect("abort");
        assert!(
            resp.aborted,
            "a pending migration must report aborted = true"
        );

        let meta = MetaRepository::new(&store)
            .load(&group_id)
            .unwrap()
            .expect("meta present");
        assert_eq!(
            meta.target_application_id,
            ApplicationId::from(V1_APP),
            "target must flip back to the pre-migration v1 app id"
        );
        assert!(
            meta.migration.is_none(),
            "pending migration marker must be dropped"
        );
        assert!(
            UpgradesRepository::new(&store)
                .load(&group_id)
                .unwrap()
                .is_none(),
            "pending upgrade record must be cleared"
        );
        // The per-context marker is cleared so a deliberate re-issue is not suppressed.
        assert!(MigrationsRepository::new(&store)
            .last_migration(&group_id, &context_id)
            .unwrap()
            .is_none());
    }

    /// In a partially-migrated (mixed-state) group — one context already committed
    /// to v2 (lazy), another still on v1 — the abort must recover the pre-migration
    /// app id from a context that is still on the *pre-migration* id (the still-v1
    /// one), NOT blindly from the first enumerated context. Flipping the group
    /// target to v2 would leave still-v1 contexts lazy-migrating to v2 with no
    /// migrate method (see `maybe_lazy_upgrade`'s IDs-mismatch branch), defeating
    /// the abort.
    #[test]
    fn abort_mixed_state_group_flips_target_to_pre_migration_v1() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xF2; 32]);
        // A context already committed to v2 (lazy) and a context still on v1.
        let migrated_ctx = ContextId::from([0xA1; 32]);
        let pending_ctx = ContextId::from([0xB2; 32]);

        install_context(&store, &group_id, &migrated_ctx, V2_APP);
        install_context(&store, &group_id, &pending_ctx, V1_APP);

        // Group is mid-migration: target points at v2 and a migration marker is set.
        MetaRepository::new(&store)
            .save(
                &group_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save meta");
        UpgradesRepository::new(&store)
            .save(&group_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save upgrade");

        let resp = abort_group_migration(&store, &group_id).expect("abort");
        assert!(
            resp.aborted,
            "a pending migration must report aborted = true"
        );

        let meta = MetaRepository::new(&store)
            .load(&group_id)
            .unwrap()
            .expect("meta present");
        assert_eq!(
            meta.target_application_id,
            ApplicationId::from(V1_APP),
            "target must flip back to the pre-migration v1 app id, not the v2 app \
             id of an already-migrated context"
        );
        assert!(
            meta.migration.is_none(),
            "pending migration marker must be dropped"
        );
    }

    /// Aborting a group with no pending migration is an idempotent no-op `Ok`,
    /// not an error — including aborting an already-aborted group twice.
    #[test]
    fn abort_is_idempotent_noop_when_nothing_pending() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xF2; 32]);
        let context_id = ContextId::from([0xF1; 32]);

        install_v1_context(&store, &group_id, &context_id);
        // No migration marker, target already on v1.
        MetaRepository::new(&store)
            .save(&group_id, &group_meta(V1_APP, None))
            .expect("save meta");

        let resp = abort_group_migration(&store, &group_id).expect("abort");
        assert!(!resp.aborted, "nothing pending → aborted = false");

        // A second abort on the same group is still a no-op success.
        let resp2 = abort_group_migration(&store, &group_id).expect("second abort");
        assert!(!resp2.aborted);
    }

    /// Aborting a group that has no metadata at all is a no-op `Ok`, not an error.
    #[test]
    fn abort_unknown_group_is_noop() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xEE; 32]);
        let resp = abort_group_migration(&store, &group_id).expect("abort");
        assert!(!resp.aborted);
    }
}
