use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupLocalGovNonce, GroupMemberContext, GroupOpHead, GroupOpHeadValue, GroupOpLog,
    GROUP_MEMBER_CONTEXT_PREFIX, GROUP_OP_LOG_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{
    collect_keys_with_prefix, delete_all_context_last_migrations, delete_all_group_signing_keys,
    delete_all_member_aliases, delete_all_member_capabilities, delete_default_capabilities,
    delete_default_visibility, delete_group_alias, delete_group_meta, delete_group_upgrade,
    list_group_members, remove_group_member,
};

pub fn get_local_gov_nonce(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
) -> EyreResult<Option<u64>> {
    let handle = store.handle();
    let key = GroupLocalGovNonce::new(group_id.to_bytes(), *signer);
    Ok(handle.get(&key)?)
}

pub fn set_local_gov_nonce(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    nonce: u64,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupLocalGovNonce::new(group_id.to_bytes(), *signer);
    handle.put(&key, &nonce)?;
    Ok(())
}

fn delete_local_gov_nonce_for_signer(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupLocalGovNonce::new(group_id.to_bytes(), *signer);
    handle.delete(&key)?;
    Ok(())
}

/// Delete all [`GroupLocalGovNonce`] rows for current group members (best-effort; ignores missing).
fn delete_local_gov_nonces_for_listed_members(
    store: &Store,
    group_id: &ContextGroupId,
    members: &[(PublicKey, GroupMemberRole)],
) -> EyreResult<()> {
    for (pk, _) in members {
        if let Err(err) = delete_local_gov_nonce_for_signer(store, group_id, pk) {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                member = %pk,
                ?err,
                "best-effort nonce cleanup failed"
            );
        }
    }
    Ok(())
}

pub fn get_op_head(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupOpHeadValue>> {
    let handle = store.handle();
    let key = GroupOpHead::new(group_id.to_bytes());
    handle.get(&key).map_err(Into::into)
}

pub(crate) fn set_op_head(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    dag_heads: Vec<[u8; 32]>,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupOpHead::new(group_id.to_bytes());
    handle.put(
        &key,
        &GroupOpHeadValue {
            sequence,
            dag_heads,
        },
    )?;
    Ok(())
}

pub(crate) fn append_op_log_entry(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    op_bytes: &[u8],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupOpLog::new(group_id.to_bytes(), sequence);
    handle.put(&key, &op_bytes.to_vec())?;
    Ok(())
}

pub fn read_op_log_after(
    store: &Store,
    group_id: &ContextGroupId,
    after_sequence: u64,
    limit: usize,
) -> EyreResult<Vec<(u64, Vec<u8>)>> {
    let gid = group_id.to_bytes();
    let start_seq = after_sequence.saturating_add(1);
    let keys = collect_keys_with_prefix(
        store,
        GroupOpLog::new(gid, start_seq),
        GROUP_OP_LOG_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys.into_iter().take(limit) {
        let Some(op_bytes) = handle.get(&key)? else {
            continue;
        };
        results.push((key.sequence(), op_bytes));
    }

    Ok(results)
}

fn delete_op_log_and_head(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    const BATCH_SIZE: usize = 1000;
    let mut after_sequence = 0u64;
    loop {
        let batch = read_op_log_after(store, group_id, after_sequence, BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
        let mut handle = store.handle();
        for (seq, _) in batch {
            let key = GroupOpLog::new(group_id.to_bytes(), seq);
            handle.delete(&key)?;
            after_sequence = seq;
        }
    }
    let mut handle = store.handle();
    let head_key = GroupOpHead::new(group_id.to_bytes());
    handle.delete(&head_key)?;
    Ok(())
}

pub fn track_member_context_join(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    context_id: &ContextId,
    context_identity: [u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMemberContext::new(group_id.to_bytes(), *member, *context_id);
    handle.put(&key, &context_identity)?;
    Ok(())
}

pub fn get_member_context_joins(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    let gid = group_id.to_bytes();
    let member_pk = *member;
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberContext::new(gid, member_pk, ContextId::from([0u8; 32])),
        GROUP_MEMBER_CONTEXT_PREFIX,
        |k| k.group_id() == gid && k.member() == member_pk,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys {
        let Some(ctx_identity) = handle.get(&key)? else {
            continue;
        };
        results.push((key.context_id(), ctx_identity));
    }

    Ok(results)
}

pub fn remove_all_member_context_joins(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    let joins = get_member_context_joins(store, group_id, member)?;
    let mut handle = store.handle();
    for (context_id, _) in &joins {
        let key = GroupMemberContext::new(group_id.to_bytes(), *member, *context_id);
        handle.delete(&key)?;
    }
    Ok(joins)
}

/// Remove all local rows for a group (metadata, members, caps, aliases, ...).
/// Caller must enforce admin authorization and `count_group_contexts == 0`.
pub fn delete_group_local_rows(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let members_snapshot = list_group_members(store, group_id, 0, usize::MAX)?;
    delete_local_gov_nonces_for_listed_members(store, group_id, &members_snapshot)?;

    for (pk, _) in &members_snapshot {
        if let Err(err) = remove_all_member_context_joins(store, group_id, pk) {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                member = %pk,
                ?err,
                "best-effort member-context cleanup failed"
            );
        }
    }

    for (identity, _) in &members_snapshot {
        remove_group_member(store, group_id, identity)?;
    }

    delete_all_member_capabilities(store, group_id)?;
    delete_all_member_aliases(store, group_id)?;
    delete_default_capabilities(store, group_id)?;
    delete_default_visibility(store, group_id)?;
    delete_group_alias(store, group_id)?;
    delete_all_context_last_migrations(store, group_id)?;
    delete_group_upgrade(store, group_id)?;
    delete_all_group_signing_keys(store, group_id)?;
    delete_op_log_and_head(store, group_id)?;
    delete_group_meta(store, group_id)?;
    Ok(())
}
