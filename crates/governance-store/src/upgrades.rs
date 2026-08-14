use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{
    GroupFleetCompletion, GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue,
    GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

/// Typed Repository for per-group upgrade state.
///
/// Holds a single `GroupUpgradeValue` per group (save/load/delete)
/// plus a workspace-wide scan for in-progress upgrades (used by
/// crash-recovery on startup).
///
/// Issue #2303 / epic #2300.
pub struct UpgradesRepository<'a> {
    store: &'a Store,
}

impl<'a> UpgradesRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Writing the record clears [`Self::fleet_completed_at`]: that stamp is an
    /// observation about the record as last written, and the next migration is
    /// recorded by overwriting this one (the lazy path writes `Completed`
    /// directly, with no `InProgress` in between).
    pub fn save(&self, group_id: &ContextGroupId, upgrade: &GroupUpgradeValue) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(&GroupUpgradeKey::new(group_id.to_bytes()), upgrade)?;
        handle.delete(&GroupFleetCompletion::new(group_id.to_bytes()))?;
        Ok(())
    }

    pub fn load(&self, group_id: &ContextGroupId) -> EyreResult<Option<GroupUpgradeValue>> {
        let handle = self.store.handle();
        let key = GroupUpgradeKey::new(group_id.to_bytes());
        Ok(handle.get(&key)?)
    }

    pub fn delete(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupUpgradeKey::new(group_id.to_bytes()))?;
        handle.delete(&GroupFleetCompletion::new(group_id.to_bytes()))?;
        Ok(())
    }

    /// When this node watched the whole cohort converge on the stored record's
    /// `to_state_version`, or `None` while that is still outstanding.
    pub fn fleet_completed_at(&self, group_id: &ContextGroupId) -> EyreResult<Option<u64>> {
        let handle = self.store.handle();
        Ok(handle.get(&GroupFleetCompletion::new(group_id.to_bytes()))?)
    }

    pub fn set_fleet_completed_at(&self, group_id: &ContextGroupId, at: u64) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.put(&GroupFleetCompletion::new(group_id.to_bytes()), &at)?;
        Ok(())
    }

    /// Scans all `GroupUpgradeKey` entries and returns
    /// `(group_id, upgrade_value)` pairs where status is `InProgress`.
    /// Used for crash recovery on startup.
    pub fn enumerate_in_progress(&self) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
        let keys = collect_keys_with_prefix(
            self.store,
            GroupUpgradeKey::new([0u8; 32]),
            GROUP_UPGRADE_PREFIX,
            |_| true,
        )?;
        let handle = self.store.handle();
        let mut results = Vec::new();
        for key in keys {
            let Some(upgrade) = handle.get(&key)? else {
                continue;
            };
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                results.push((ContextGroupId::from(key.group_id()), upgrade));
            }
        }
        Ok(results)
    }
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

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PublicKey;

    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};

    fn sample_upgrade(status: GroupUpgradeStatus) -> GroupUpgradeValue {
        GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration: None,
            initiated_at: 1_700_000_000,
            initiated_by: PublicKey::from([0x01; 32]),
            status,
            cascade_hlc: None,
            cascade_seq: None,
            to_state_version: 2,
        }
    }

    #[test]
    fn load_returns_none_when_unset() {
        let store = test_store();
        let repo = UpgradesRepository::new(&store);
        assert!(repo.load(&test_group_id()).unwrap().is_none());
    }

    #[test]
    fn save_then_load_round_trip() {
        let store = test_store();
        let repo = UpgradesRepository::new(&store);
        let gid = test_group_id();
        let upgrade = sample_upgrade(GroupUpgradeStatus::InProgress {
            total: 5,
            completed: 0,
            failed: 0,
        });
        repo.save(&gid, &upgrade).unwrap();
        let loaded = repo.load(&gid).unwrap().expect("upgrade must round-trip");
        assert_eq!(loaded.from_version, upgrade.from_version);
        assert_eq!(loaded.to_version, upgrade.to_version);
    }

    #[test]
    fn delete_clears_existing_upgrade() {
        let store = test_store();
        let repo = UpgradesRepository::new(&store);
        let gid = test_group_id();
        repo.save(
            &gid,
            &sample_upgrade(GroupUpgradeStatus::Completed { completed_at: None }),
        )
        .unwrap();
        repo.delete(&gid).unwrap();
        assert!(repo.load(&gid).unwrap().is_none());
    }

    #[test]
    fn enumerate_in_progress_filters_by_status() {
        let store = test_store();
        let repo = UpgradesRepository::new(&store);
        let gid_progress = test_group_id();
        let gid_completed = ContextGroupId::from([0xCC; 32]);
        repo.save(
            &gid_progress,
            &sample_upgrade(GroupUpgradeStatus::InProgress {
                total: 5,
                completed: 0,
                failed: 0,
            }),
        )
        .unwrap();
        repo.save(
            &gid_completed,
            &sample_upgrade(GroupUpgradeStatus::Completed { completed_at: None }),
        )
        .unwrap();
        let in_progress = repo.enumerate_in_progress().unwrap();
        assert_eq!(in_progress.len(), 1);
        assert_eq!(in_progress[0].0, gid_progress);
    }

    /// Both readers of this key answer an undecodable row the same way: loudly.
    /// Tolerating it in one of them makes that reader report "no upgrade
    /// record", which every migration caller reads as "no migration in flight" -
    /// a false green over a record that says otherwise.
    #[test]
    fn an_undecodable_record_fails_loud_in_both_readers() {
        use calimero_store::layer::WriteLayer;
        use calimero_store::slice::Slice;

        let mut store = test_store();
        let gid = test_group_id();
        store
            .put(
                &GroupUpgradeKey::new(gid.to_bytes()),
                Slice::from(&b"not a GroupUpgradeValue"[..]),
            )
            .unwrap();

        let repo = UpgradesRepository::new(&store);
        assert!(repo.load(&gid).is_err(), "load must not report Ok(None)");
        assert!(repo.enumerate_in_progress().is_err());
    }

    /// The stamp answers "did this node watch the cohort converge on the record
    /// as it now stands", so writing the record retires it: the next migration
    /// is recorded by overwriting this one, and a surviving stamp would both
    /// report the old convergence and latch the new completion shut.
    #[test]
    fn writing_the_record_clears_the_fleet_stamp() {
        let store = test_store();
        let repo = UpgradesRepository::new(&store);
        let gid = test_group_id();
        repo.save(
            &gid,
            &sample_upgrade(GroupUpgradeStatus::Completed { completed_at: None }),
        )
        .unwrap();
        repo.set_fleet_completed_at(&gid, 1_700_002_000).unwrap();
        assert_eq!(repo.fleet_completed_at(&gid).unwrap(), Some(1_700_002_000));

        // A second migration, recorded the way the lazy path records one.
        repo.save(
            &gid,
            &sample_upgrade(GroupUpgradeStatus::Completed { completed_at: None }),
        )
        .unwrap();
        assert_eq!(repo.fleet_completed_at(&gid).unwrap(), None);

        repo.set_fleet_completed_at(&gid, 1_700_003_000).unwrap();
        repo.delete(&gid).unwrap();
        assert_eq!(repo.fleet_completed_at(&gid).unwrap(), None);
    }
}
