use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, GroupContextIndex, GroupMember, GroupMeta, GroupMetaValue,
    GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue, GROUP_CONTEXT_INDEX_PREFIX,
    GROUP_MEMBER_PREFIX, GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

// ---------------------------------------------------------------------------
// Group meta helpers
// ---------------------------------------------------------------------------

pub fn load_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupMetaValue>> {
    let handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn save_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.put(&key, meta)?;
    Ok(())
}

pub fn delete_group_meta(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.delete(&key)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Group member helpers
// ---------------------------------------------------------------------------

pub fn add_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    role: GroupMemberRole,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    handle.put(&key, &role)?;
    Ok(())
}

pub fn remove_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    handle.delete(&key)?;
    Ok(())
}

pub fn get_group_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let exists = handle.has(&key)?;
    Ok(exists)
}

pub fn is_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    match get_group_member_role(store, group_id, identity)? {
        Some(GroupMemberRole::Admin) => Ok(true),
        _ => Ok(false),
    }
}

pub fn require_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    if !is_group_admin(store, group_id, identity)? {
        bail!("requester is not an admin of group '{group_id:?}'");
    }
    Ok(())
}

// TODO: replace with iter.entries() for a single-pass scan once the
// Iter::read() / Iter::next() borrow-conflict (read takes &'a self) is
// resolved in the store API — currently each value requires a separate
// handle.get() lookup after collecting the key.
pub fn count_group_admins(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMember::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupMember>()?;
    let first = iter.seek(start_key).transpose();
    let mut count = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        let role = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        if role == GroupMemberRole::Admin {
            count += 1;
        }
    }

    Ok(count)
}

pub fn list_group_members(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMember::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupMember>()?;
    let first_key = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    // TODO: replace with iter.entries() for a single-pass scan once the
    // Iter::read() / Iter::next() borrow-conflict (read takes &'a self) is
    // resolved in the store API — currently each value requires a separate
    // handle.get() lookup after collecting the key.
    for key_result in first_key.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_MEMBER_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        let role = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        results.push((key.identity(), role));
    }

    Ok(results)
}

pub fn count_group_members(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMember::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupMember>()?;
    let first = iter.seek(start_key).transpose();
    let mut count = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Context-group index helpers
// ---------------------------------------------------------------------------

pub fn register_context_in_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();

    // If already registered in a different group, remove the stale index entry
    // to prevent orphaned counts and enumerations for the old group.
    let ref_key = ContextGroupRef::new(*context_id);
    if let Some(existing_group_bytes) = handle.get(&ref_key)? {
        if existing_group_bytes != group_id_bytes {
            let old_idx = GroupContextIndex::new(existing_group_bytes, *context_id);
            handle.delete(&old_idx)?;
        }
    }

    let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
    handle.put(&idx_key, &())?;
    handle.put(&ref_key, &group_id_bytes)?;

    Ok(())
}

pub fn unregister_context_from_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();

    let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
    handle.delete(&idx_key)?;

    let ref_key = ContextGroupRef::new(*context_id);
    handle.delete(&ref_key)?;

    Ok(())
}

pub fn get_group_for_context(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<ContextGroupId>> {
    let handle = store.handle();
    let key = ContextGroupRef::new(*context_id);
    let value = handle.get(&key)?;
    Ok(value.map(ContextGroupId::from))
}

pub fn enumerate_group_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<ContextId>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextIndex::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_INDEX_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        results.push(key.context_id());
    }

    Ok(results)
}

pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextIndex::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut count = 0usize;

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_INDEX_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Group upgrade helpers
// ---------------------------------------------------------------------------

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

