use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{
    GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue, GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

pub fn save_group_upgrade(
    store: &Store,
    group_id: &ContextGroupId,
    upgrade: &GroupUpgradeValue,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    handle.put(&key, upgrade)?;
    Ok(())
}

pub fn load_group_upgrade(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupUpgradeValue>> {
    let handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn delete_group_upgrade(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    handle.delete(&key)?;
    Ok(())
}

#[cfg(test)]
pub fn extract_application_id(
    app_json: &serde_json::Value,
) -> EyreResult<calimero_primitives::application::ApplicationId> {
    use calimero_context_config::repr::{Repr, ReprBytes};
    use calimero_context_config::types::ApplicationId as ConfigApplicationId;

    let id_val = app_json
        .get("id")
        .ok_or_else(|| eyre::eyre!("missing 'id' in target_application"))?;
    let repr: Repr<ConfigApplicationId> = serde_json::from_value(id_val.clone())
        .map_err(|e| eyre::eyre!("invalid application id encoding: {e}"))?;
    Ok(calimero_primitives::application::ApplicationId::from(
        repr.as_bytes(),
    ))
}

/// Scans all GroupUpgradeKey entries and returns (group_id, upgrade_value)
/// pairs where status is InProgress. Used for crash recovery on startup.
pub fn enumerate_in_progress_upgrades(
    store: &Store,
) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
    let keys = collect_keys_with_prefix(
        store,
        GroupUpgradeKey::new([0u8; 32]),
        GROUP_UPGRADE_PREFIX,
        |_| true,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        if let Some(upgrade) = handle.get(&key)? {
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                results.push((ContextGroupId::from(key.group_id()), upgrade));
            }
        }
    }
    Ok(results)
}
