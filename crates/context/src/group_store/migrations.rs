use super::{collect_keys_with_prefix, ContextGroupId, ContextId, EyreResult, Store};
use calimero_store::key::{
    GroupContextLastMigration, GroupContextLastMigrationValue, GROUP_CONTEXT_LAST_MIGRATION_PREFIX,
};

/// Returns the migration method name that was last successfully applied to
/// `context_id` within `group_id`, or `None` if no migration has been recorded.
pub fn get_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
    Ok(handle
        .get(&key)?
        .map(|v: GroupContextLastMigrationValue| v.method))
}

/// Records that `method` was successfully applied to `context_id` within
/// `group_id`. Subsequent calls to `maybe_lazy_upgrade` will skip this
/// migration for this context unless a different method is configured.
pub fn set_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    method: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
    handle.put(
        &key,
        &GroupContextLastMigrationValue {
            method: method.to_owned(),
        },
    )?;
    Ok(())
}

pub fn delete_all_context_last_migrations(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupContextLastMigration::new(gid, ContextId::from([0u8; 32]).into()),
        GROUP_CONTEXT_LAST_MIGRATION_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
