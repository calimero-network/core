use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{AbortMigrationRequest, AbortMigrationResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    enumerate_group_contexts, MembershipRepository, MetaRepository, NamespaceRepository,
    UpgradesRepository,
};
use calimero_store::key;
use eyre::bail;
use tracing::info;

use crate::ContextManager;

/// Logically abort an in-flight namespace migration: flip each affected
/// group's target back to the pre-migration app id, clear `meta.migration`,
/// and drop its pending upgrade record. No snapshot, no restore —
/// already-committed contexts are NOT recalled. A cascade applied the same
/// migration to descendants matched on `app_key == from_app_key`, so the
/// abort walks the subtree and aborts each matched descendant too.
/// Pure store I/O; idempotent (`aborted: false` when nothing is pending).
pub fn abort_group_migration(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
) -> eyre::Result<AbortMigrationResponse> {
    let meta_repo = MetaRepository::new(store);
    let Some(root_meta) = meta_repo.load(namespace_id)? else {
        // No group metadata at all — nothing to abort. Idempotent no-op.
        return Ok(AbortMigrationResponse {
            namespace_id: *namespace_id,
            aborted: false,
        });
    };

    // The cascade predicate matches descendants on the root's `app_key` (the
    // cascade's `from_app_key`). Capture it before the root's meta is mutated.
    let from_app_key = root_meta.app_key;

    // Abort the requested root group first (preserving the original semantics).
    let mut aborted = abort_single_group(store, namespace_id, root_meta)?;

    // Then walk the descendant subtree and abort each matched descendant that
    // carries the same pending migration. `collect_descendants` EXCLUDES the
    // starting group (already handled above), exactly like
    // `collect_cascade_status` does on the read side.
    let descendants = NamespaceRepository::new(store).collect_descendants(namespace_id)?;
    for descendant_id in descendants {
        let Some(descendant_meta) = meta_repo.load(&descendant_id)? else {
            // Registered in the child index but meta not yet materialized (e.g. a
            // catching-up peer) — nothing to abort here. Same "missing meta ⇒
            // doesn't match" treatment the cascade walk uses.
            continue;
        };
        // Mirror the cascade predicate: only descendants the cascade migration
        // actually applied to (same `app_key` as the root's `from_app_key`).
        if descendant_meta.app_key != from_app_key {
            continue;
        }
        if abort_single_group(store, &descendant_id, descendant_meta)? {
            aborted = true;
        }
    }

    Ok(AbortMigrationResponse {
        namespace_id: *namespace_id,
        aborted,
    })
}

