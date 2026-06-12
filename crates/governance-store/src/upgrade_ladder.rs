use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{GroupUpgradeLadder, LadderRung, UpgradeLadderValue};
use calimero_store::Store;
use eyre::Result as EyreResult;

/// Typed repository for the per-group upgrade ladder: the ordered upgrade
/// targets the group has moved through, captured as fold state whenever an
/// upgrade op advances `GroupMeta.app_key`. A context behind the group
/// replays these rungs in order, each in that release's own bytecode.
pub struct UpgradeLadderRepository<'a> {
    store: &'a Store,
}

impl<'a> UpgradeLadderRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// The group's rungs in causal-application order. Empty when no upgrade
    /// has ever applied.
    pub fn load(&self, group_id: &ContextGroupId) -> EyreResult<Vec<LadderRung>> {
        let handle = self.store.handle();
        let key = GroupUpgradeLadder::new(group_id.to_bytes());
        Ok(handle
            .get(&key)?
            .map(|value: UpgradeLadderValue| value.rungs)
            .unwrap_or_default())
    }

    /// Append a rung. Skips when the last rung carries the same `app_key`,
    /// which makes re-application of the same op (governance replay) a no-op
    /// while still recording legitimate A→B→A sequences.
    pub fn append(&self, group_id: &ContextGroupId, rung: LadderRung) -> EyreResult<()> {
        let mut rungs = self.load(group_id)?;
        if rungs
            .last()
            .is_some_and(|last| last.app_key == rung.app_key)
        {
            return Ok(());
        }
        rungs.push(rung);
        let mut handle = self.store.handle();
        let key = GroupUpgradeLadder::new(group_id.to_bytes());
        handle.put(&key, &UpgradeLadderValue { rungs })?;
        Ok(())
    }

    /// Remove the group's ladder (group teardown).
    pub fn delete(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupUpgradeLadder::new(group_id.to_bytes());
        handle.delete(&key)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};
    use calimero_primitives::application::ApplicationId;

    fn rung(byte: u8) -> LadderRung {
        LadderRung {
            app_key: [byte; 32],
            application_id: ApplicationId::from([0xCC; 32]),
        }
    }

    #[test]
    fn load_is_empty_when_unset() {
        let store = test_store();
        let repo = UpgradeLadderRepository::new(&store);
        assert!(repo.load(&test_group_id()).unwrap().is_empty());
    }

    #[test]
    fn append_preserves_order() {
        let store = test_store();
        let repo = UpgradeLadderRepository::new(&store);
        let gid = test_group_id();

        repo.append(&gid, rung(0x01)).unwrap();
        repo.append(&gid, rung(0x02)).unwrap();

        assert_eq!(repo.load(&gid).unwrap(), vec![rung(0x01), rung(0x02)]);
    }

    #[test]
    fn consecutive_duplicate_append_is_skipped() {
        // The same op re-applied (governance replay) must not double a rung.
        let store = test_store();
        let repo = UpgradeLadderRepository::new(&store);
        let gid = test_group_id();

        repo.append(&gid, rung(0x01)).unwrap();
        repo.append(&gid, rung(0x01)).unwrap();

        assert_eq!(repo.load(&gid).unwrap(), vec![rung(0x01)]);
    }

    #[test]
    fn non_consecutive_repeat_is_recorded() {
        // A legitimate A→B→A sequence (e.g. a code-only re-pin of an older
        // blob) is three real rungs, not a dedup case.
        let store = test_store();
        let repo = UpgradeLadderRepository::new(&store);
        let gid = test_group_id();

        repo.append(&gid, rung(0x01)).unwrap();
        repo.append(&gid, rung(0x02)).unwrap();
        repo.append(&gid, rung(0x01)).unwrap();

        assert_eq!(
            repo.load(&gid).unwrap(),
            vec![rung(0x01), rung(0x02), rung(0x01)]
        );
    }

    #[test]
    fn delete_removes_ladder() {
        let store = test_store();
        let repo = UpgradeLadderRepository::new(&store);
        let gid = test_group_id();

        repo.append(&gid, rung(0x01)).unwrap();
        repo.delete(&gid).unwrap();

        assert!(repo.load(&gid).unwrap().is_empty());
    }
}
