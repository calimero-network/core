use calimero_store::key::{
    GroupContextLastMigration, GroupContextLastMigrationValue, GROUP_CONTEXT_LAST_MIGRATION_PREFIX,
};

use super::{collect_keys_with_prefix, ContextGroupId, ContextId, EyreResult, Store};

/// Typed Repository for per-(group, context) migration tracking.
///
/// Replaces the free-function-over-`&Store` style with a struct that
/// borrows the store once and exposes its methods on `&self`. The
/// `&'a Store` borrow lives for the Repository's lifetime; callers
/// that want a long-lived handle clone a single `&Store` reference
/// into the constructor instead of passing it on every call.
///
/// Issue #2303 / epic #2300.
pub struct MigrationsRepository<'a> {
    store: &'a Store,
}

impl<'a> MigrationsRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Returns the migration method name that was last successfully applied
    /// to `context_id` within `group_id`, or `None` if no migration has
    /// been recorded.
    pub fn last_migration(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
    ) -> EyreResult<Option<String>> {
        let handle = self.store.handle();
        let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
        Ok(handle
            .get(&key)?
            .map(|v: GroupContextLastMigrationValue| v.method))
    }

    /// Records that `method` was successfully applied to `context_id`
    /// within `group_id`. Subsequent calls to `maybe_lazy_upgrade` will
    /// skip this migration for this context unless a different method
    /// is configured.
    pub fn set_last_migration(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
        method: &str,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
        handle.put(
            &key,
            &GroupContextLastMigrationValue {
                method: method.to_owned(),
            },
        )?;
        Ok(())
    }

    /// Deletes all per-context migration rows for `group_id`. Used by
    /// group-cascade cleanup.
    pub fn delete_all_for_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupContextLastMigration::new(gid, ContextId::from([0u8; 32]).into()),
            GROUP_CONTEXT_LAST_MIGRATION_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
//
// Preserved for one release cycle so existing callers compile unchanged.
// Each wrapper constructs a transient `MigrationsRepository` and delegates.
// New code should use `MigrationsRepository::new(store).method(...)`.
// ---------------------------------------------------------------------------

#[deprecated(note = "use MigrationsRepository::new(store).last_migration(...)")]
pub fn get_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<String>> {
    MigrationsRepository::new(store).last_migration(group_id, context_id)
}

#[deprecated(note = "use MigrationsRepository::new(store).set_last_migration(...)")]
pub fn set_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    method: &str,
) -> EyreResult<()> {
    MigrationsRepository::new(store).set_last_migration(group_id, context_id, method)
}

#[deprecated(note = "use MigrationsRepository::new(store).delete_all_for_group(...)")]
pub fn delete_all_context_last_migrations(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()> {
    MigrationsRepository::new(store).delete_all_for_group(group_id)
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;

    use super::*;
    use crate::group_store::test_fixtures::{test_group_id, test_store};

    fn ctx_id(seed: u8) -> ContextId {
        ContextId::from([seed; 32])
    }

    #[test]
    fn last_migration_returns_none_when_unset() {
        let store = test_store();
        let repo = MigrationsRepository::new(&store);
        assert!(repo
            .last_migration(&test_group_id(), &ctx_id(1))
            .unwrap()
            .is_none());
    }

    #[test]
    fn set_then_last_migration_round_trip() {
        let store = test_store();
        let repo = MigrationsRepository::new(&store);
        let gid = test_group_id();
        let ctx = ctx_id(1);

        repo.set_last_migration(&gid, &ctx, "migrate_v1_to_v2")
            .unwrap();
        assert_eq!(
            repo.last_migration(&gid, &ctx).unwrap().as_deref(),
            Some("migrate_v1_to_v2"),
        );
    }

    #[test]
    fn set_last_migration_overwrites_prior_value() {
        let store = test_store();
        let repo = MigrationsRepository::new(&store);
        let gid = test_group_id();
        let ctx = ctx_id(1);

        repo.set_last_migration(&gid, &ctx, "v1_to_v2").unwrap();
        repo.set_last_migration(&gid, &ctx, "v2_to_v3").unwrap();
        assert_eq!(
            repo.last_migration(&gid, &ctx).unwrap().as_deref(),
            Some("v2_to_v3"),
        );
    }

    #[test]
    fn delete_all_for_group_clears_only_that_group() {
        let store = test_store();
        let repo = MigrationsRepository::new(&store);
        let gid_a = test_group_id();
        let gid_b = ContextGroupId::from([0xBB; 32]);
        let ctx = ctx_id(1);

        repo.set_last_migration(&gid_a, &ctx, "v1_to_v2").unwrap();
        repo.set_last_migration(&gid_b, &ctx, "v2_to_v3").unwrap();

        repo.delete_all_for_group(&gid_a).unwrap();

        assert!(repo.last_migration(&gid_a, &ctx).unwrap().is_none());
        assert_eq!(
            repo.last_migration(&gid_b, &ctx).unwrap().as_deref(),
            Some("v2_to_v3"),
            "delete_all_for_group must not affect sibling groups",
        );
    }

    #[test]
    fn delete_all_for_group_is_idempotent_when_empty() {
        let store = test_store();
        let repo = MigrationsRepository::new(&store);
        // No prior writes; delete_all should succeed as a no-op.
        repo.delete_all_for_group(&test_group_id()).unwrap();
    }
}