/// Apply the logical abort to a single group. Returns `true` if a pending
/// migration was found and cleared, `false` for the idempotent no-op case.
///
/// Caller supplies the already-loaded `meta` to avoid a redundant reload.
fn abort_single_group(
    store: &calimero_store::Store,
    group_id: &ContextGroupId,
    mut meta: calimero_store::key::GroupMetaValue,
) -> eyre::Result<bool> {
    let upgrades_repo = UpgradesRepository::new(store);
    let pending_upgrade = upgrades_repo.load(group_id)?;

    // Nothing pending if there is no migration marker on the group meta AND no
    // upgrade record carrying a migration. Idempotent no-op.
    let has_pending_meta_migration = meta.migration.is_some();
    let has_pending_upgrade_migration = pending_upgrade
        .as_ref()
        .map(|u| u.migration.is_some())
        .unwrap_or(false);
    if !has_pending_meta_migration && !has_pending_upgrade_migration {
        return Ok(false);
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
    let contexts = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
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
    MetaRepository::new(store).save(group_id, &meta)?;

    // Drop the pending upgrade record (the migration marker) so a future
    // `get_migration_status` / lazy-upgrade pass sees no in-flight migration.
    // Per-context activation markers stay: the up-to-date rule is
    // `marker == group.app_key`, so a re-issued upgrade (which moves the
    // app_key forward again) re-fires on every not-yet-activated context,
    // while already-committed contexts stay correctly suppressed.
    upgrades_repo.delete(group_id)?;

    info!(
        ?group_id,
        target_app = %meta.target_application_id,
        "migration logically aborted: target flipped back, pending migration dropped \
         (already-committed v2 contexts are NOT recalled)"
    );

    Ok(true)
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
        register_context_in_group, MetaRepository, NamespaceRepository, UpgradesRepository,
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
    /// An app_key belonging to an unrelated subgroup that the cascade never
    /// matched (its `app_key != root.app_key`). Used to exercise the skip branch.
    const OTHER_APP_KEY: [u8; 32] = [0x33; 32];
    /// The v2 target of that unrelated subgroup's independent migration.
    const OTHER_V2_APP: [u8; 32] = [0x44; 32];

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn group_meta(target: [u8; 32], migration: Option<Vec<u8>>) -> GroupMetaValue {
        group_meta_with_app_key(V1_APP, target, migration)
    }

    fn group_meta_with_app_key(
        app_key: [u8; 32],
        target: [u8; 32],
        migration: Option<Vec<u8>>,
    ) -> GroupMetaValue {
        let pk = PublicKey::from([0xAB; 32]);
        GroupMetaValue {
            app_key,
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

    /// **Cascade abort.** A namespace mid-*cascade* migration applies the same
    /// pending migration to descendant subgroups (`GroupOp::CascadeUpgrade` /
    /// `CascadeGroupMigrationSet` walks the subtree and sets each matched
    /// descendant's `migration` marker + v2 target). Aborting the ROOT must
    /// therefore also abort every descendant carrying the same pending migration
    /// — otherwise the descendants keep lazy-migrating and the abort is
    /// incomplete. After aborting the root: BOTH the root AND the descendant have
    /// their target flipped back to the pre-migration v1 app id and their
    /// migration markers cleared, so a descendant context no longer lazy-migrates.
    #[test]
    fn abort_root_also_aborts_cascade_descendant() {
        let store = fresh_store();
        let root_id = ContextGroupId::from([0xF2; 32]);
        let child_id = ContextGroupId::from([0xC3; 32]);
        let root_ctx = ContextId::from([0xF1; 32]);
        let child_ctx = ContextId::from([0xD4; 32]);

        // The descendant subgroup is nested under the root so the abort's subtree
        // walk (collect_descendants) reaches it.
        NamespaceRepository::new(&store)
            .nest(&root_id, &child_id)
            .expect("nest child under root");

        // Both groups still run the pre-migration v1 app via their contexts.
        install_v1_context(&store, &root_id, &root_ctx);
        install_v1_context(&store, &child_id, &child_ctx);

        // Both groups are mid-cascade-migration: target → v2, migration marker set,
        // and (crucially) the same `app_key` (V1_APP) that the cascade predicate
        // matched on.
        let meta_repo = MetaRepository::new(&store);
        meta_repo
            .save(
                &root_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save root meta");
        meta_repo
            .save(
                &child_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save child meta");
        let upgrades_repo = UpgradesRepository::new(&store);
        upgrades_repo
            .save(&root_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save root upgrade");
        upgrades_repo
            .save(&child_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save child upgrade");

        // Abort the ROOT only.
        let resp = abort_group_migration(&store, &root_id).expect("abort root");
        assert!(resp.aborted, "the root had a pending migration");

        // Root is aborted.
        let root_meta = meta_repo.load(&root_id).unwrap().expect("root meta");
        assert_eq!(
            root_meta.target_application_id,
            ApplicationId::from(V1_APP),
            "root target must flip back to v1"
        );
        assert!(root_meta.migration.is_none(), "root marker dropped");

        // The DESCENDANT must ALSO be aborted by the same root abort.
        let child_meta = meta_repo.load(&child_id).unwrap().expect("child meta");
        assert_eq!(
            child_meta.target_application_id,
            ApplicationId::from(V1_APP),
            "descendant target must ALSO flip back to v1 (cascade abort)"
        );
        assert!(
            child_meta.migration.is_none(),
            "descendant migration marker must ALSO be dropped — otherwise it keeps \
             lazy-migrating"
        );
        assert!(
            upgrades_repo.load(&child_id).unwrap().is_none(),
            "descendant pending upgrade record must ALSO be cleared"
        );
    }

    /// **Cascade abort must NOT clobber an unrelated descendant.** The cascade
    /// migration only ever touched descendants whose `app_key == root.app_key`
    /// (the cascade's `from_app_key`). A subgroup nested under the root that runs
    /// a *different* `app_key` and is independently mid-migration on its own
    /// (unrelated) app pair was NEVER part of this cascade — aborting the root
    /// must leave it fully intact. This exercises the load-bearing skip predicate
    /// at `abort_migration.rs:86` (`descendant.app_key != from_app_key`): without
    /// it, the root abort would wrongly clear that subgroup's independent pending
    /// migration too.
    #[test]
    fn abort_root_skips_descendant_on_different_app_key() {
        let store = fresh_store();
        let root_id = ContextGroupId::from([0xF2; 32]);
        // A cascade-matched descendant (same app_key as root) and an unrelated
        // descendant on a different app_key with its own independent migration.
        let matched_id = ContextGroupId::from([0xC3; 32]);
        let unrelated_id = ContextGroupId::from([0xE5; 32]);
        let root_ctx = ContextId::from([0xF1; 32]);
        let matched_ctx = ContextId::from([0xD4; 32]);
        let unrelated_ctx = ContextId::from([0xA6; 32]);

        // Both descendants are nested under the root so the subtree walk reaches them.
        let ns_repo = NamespaceRepository::new(&store);
        ns_repo
            .nest(&root_id, &matched_id)
            .expect("nest matched child under root");
        ns_repo
            .nest(&root_id, &unrelated_id)
            .expect("nest unrelated child under root");

        // Root + matched descendant still run the pre-migration v1 app.
        install_v1_context(&store, &root_id, &root_ctx);
        install_v1_context(&store, &matched_id, &matched_ctx);
        // The unrelated descendant runs a context still on its own pre-migration app.
        install_context(&store, &unrelated_id, &unrelated_ctx, OTHER_APP_KEY);

        let meta_repo = MetaRepository::new(&store);
        // Root + matched descendant: mid-cascade on app_key V1_APP, target → V2_APP.
        meta_repo
            .save(
                &root_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save root meta");
        meta_repo
            .save(
                &matched_id,
                &group_meta(V2_APP, Some(b"migrate_v1_v2".to_vec())),
            )
            .expect("save matched child meta");
        // Unrelated descendant: a DIFFERENT app_key with its OWN independent pending
        // migration (different app pair, different migration method).
        meta_repo
            .save(
                &unrelated_id,
                &group_meta_with_app_key(
                    OTHER_APP_KEY,
                    OTHER_V2_APP,
                    Some(b"migrate_other_v1_v2".to_vec()),
                ),
            )
            .expect("save unrelated child meta");

        let upgrades_repo = UpgradesRepository::new(&store);
        upgrades_repo
            .save(&root_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save root upgrade");
        upgrades_repo
            .save(&matched_id, &upgrade_value(Some(b"migrate_v1_v2".to_vec())))
            .expect("save matched child upgrade");
        upgrades_repo
            .save(
                &unrelated_id,
                &upgrade_value(Some(b"migrate_other_v1_v2".to_vec())),
            )
            .expect("save unrelated child upgrade");

        // Abort the ROOT only.
        let resp = abort_group_migration(&store, &root_id).expect("abort root");
        assert!(resp.aborted, "the root had a pending migration");

        // The cascade-matched descendant IS aborted.
        let matched_meta = meta_repo.load(&matched_id).unwrap().expect("matched meta");
        assert_eq!(
            matched_meta.target_application_id,
            ApplicationId::from(V1_APP),
            "cascade-matched descendant target must flip back to v1"
        );
        assert!(
            matched_meta.migration.is_none(),
            "cascade-matched descendant marker must be dropped"
        );

        // The unrelated descendant on a different app_key is left FULLY untouched:
        // its target still points at its own v2, its migration marker is still set,
        // and its upgrade record is still present.
        let unrelated_meta = meta_repo
            .load(&unrelated_id)
            .unwrap()
            .expect("unrelated meta");
        assert_eq!(
            unrelated_meta.target_application_id,
            ApplicationId::from(OTHER_V2_APP),
            "unrelated descendant target must NOT be touched (cascade never matched it)"
        );
        assert_eq!(
            unrelated_meta.migration,
            Some(b"migrate_other_v1_v2".to_vec()),
            "unrelated descendant migration marker must NOT be cleared"
        );
        assert!(
            upgrades_repo.load(&unrelated_id).unwrap().is_some(),
            "unrelated descendant pending upgrade record must NOT be cleared"
        );
    }

    /// A cascade abort is idempotent across the whole subtree: when neither the
    /// root nor its descendant has a pending migration, aborting the root is a
    /// no-op `Ok` and leaves the descendant untouched.
    #[test]
    fn abort_cascade_subtree_idempotent_when_nothing_pending() {
        let store = fresh_store();
        let root_id = ContextGroupId::from([0xF2; 32]);
        let child_id = ContextGroupId::from([0xC3; 32]);

        NamespaceRepository::new(&store)
            .nest(&root_id, &child_id)
            .expect("nest child under root");

        let meta_repo = MetaRepository::new(&store);
        // Both already on v1 with no migration marker.
        meta_repo
            .save(&root_id, &group_meta(V1_APP, None))
            .expect("save root meta");
        meta_repo
            .save(&child_id, &group_meta(V1_APP, None))
            .expect("save child meta");

        let resp = abort_group_migration(&store, &root_id).expect("abort root");
        assert!(
            !resp.aborted,
            "nothing pending anywhere in the subtree → aborted = false"
        );

        let child_meta = meta_repo.load(&child_id).unwrap().expect("child meta");
        assert_eq!(
            child_meta.target_application_id,
            ApplicationId::from(V1_APP),
            "descendant left untouched"
        );
        assert!(child_meta.migration.is_none());
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
