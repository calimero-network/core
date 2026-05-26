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