/// Scans all GroupUpgradeKey entries and returns (group_id, upgrade_value)
/// pairs where status is InProgress. Used for crash recovery on startup.
pub fn enumerate_in_progress_upgrades(
    store: &Store,
) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
    let handle = store.handle();
    let start_key = GroupUpgradeKey::new([0u8; 32]);

    let mut iter = handle.iter::<GroupUpgradeKey>()?;
    let first = iter.seek(start_key).transpose();

    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_UPGRADE_PREFIX {
            break;
        }

        if let Some(upgrade) = handle.get(&key)? {
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                let group_id = ContextGroupId::from(key.group_id());
                results.push((group_id, upgrade));
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
    use calimero_store::Store;

    use super::*;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn test_group_id() -> ContextGroupId {
        ContextGroupId::from([0xAA; 32])
    }

    fn test_meta() -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0xBB; 32],
            target_application_id: ApplicationId::from([0xCC; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: PublicKey::from([0x01; 32]),
        }
    }

    // -----------------------------------------------------------------------
    // Group meta tests
    // -----------------------------------------------------------------------

    #[test]
    fn save_load_delete_group_meta() {
        let store = test_store();
        let gid = test_group_id();
        let meta = test_meta();

        assert!(load_group_meta(&store, &gid).unwrap().is_none());

        save_group_meta(&store, &gid, &meta).unwrap();
        let loaded = load_group_meta(&store, &gid).unwrap().unwrap();
        assert_eq!(loaded.app_key, meta.app_key);
        assert_eq!(loaded.target_application_id, meta.target_application_id);

        delete_group_meta(&store, &gid).unwrap();
        assert!(load_group_meta(&store, &gid).unwrap().is_none());
    }

    // -----------------------------------------------------------------------
    // Member tests
    // -----------------------------------------------------------------------

    #[test]
    fn add_and_check_membership() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        assert!(!check_group_membership(&store, &gid, &pk).unwrap());

        add_group_member(&store, &gid, &pk, GroupMemberRole::Admin).unwrap();
        assert!(check_group_membership(&store, &gid, &pk).unwrap());
        assert!(is_group_admin(&store, &gid, &pk).unwrap());
    }

    #[test]
    fn remove_member() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x02; 32]);

        add_group_member(&store, &gid, &pk, GroupMemberRole::Member).unwrap();
        assert!(check_group_membership(&store, &gid, &pk).unwrap());

        remove_group_member(&store, &gid, &pk).unwrap();
        assert!(!check_group_membership(&store, &gid, &pk).unwrap());
    }

    #[test]
    fn get_member_role() {
        let store = test_store();
        let gid = test_group_id();
        let admin = PublicKey::from([0x01; 32]);
        let member = PublicKey::from([0x02; 32]);

        add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
        add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

        assert_eq!(
            get_group_member_role(&store, &gid, &admin).unwrap(),
            Some(GroupMemberRole::Admin)
        );
        assert_eq!(
            get_group_member_role(&store, &gid, &member).unwrap(),
            Some(GroupMemberRole::Member)
        );
        assert!(!is_group_admin(&store, &gid, &member).unwrap());
    }

    #[test]
    fn require_group_admin_rejects_non_admin() {
        let store = test_store();
        let gid = test_group_id();
        let member = PublicKey::from([0x03; 32]);

        add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
        assert!(require_group_admin(&store, &gid, &member).is_err());
    }

    #[test]
    fn count_members_and_admins() {
        let store = test_store();
        let gid = test_group_id();

        assert_eq!(count_group_members(&store, &gid).unwrap(), 0);
        assert_eq!(count_group_admins(&store, &gid).unwrap(), 0);

        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x01; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x02; 32]),
            GroupMemberRole::Member,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x03; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();

        assert_eq!(count_group_members(&store, &gid).unwrap(), 3);
        assert_eq!(count_group_admins(&store, &gid).unwrap(), 2);
    }

    #[test]
    fn list_members_with_offset_and_limit() {
        let store = test_store();
        let gid = test_group_id();

        for i in 0u8..5 {
            let mut pk_bytes = [0u8; 32];
            pk_bytes[0] = i;
            add_group_member(
                &store,
                &gid,
                &PublicKey::from(pk_bytes),
                GroupMemberRole::Member,
            )
            .unwrap();
        }

        let all = list_group_members(&store, &gid, 0, 100).unwrap();
        assert_eq!(all.len(), 5);

        let page = list_group_members(&store, &gid, 1, 2).unwrap();
        assert_eq!(page.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Context-group index tests
    // -----------------------------------------------------------------------

    #[test]
    fn register_and_unregister_context() {
        let store = test_store();
        let gid = test_group_id();
        let cid = ContextId::from([0x11; 32]);

        assert!(get_group_for_context(&store, &cid).unwrap().is_none());

        register_context_in_group(&store, &gid, &cid).unwrap();
        assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid);

        unregister_context_from_group(&store, &gid, &cid).unwrap();
        assert!(get_group_for_context(&store, &cid).unwrap().is_none());
    }

    #[test]
    fn re_register_context_cleans_old_group() {
        let store = test_store();
        let gid1 = ContextGroupId::from([0x01; 32]);
        let gid2 = ContextGroupId::from([0x02; 32]);
        let cid = ContextId::from([0x11; 32]);

        register_context_in_group(&store, &gid1, &cid).unwrap();
        assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 1);

        register_context_in_group(&store, &gid2, &cid).unwrap();
        assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 0);
        assert_eq!(count_group_contexts(&store, &gid2).unwrap(), 1);
        assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid2);
    }

    #[test]
    fn enumerate_and_count_contexts() {
        let store = test_store();
        let gid = test_group_id();

        for i in 0u8..4 {
            let mut cid_bytes = [0u8; 32];
            cid_bytes[0] = i;
            register_context_in_group(&store, &gid, &ContextId::from(cid_bytes)).unwrap();
        }

        assert_eq!(count_group_contexts(&store, &gid).unwrap(), 4);

        let page = enumerate_group_contexts(&store, &gid, 1, 2).unwrap();
        assert_eq!(page.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Upgrade tests
    // -----------------------------------------------------------------------

    #[test]
    fn save_load_delete_upgrade() {
        let store = test_store();
        let gid = test_group_id();

        assert!(load_group_upgrade(&store, &gid).unwrap().is_none());

        let upgrade = GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration: None,
            initiated_at: 1_700_000_000,
            initiated_by: PublicKey::from([0x01; 32]),
            status: GroupUpgradeStatus::InProgress {
                total: 5,
                completed: 0,
                failed: 0,
            },
        };

        save_group_upgrade(&store, &gid, &upgrade).unwrap();
        let loaded = load_group_upgrade(&store, &gid).unwrap().unwrap();
        assert_eq!(loaded.from_version, "1.0.0");
        assert_eq!(loaded.to_version, "2.0.0");

        delete_group_upgrade(&store, &gid).unwrap();
        assert!(load_group_upgrade(&store, &gid).unwrap().is_none());
    }

    #[test]
    fn enumerate_in_progress_upgrades_filters_completed() {
        let store = test_store();
        let gid_in_progress = ContextGroupId::from([0x01; 32]);
        let gid_completed = ContextGroupId::from([0x02; 32]);

        save_group_upgrade(
            &store,
            &gid_in_progress,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::InProgress {
                    total: 5,
                    completed: 2,
                    failed: 0,
                },
            },
        )
        .unwrap();

        save_group_upgrade(
            &store,
            &gid_completed,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::Completed {
                    completed_at: 1_700_001_000,
                },
            },
        )
        .unwrap();

        let results = enumerate_in_progress_upgrades(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, gid_in_progress);
    }
}
