use calimero_context_config::types::{
    ContextGroupId, GroupRevealPayloadData, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, ContextIdentity, GroupAlias, GroupChildIndex, GroupContextAlias,
    GroupContextIndex, GroupContextLastMigration, GroupContextLastMigrationValue,
    GroupContextMemberCap, GroupDefaultCaps, GroupDefaultCapsValue, GroupDefaultVis,
    GroupDefaultVisValue, GroupInvitationCommitment, GroupInvitationCommitmentValue,
    GroupLocalGovNonce, GroupMember, GroupMemberAlias, GroupMemberCapability,
    GroupMemberCapabilityValue, GroupMemberContext, GroupMemberValue, GroupMeta, GroupMetaValue,
    GroupOpHead, GroupOpHeadValue, GroupOpLog, GroupParentRef, GroupSigningKey,
    GroupSigningKeyValue, GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue,
    NamespaceIdentity, NamespaceIdentityValue, GROUP_CONTEXT_INDEX_PREFIX,
    GROUP_CONTEXT_LAST_MIGRATION_PREFIX, GROUP_MEMBER_ALIAS_PREFIX,
    GROUP_MEMBER_CAPABILITY_PREFIX, GROUP_MEMBER_CONTEXT_PREFIX, GROUP_MEMBER_PREFIX,
    GROUP_META_PREFIX, GROUP_OP_LOG_PREFIX, GROUP_SIGNING_KEY_PREFIX, GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::{Digest, Sha256};

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

pub fn enumerate_all_groups(
    store: &Store,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
    let handle = store.handle();
    let start_key = GroupMeta::new([0u8; 32]);
    let mut iter = handle.iter::<GroupMeta>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_META_PREFIX {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        let Some(meta) = handle.get(&key)? else {
            continue;
        };
        results.push((key.group_id(), meta));
    }

    Ok(results)
}

/// Compute a deterministic SHA-256 hash of the group's authorization-relevant state.
///
/// Covers members (sorted by public key) + roles + admin identity + target application.
/// This hash is embedded in each [`SignedGroupOp`] to ensure ops can only apply against
/// the exact state they were signed against, preventing divergence from concurrent ops.
pub fn compute_group_state_hash(store: &Store, group_id: &ContextGroupId) -> EyreResult<[u8; 32]> {
    let meta = load_group_meta(store, group_id)?
        .ok_or_else(|| eyre::eyre!("group not found for state hash computation"))?;

    let mut members = list_group_members(store, group_id, 0, usize::MAX)?;
    members.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    hasher.update(group_id.to_bytes());
    hasher.update(AsRef::<[u8]>::as_ref(&meta.admin_identity));
    hasher.update(meta.target_application_id.as_ref());
    for (pk, role) in &members {
        hasher.update(AsRef::<[u8]>::as_ref(pk));
        hasher.update(&borsh::to_vec(role).unwrap_or_default());
    }
    Ok(hasher.finalize().into())
}

/// Namespace governance epoch: just the namespace DAG heads.
///
/// With the single-DAG model, governance epoch is simply the heads of
/// the namespace DAG that contains this context's group.
pub fn compute_namespace_governance_epoch(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Vec<[u8; 32]>> {
    let Some(owning_gid) = get_group_for_context(store, context_id)? else {
        return Ok(vec![]);
    };

    let ns_id = resolve_namespace(store, &owning_gid)?;
    let ns_key = calimero_store::key::NamespaceGovHead::new(ns_id.to_bytes());
    let handle = store.handle();
    match handle.get(&ns_key)? {
        Some(head) => Ok(head.dag_heads),
        None => {
            // Fall back to the old per-group DAG heads during migration.
            get_op_head(store, &owning_gid)?
                .map(|h| Ok(h.dag_heads))
                .unwrap_or_else(|| Ok(vec![]))
        }
    }
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
    add_group_member_with_keys(store, group_id, identity, role, None, None)
}

pub fn add_group_member_with_keys(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    role: GroupMemberRole,
    private_key: Option<[u8; 32]>,
    sender_key: Option<[u8; 32]>,
) -> EyreResult<()> {
    let is_admin = role == GroupMemberRole::Admin;
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    handle.put(
        &key,
        &GroupMemberValue {
            role,
            private_key,
            sender_key,
        },
    )?;
    drop(handle);

    if !is_admin {
        if let Some(defaults) = get_default_capabilities(store, group_id)? {
            if defaults != 0 {
                set_member_capability(store, group_id, identity, defaults)?;
            }
        }
    }

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
        let _ = delete_local_gov_nonce_for_signer(store, group_id, pk);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Op log — persistent, ordered log of applied SignedGroupOps per group
// ---------------------------------------------------------------------------

pub fn get_op_head(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupOpHeadValue>> {
    let handle = store.handle();
    let key = GroupOpHead::new(group_id.to_bytes());
    handle.get(&key).map_err(Into::into)
}

fn set_op_head(
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

fn append_op_log_entry(
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
    let handle = store.handle();
    let start_seq = after_sequence.saturating_add(1);
    let start_key = GroupOpLog::new(group_id.to_bytes(), start_seq);
    let mut iter = handle.iter::<GroupOpLog>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_OP_LOG_PREFIX {
            break;
        }

        if key.group_id() != group_id.to_bytes() {
            break;
        }

        if results.len() >= limit {
            break;
        }

        let Some(op_bytes) = handle.get(&key)? else {
            continue;
        };
        results.push((key.sequence(), op_bytes));
    }

    Ok(results)
}

fn delete_op_log_and_head(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    const BATCH_SIZE: usize = 1000;
    loop {
        let batch = read_op_log_after(store, group_id, 0, BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
        let mut handle = store.handle();
        for (seq, _) in &batch {
            let key = GroupOpLog::new(group_id.to_bytes(), *seq);
            handle.delete(&key)?;
        }
    }
    let mut handle = store.handle();
    let head_key = GroupOpHead::new(group_id.to_bytes());
    handle.delete(&head_key)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Member-context join tracking (for cascade removal)
// ---------------------------------------------------------------------------

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
    let handle = store.handle();
    let start_key =
        GroupMemberContext::new(group_id.to_bytes(), *member, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberContext>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_MEMBER_CONTEXT_PREFIX {
            break;
        }
        if key.group_id() != group_id.to_bytes() {
            break;
        }
        if key.member() != *member {
            break;
        }

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

/// Remove all local rows for a group (metadata, members, caps, aliases, …).  
/// Caller must enforce admin authorization and `count_group_contexts == 0`.
pub fn delete_group_local_rows(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let members_snapshot = list_group_members(store, group_id, 0, usize::MAX)?;
    delete_local_gov_nonces_for_listed_members(store, group_id, &members_snapshot)?;

    for (pk, _) in &members_snapshot {
        let _ = remove_all_member_context_joins(store, group_id, pk);
    }

    loop {
        let batch = list_group_members(store, group_id, 0, 500)?;
        if batch.is_empty() {
            break;
        }
        for (identity, _role) in &batch {
            remove_group_member(store, group_id, identity)?;
        }
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

fn ensure_not_last_admin_removal(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    if !is_group_admin(store, group_id, member)? {
        return Ok(());
    }
    if count_group_admins(store, group_id)? > 1 {
        return Ok(());
    }
    bail!("cannot remove the last admin of the group");
}

fn ensure_not_last_admin_demotion(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    new_role: &GroupMemberRole,
) -> EyreResult<()> {
    if *new_role == GroupMemberRole::Admin {
        return Ok(());
    }
    if !is_group_admin(store, group_id, member)? {
        return Ok(());
    }
    if count_group_admins(store, group_id)? > 1 {
        return Ok(());
    }
    bail!("cannot demote the last admin of the group");
}

/// Maximum number of parent hashes allowed in a single [`SignedGroupOp`].
/// Chosen to allow realistic merge breadth (multi-admin concurrent ops) while
/// bounding memory/CPU cost during signature verification and storage.
const MAX_PARENT_OP_HASHES: usize = 256;

/// Maximum DAG heads before forcing a synthetic merge. Prevents unbounded
/// growth from many concurrent admins operating without merges.
const MAX_DAG_HEADS: usize = 64;

/// Apply a [`SignedGroupOp`] to the local group store (signature, monotonic nonce, admin rules).
///
/// # Concurrency
///
/// Callers must serialize access per `group_id`. In the node this is guaranteed
/// by the single-threaded actix `ContextManager` actor which processes messages
/// sequentially. Direct concurrent calls from multiple threads for the same
/// group are **not** safe and could produce duplicate sequence numbers.
///
/// # Parent validation
///
/// `parent_op_hashes` are not validated against the current `dag_heads` — an op
/// may reference ancestors further back in the DAG. This is acceptable because
/// authorization is checked against the *current* group state (not the parent
/// state), and the `DagStore` performs topological ordering independently.
pub fn apply_local_signed_group_op(store: &Store, op: &SignedGroupOp) -> EyreResult<()> {
    if op.parent_op_hashes.len() > MAX_PARENT_OP_HASHES {
        bail!(
            "too many parent_op_hashes ({}, max {})",
            op.parent_op_hashes.len(),
            MAX_PARENT_OP_HASHES
        );
    }
    op.verify_signature()
        .map_err(|e| eyre::eyre!("signed group op: {e}"))?;
    let group_id = ContextGroupId::from(op.group_id);

    let zero_hash = [0u8; 32];
    if op.state_hash != zero_hash {
        let current_state_hash = compute_group_state_hash(store, &group_id)?;
        if op.state_hash != current_state_hash {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                expected = %hex::encode(op.state_hash),
                actual = %hex::encode(current_state_hash),
                nonce = op.nonce,
                signer = %op.signer,
                "rejecting op: state_hash mismatch (signed against stale state)"
            );
            bail!(
                "state_hash mismatch: op was signed against {}, current state is {}",
                hex::encode(op.state_hash),
                hex::encode(current_state_hash)
            );
        }
    }

    let last = get_local_gov_nonce(store, &group_id, &op.signer)?.unwrap_or(0);
    if op.nonce <= last {
        tracing::debug!(
            nonce = op.nonce,
            last_nonce = last,
            signer = %op.signer,
            "ignoring op with already-processed nonce"
        );
        return Ok(());
    }

    match &op.op {
        GroupOp::Noop => {}
        GroupOp::MemberAdded { member, role } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "add member",
            )?;
            if *role == GroupMemberRole::Admin && !is_group_admin(store, &group_id, &op.signer)? {
                bail!("only admins can add new admins");
            }
            add_group_member(store, &group_id, member, role.clone())?;
        }
        GroupOp::MemberRemoved { member } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "remove member",
            )?;
            if is_group_admin(store, &group_id, member)?
                && !is_group_admin(store, &group_id, &op.signer)?
            {
                bail!("only admins can remove other admins");
            }
            ensure_not_last_admin_removal(store, &group_id, member)?;

            cascade_remove_member_from_group_tree(store, &group_id, member)?;

            remove_group_member(store, &group_id, member)?;
        }
        GroupOp::MemberRoleSet { member, role } => {
            require_group_admin(store, &group_id, &op.signer)?;
            ensure_not_last_admin_demotion(store, &group_id, member, role)?;
            add_group_member(store, &group_id, member, role.clone())?;
        }
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => {
            require_group_admin(store, &group_id, &op.signer)?;
            set_member_capability(store, &group_id, member, *capabilities)?;
        }
        GroupOp::DefaultCapabilitiesSet { capabilities } => {
            require_group_admin(store, &group_id, &op.signer)?;
            set_default_capabilities(store, &group_id, *capabilities)?;
        }
        GroupOp::UpgradePolicySet { policy } => {
            require_group_admin(store, &group_id, &op.signer)?;
            let mut meta = load_group_meta(store, &group_id)?
                .ok_or_else(|| eyre::eyre!("group metadata not found"))?;
            meta.upgrade_policy = policy.clone();
            save_group_meta(store, &group_id, &meta)?;
        }
        GroupOp::TargetApplicationSet {
            app_key,
            target_application_id,
        } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_APPLICATION,
                "set target application",
            )?;
            let mut meta = load_group_meta(store, &group_id)?
                .ok_or_else(|| eyre::eyre!("group metadata not found"))?;
            meta.app_key = *app_key;
            meta.target_application_id = *target_application_id;
            save_group_meta(store, &group_id, &meta)?;

            cascade_target_application(store, &group_id, target_application_id, app_key)?;
        }
        GroupOp::ContextRegistered { context_id } => {
            if !is_group_admin_or_has_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::CAN_CREATE_CONTEXT,
            )? {
                bail!("only group admin or members with CAN_CREATE_CONTEXT can register a context");
            }
            register_context_in_group(store, &group_id, context_id)?;
        }
        GroupOp::ContextDetached { context_id } => {
            require_group_admin(store, &group_id, &op.signer)?;
            match get_group_for_context(store, context_id)? {
                Some(g) if g == group_id => {
                    unregister_context_from_group(store, &group_id, context_id)?;
                }
                Some(_) => bail!("context is registered to a different group"),
                None => bail!("context is not registered in any group"),
            }
        }
        GroupOp::DefaultVisibilitySet { mode } => {
            require_group_admin(store, &group_id, &op.signer)?;
            if *mode > 1 {
                bail!("visibility mode must be 0 (Open) or 1 (Restricted), got {mode}");
            }
            set_default_visibility(store, &group_id, *mode)?;
        }
        GroupOp::ContextAliasSet { context_id, alias } => {
            require_group_admin(store, &group_id, &op.signer)?;
            set_context_alias(store, &group_id, context_id, alias)?;
        }
        GroupOp::MemberAliasSet { member, alias } => {
            let is_admin = is_group_admin(store, &group_id, &op.signer)?;
            if !is_admin && op.signer != *member {
                bail!("only group admin or the member can set member alias");
            }
            set_member_alias(store, &group_id, member, alias)?;
        }
        GroupOp::GroupAliasSet { alias } => {
            require_group_admin(store, &group_id, &op.signer)?;
            set_group_alias(store, &group_id, alias)?;
        }
        GroupOp::GroupDelete => {
            require_group_admin(store, &group_id, &op.signer)?;
            if count_group_contexts(store, &group_id)? > 0 {
                bail!("cannot delete group: one or more contexts are still registered");
            }
            delete_group_local_rows(store, &group_id)?;
        }
        GroupOp::GroupMigrationSet { migration } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_APPLICATION,
                "set group migration",
            )?;
            let mut meta = load_group_meta(store, &group_id)?
                .ok_or_else(|| eyre::eyre!("group metadata not found"))?;
            meta.migration = migration.clone();
            save_group_meta(store, &group_id, &meta)?;
        }
        GroupOp::InvitationCommitted { .. } | GroupOp::JoinWithInvitationClaim { .. } => {
            tracing::debug!(
                "InvitationCommitted/JoinWithInvitationClaim deprecated; \
                 use RootOp::MemberJoined on namespace topic instead"
            );
        }
        GroupOp::ContextCapabilityGranted {
            context_id,
            member,
            capability,
        } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "grant context capability",
            )?;
            let current =
                get_context_member_capability(store, &group_id, context_id, member)?.unwrap_or(0);
            set_context_member_capability(
                store,
                &group_id,
                context_id,
                member,
                current | capability,
            )?;
        }
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member,
            capability,
        } => {
            require_group_admin_or_capability(
                store,
                &group_id,
                &op.signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "revoke context capability",
            )?;
            let current =
                get_context_member_capability(store, &group_id, context_id, member)?.unwrap_or(0);
            set_context_member_capability(
                store,
                &group_id,
                context_id,
                member,
                current & !capability,
            )?;
        }
        GroupOp::SubgroupCreated { .. } | GroupOp::SubgroupRemoved { .. } => {
            tracing::debug!("SubgroupCreated/Removed ops are deprecated in namespace model");
        }
        GroupOp::TeeAdmissionPolicySet { .. } => {
            require_group_admin(store, &group_id, &op.signer)?;
            // Policy is persisted in the governance DAG itself (via op log).
            // Peers read TeeAdmissionPolicySet ops from the DAG to reconstruct policy.
        }
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash: _,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
        } => {
            if !check_group_membership(store, &group_id, &op.signer)? {
                bail!("TEE attestation verifier must be a group member");
            }
            let policy = read_tee_admission_policy(store, &group_id)?
                .ok_or_else(|| eyre::eyre!(
                    "MemberJoinedViaTeeAttestation rejected: no TeeAdmissionPolicySet exists for group"
                ))?;
            if !policy.allowed_mrtd.is_empty() && !policy.allowed_mrtd.iter().any(|a| a == mrtd) {
                bail!("MemberJoinedViaTeeAttestation rejected: MRTD not in policy allowlist");
            }
            if !policy.allowed_tcb_statuses.is_empty()
                && !policy.allowed_tcb_statuses.iter().any(|a| a == tcb_status)
            {
                bail!("MemberJoinedViaTeeAttestation rejected: TCB status not in policy allowlist");
            }
            for (allowlist, actual, label) in [
                (&policy.allowed_rtmr0, rtmr0, "RTMR0"),
                (&policy.allowed_rtmr1, rtmr1, "RTMR1"),
                (&policy.allowed_rtmr2, rtmr2, "RTMR2"),
                (&policy.allowed_rtmr3, rtmr3, "RTMR3"),
            ] {
                if !allowlist.is_empty() && !allowlist.iter().any(|a| a == actual) {
                    bail!(
                        "MemberJoinedViaTeeAttestation rejected: {label} not in policy allowlist"
                    );
                }
            }
            if !check_group_membership(store, &group_id, member)? {
                add_group_member(store, &group_id, member, role.clone())?;
            }
        }
        #[allow(unreachable_patterns)]
        _ => bail!("unsupported group op variant for local apply"),
    }

    let content_hash = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let head = get_op_head(store, &group_id)?;
    let next_seq = head.as_ref().map_or(1, |h| h.sequence.saturating_add(1));
    let op_bytes = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    append_op_log_entry(store, &group_id, next_seq, &op_bytes)?;

    let parent_set: std::collections::HashSet<[u8; 32]> =
        op.parent_op_hashes.iter().copied().collect();
    let mut new_heads: Vec<[u8; 32]> = head
        .map(|h| h.dag_heads)
        .unwrap_or_default()
        .into_iter()
        .filter(|h| !parent_set.contains(h))
        .collect();
    new_heads.push(content_hash);
    if new_heads.len() > MAX_DAG_HEADS {
        let excess = new_heads.len() - MAX_DAG_HEADS;
        tracing::warn!(
            group_id = %hex::encode(group_id.to_bytes()),
            dropped = excess,
            remaining = MAX_DAG_HEADS,
            "DAG heads exceeded cap, dropping oldest heads"
        );
        new_heads.drain(..excess);
    }
    set_op_head(store, &group_id, next_seq, new_heads)?;

    set_local_gov_nonce(store, &group_id, &op.signer, op.nonce)?;

    Ok(())
}

/// Output of [`sign_apply_local_group_op_borsh`] for publishing via gossip.
pub struct SignedOpOutput {
    pub bytes: Vec<u8>,
    pub delta_id: [u8; 32],
    pub parent_ids: Vec<[u8; 32]>,
}

/// Sign the next monotonic [`SignedGroupOp`] for `signer_sk`, apply via [`apply_local_signed_group_op`],
/// and return a [`SignedOpOutput`] for
/// [`calimero_node_primitives::client::NodeClient::publish_signed_group_op`].
pub fn sign_apply_local_group_op_borsh(
    store: &Store,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    op: GroupOp,
) -> EyreResult<SignedOpOutput> {
    let last = get_local_gov_nonce(store, group_id, &signer_sk.public_key())?.unwrap_or(0);
    let nonce = last
        .checked_add(1)
        .ok_or_else(|| eyre::eyre!("group governance nonce overflow"))?;
    let parent_hashes = get_op_head(store, group_id)?
        .map(|h| h.dag_heads.clone())
        .unwrap_or_default();
    let state_hash = compute_group_state_hash(store, group_id)?;
    let signed = SignedGroupOp::sign(
        signer_sk,
        group_id.to_bytes(),
        parent_hashes,
        state_hash,
        nonce,
        op,
    )?;
    let delta_id = signed
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let parent_ids = signed.parent_op_hashes.clone();
    apply_local_signed_group_op(store, &signed)?;
    let bytes = borsh::to_vec(&signed).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    Ok(SignedOpOutput {
        bytes,
        delta_id,
        parent_ids,
    })
}

/// Sign, apply locally, and publish a group op to the network in one step.
///
/// Combines [`sign_apply_local_group_op_borsh`] + [`calimero_node_primitives::client::NodeClient::publish_signed_group_op`].
pub async fn sign_apply_and_publish(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    op: GroupOp,
) -> EyreResult<()> {
    let output = sign_apply_local_group_op_borsh(store, group_id, signer_sk, op)?;
    node_client
        .publish_signed_group_op(
            group_id.to_bytes(),
            output.delta_id,
            output.parent_ids,
            output.bytes,
        )
        .await
}

// ---------------------------------------------------------------------------
// Namespace governance ops (Phase 2)
// ---------------------------------------------------------------------------

use calimero_context_primitives::local_governance::{
    EncryptedGroupOp, NamespaceOp, OpaqueSkeleton, RootOp, SignedNamespaceOp,
};

/// Encrypt a [`GroupOp`] with the group's sender key to produce an
/// [`EncryptedGroupOp`] for inclusion in a [`NamespaceOp::Group`].
pub fn encrypt_group_op(
    sender_key_bytes: &[u8; 32],
    op: &GroupOp,
) -> EyreResult<EncryptedGroupOp> {
    use calimero_crypto::SharedKey;

    let plaintext = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh encode GroupOp: {e}"))?;
    let sk = calimero_primitives::identity::PrivateKey::from(*sender_key_bytes);
    let shared_key = SharedKey::from_sk(&sk);

    let nonce: [u8; 12] = {
        use rand::Rng;
        rand::thread_rng().gen()
    };

    let ciphertext = shared_key
        .encrypt(plaintext, nonce)
        .ok_or_else(|| eyre::eyre!("AES-GCM encryption failed"))?;

    Ok(EncryptedGroupOp { nonce, ciphertext })
}

/// Apply a [`SignedNamespaceOp`] to the local store.
///
/// - `RootOp` variants are applied in cleartext (group creation/deletion, admin changes).
/// - `Group { .. }` variants: if we hold the group's sender key we decrypt
///   and delegate to the existing group-op logic; otherwise we store an
///   [`OpaqueSkeleton`].
pub fn apply_signed_namespace_op(store: &Store, op: &SignedNamespaceOp) -> EyreResult<()> {
    op.verify_signature()
        .map_err(|e| eyre::eyre!("signed namespace op: {e}"))?;

    match &op.op {
        NamespaceOp::Root(root) => apply_root_op(store, op, root),
        NamespaceOp::Group {
            group_id,
            encrypted,
        } => {
            let group_id_typed = ContextGroupId::from(*group_id);

            // Try to find a sender_key for this group by looking up the local
            // node's namespace identity and checking its group membership.
            let ns_id = ContextGroupId::from(op.namespace_id);
            let sender_key_bytes = get_namespace_identity(store, &ns_id)?
                .and_then(|(local_pk, _, _)| {
                    get_group_member_value(store, &group_id_typed, &local_pk)
                        .ok()
                        .flatten()
                })
                .and_then(|member_val| member_val.sender_key);

            match sender_key_bytes {
                Some(sk_bytes) => {
                    decrypt_and_apply_group_op(store, op, &group_id_typed, &sk_bytes, encrypted)?;
                    Ok(())
                }
                None => {
                    store_opaque_skeleton(store, op)?;
                    Ok(())
                }
            }
        }
    }
}

/// Decrypt an [`EncryptedGroupOp`] and apply the inner [`GroupOp`] via the
/// existing group governance logic, then persist the op in the namespace
/// governance log.
fn decrypt_and_apply_group_op(
    store: &Store,
    ns_op: &SignedNamespaceOp,
    group_id: &ContextGroupId,
    sender_key_bytes: &[u8; 32],
    encrypted: &calimero_context_primitives::local_governance::EncryptedGroupOp,
) -> EyreResult<()> {
    use calimero_context_primitives::local_governance::GroupOp;
    use calimero_crypto::SharedKey;

    let sk = calimero_primitives::identity::PrivateKey::from(*sender_key_bytes);
    let shared_key = SharedKey::from_sk(&sk);

    let plaintext = shared_key
        .decrypt(encrypted.ciphertext.clone(), encrypted.nonce)
        .ok_or_else(|| eyre::eyre!("failed to decrypt group op (bad sender_key or corrupt)"))?;

    let inner_op: GroupOp =
        borsh::from_slice(&plaintext).map_err(|e| eyre::eyre!("borsh decode inner GroupOp: {e}"))?;

    // Build a synthetic SignedGroupOp so the existing apply path works.
    // The signature was already verified on the outer SignedNamespaceOp.
    let signed_group_op = calimero_context_primitives::local_governance::SignedGroupOp {
        version: calimero_context_primitives::local_governance::SIGNED_GROUP_OP_SCHEMA_VERSION,
        group_id: group_id.to_bytes(),
        parent_op_hashes: ns_op.parent_op_hashes.clone(),
        state_hash: ns_op.state_hash,
        signer: ns_op.signer,
        nonce: ns_op.nonce,
        op: inner_op,
        signature: ns_op.signature,
    };

    // Apply using the existing group governance logic. We skip signature
    // verification inside apply_local_signed_group_op because the version
    // field won't match (it's a namespace op version, not group). Instead
    // we apply the inner mutation directly.
    apply_group_op_inner(store, group_id, &ns_op.signer, ns_op.nonce, &signed_group_op.op)?;

    // Also store the full op in the namespace gov log for persistence.
    store_opaque_skeleton(store, ns_op)?;

    Ok(())
}

/// Apply a `GroupOp` mutation to the store, extracted from
/// `apply_local_signed_group_op` but without re-verifying the signature
/// (already verified on the outer `SignedNamespaceOp`).
fn apply_group_op_inner(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    nonce: u64,
    op: &calimero_context_primitives::local_governance::GroupOp,
) -> EyreResult<()> {
    use calimero_context_config::MemberCapabilities;
    use calimero_context_primitives::local_governance::GroupOp;

    let last = get_local_gov_nonce(store, group_id, signer)?.unwrap_or(0);
    if nonce <= last {
        tracing::debug!(
            nonce,
            last_nonce = last,
            signer = %signer,
            "ignoring namespace group op with already-processed nonce"
        );
        return Ok(());
    }

    match op {
        GroupOp::Noop => {}
        GroupOp::MemberAdded { member, role } => {
            require_group_admin_or_capability(
                store,
                group_id,
                signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "add member",
            )?;
            if *role == calimero_primitives::context::GroupMemberRole::Admin
                && !is_group_admin(store, group_id, signer)?
            {
                bail!("only admins can add new admins");
            }
            add_group_member(store, group_id, member, role.clone())?;
        }
        GroupOp::MemberRemoved { member } => {
            require_group_admin_or_capability(
                store,
                group_id,
                signer,
                MemberCapabilities::MANAGE_MEMBERS,
                "remove member",
            )?;
            if is_group_admin(store, group_id, member)?
                && !is_group_admin(store, group_id, signer)?
            {
                bail!("only admins can remove other admins");
            }
            ensure_not_last_admin_removal(store, group_id, member)?;
            cascade_remove_member_from_group_tree(store, group_id, member)?;
            remove_group_member(store, group_id, member)?;
        }
        GroupOp::MemberRoleSet { member, role } => {
            require_group_admin(store, group_id, signer)?;
            ensure_not_last_admin_demotion(store, group_id, member, role)?;
            add_group_member(store, group_id, member, role.clone())?;
        }
        GroupOp::ContextRegistered { context_id } => {
            if !is_group_admin_or_has_capability(
                store,
                group_id,
                signer,
                MemberCapabilities::CAN_CREATE_CONTEXT,
            )? {
                bail!("only group admin or members with CAN_CREATE_CONTEXT can register a context");
            }
            register_context_in_group(store, group_id, context_id)?;
        }
        GroupOp::ContextDetached { context_id } => {
            require_group_admin(store, group_id, signer)?;
            match get_group_for_context(store, context_id)? {
                Some(g) if g == *group_id => {
                    unregister_context_from_group(store, group_id, context_id)?;
                }
                Some(_) => bail!("context is registered to a different group"),
                None => bail!("context is not registered in any group"),
            }
        }
        _ => {
            tracing::debug!(
                ?op,
                "namespace group op variant not handled by inner apply, stored as skeleton"
            );
        }
    }

    set_local_gov_nonce(store, group_id, signer, nonce)?;

    Ok(())
}

/// Apply a cleartext root operation to the store.
fn apply_root_op(store: &Store, op: &SignedNamespaceOp, root: &RootOp) -> EyreResult<()> {
    match root {
        RootOp::GroupCreated { group_id } => {
            let gid = ContextGroupId::from(*group_id);
            if load_group_meta(store, &gid)?.is_some() {
                tracing::debug!(
                    group_id = %hex::encode(group_id),
                    "group already exists, ignoring GroupCreated"
                );
                return Ok(());
            }
            let meta = GroupMetaValue {
                admin_identity: op.signer,
                target_application_id: calimero_primitives::application::ApplicationId::from([0u8; 32]),
                app_key: [0u8; 32],
                upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
                migration: None,
                created_at: 0,
                auto_join: false,
            };
            save_group_meta(store, &gid, &meta)?;
            Ok(())
        }
        RootOp::GroupDeleted { group_id } => {
            let gid = ContextGroupId::from(*group_id);
            if count_group_contexts(store, &gid)? > 0 {
                bail!("cannot delete group: contexts still registered");
            }
            delete_group_meta(store, &gid)?;
            Ok(())
        }
        RootOp::AdminChanged { new_admin } => {
            let ns_id = op.namespace_id;
            let ns_gid = ContextGroupId::from(ns_id);
            let mut meta = load_group_meta(store, &ns_gid)?
                .ok_or_else(|| eyre::eyre!("namespace root group not found"))?;
            meta.admin_identity = *new_admin;
            save_group_meta(store, &ns_gid, &meta)?;
            Ok(())
        }
        RootOp::PolicyUpdated { .. } => {
            tracing::debug!("PolicyUpdated: stored in DAG log, no additional state mutation");
            Ok(())
        }
        RootOp::GroupNested {
            parent_group_id,
            child_group_id,
        } => {
            let parent = ContextGroupId::from(*parent_group_id);
            let child = ContextGroupId::from(*child_group_id);
            if load_group_meta(store, &parent)?.is_none() {
                bail!("parent group not found for nesting");
            }
            if load_group_meta(store, &child)?.is_none() {
                bail!("child group not found for nesting");
            }
            nest_group(store, &parent, &child)?;
            tracing::info!(
                parent = %hex::encode(parent_group_id),
                child = %hex::encode(child_group_id),
                "group nested"
            );
            Ok(())
        }
        RootOp::GroupUnnested {
            parent_group_id,
            child_group_id,
        } => {
            let parent = ContextGroupId::from(*parent_group_id);
            let child = ContextGroupId::from(*child_group_id);
            unnest_group(store, &parent, &child)?;
            tracing::info!(
                parent = %hex::encode(parent_group_id),
                child = %hex::encode(child_group_id),
                "group unnested"
            );
            Ok(())
        }
        RootOp::MemberJoined {
            member,
            signed_invitation,
        } => {
            let inv = &signed_invitation.invitation;
            let group_id = inv.group_id;

            // 1. The namespace op signer must be the member being added
            //    (proves key ownership).
            if op.signer != *member {
                bail!(
                    "MemberJoined signer ({}) does not match member ({})",
                    op.signer,
                    member
                );
            }

            // 2. Verify the admin's signature on the invitation.
            let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
            let invitation_bytes =
                borsh::to_vec(&inv).map_err(|e| eyre::eyre!("borsh: {e}"))?;
            let hash = sha2::Sha256::digest(&invitation_bytes);
            let sig_bytes = hex::decode(&signed_invitation.inviter_signature)
                .map_err(|e| eyre::eyre!("bad invitation signature hex: {e}"))?;
            let sig_arr: [u8; 64] = sig_bytes
                .try_into()
                .map_err(|_| eyre::eyre!("invitation signature wrong length"))?;
            inviter_pk
                .verify_raw_signature(&hash, &sig_arr)
                .map_err(|e| eyre::eyre!("invalid invitation signature: {e}"))?;

            // 3. Verify the inviter is an admin of the target group.
            if !is_group_admin(store, &group_id, &inviter_pk)? {
                bail!(
                    "invitation inviter {} is not an admin of group {:?}",
                    inviter_pk,
                    group_id
                );
            }

            // 4. Check expiration.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if inv.expiration_timestamp != 0 && now > inv.expiration_timestamp {
                bail!("invitation expired");
            }

            // 5. Check not already a member.
            if check_group_membership(store, &group_id, member)? {
                tracing::debug!(
                    member = %member,
                    "member already in group, ignoring MemberJoined"
                );
                return Ok(());
            }

            // 6. Map invited_role (u8) → GroupMemberRole.
            let role = match inv.invited_role {
                0 => calimero_primitives::context::GroupMemberRole::Admin,
                2 => calimero_primitives::context::GroupMemberRole::ReadOnly,
                _ => calimero_primitives::context::GroupMemberRole::Member,
            };

            add_group_member(store, &group_id, member, role)?;

            tracing::info!(
                member = %member,
                group_id = %hex::encode(group_id.to_bytes()),
                invited_by = %inviter_pk,
                "member joined group via namespace MemberJoined op"
            );

            Ok(())
        }
    }
}

/// Store an opaque skeleton for a namespace op we cannot (or need not) decrypt.
fn store_opaque_skeleton(store: &Store, op: &SignedNamespaceOp) -> EyreResult<()> {
    let delta_id = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;

    let group_id = match &op.op {
        NamespaceOp::Group { group_id, .. } => *group_id,
        NamespaceOp::Root(_) => op.namespace_id,
    };

    let skeleton = OpaqueSkeleton {
        delta_id,
        parent_op_hashes: op.parent_op_hashes.clone(),
        group_id,
        signer: op.signer,
    };

    let key = calimero_store::key::NamespaceGovOp::new(op.namespace_id, delta_id);
    let value = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&skeleton).map_err(|e| eyre::eyre!("borsh: {e}"))?,
    };
    let mut handle = store.handle();
    handle.put(&key, &value)?;
    Ok(())
}

/// Sign, apply locally, and publish a namespace governance op.
///
/// Combines signing a [`SignedNamespaceOp`], applying it to the local store,
/// and publishing on the `ns/<id>` gossip topic.
pub async fn sign_apply_and_publish_namespace_op(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    namespace_id: [u8; 32],
    signer_sk: &PrivateKey,
    op: NamespaceOp,
) -> EyreResult<()> {
    let ns_head_key = calimero_store::key::NamespaceGovHead::new(namespace_id);
    let handle = store.handle();
    let head = handle.get(&ns_head_key)?;
    drop(handle);

    let parent_hashes = head
        .as_ref()
        .map(|h| h.dag_heads.clone())
        .unwrap_or_default();
    let nonce = head.as_ref().map_or(1, |h| h.sequence.saturating_add(1));

    let signed = SignedNamespaceOp::sign(signer_sk, namespace_id, parent_hashes, [0u8; 32], nonce, op)?;
    let delta_id = signed
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    let parent_ids = signed.parent_op_hashes.clone();

    apply_signed_namespace_op(store, &signed)?;

    // Update namespace DAG head
    let mut new_heads: Vec<[u8; 32]> = head
        .map(|h| h.dag_heads)
        .unwrap_or_default()
        .into_iter()
        .filter(|h| !parent_ids.contains(h))
        .collect();
    new_heads.push(delta_id);

    let mut wh = store.handle();
    let ns_head_key = calimero_store::key::NamespaceGovHead::new(namespace_id);
    wh.put(
        &ns_head_key,
        &calimero_store::key::NamespaceGovHeadValue {
            sequence: nonce,
            dag_heads: new_heads,
        },
    )?;
    drop(wh);

    let bytes = borsh::to_vec(&signed).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    node_client
        .publish_signed_namespace_op(namespace_id, delta_id, parent_ids, bytes)
        .await
}

/// Collect delta IDs from namespace governance skeletons that belong to a
/// specific group. These are ops we stored as opaque skeletons because we
/// didn't hold the group's sender key at the time; now that we've joined
/// the group, we can request the full payloads from peers.
pub fn collect_skeleton_delta_ids_for_group(
    store: &Store,
    namespace_id: [u8; 32],
    group_id: [u8; 32],
) -> EyreResult<Vec<[u8; 32]>> {
    let handle = store.handle();
    let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
    let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
    let first = iter.seek(start).transpose();
    let mut delta_ids = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.namespace_id() != namespace_id {
            break;
        }
        if let Some(value) = handle.get(&key)? {
            if let Ok(skeleton) =
                borsh::from_slice::<OpaqueSkeleton>(&value.skeleton_bytes)
            {
                if skeleton.group_id == group_id {
                    delta_ids.push(skeleton.delta_id);
                }
            }
        }
    }

    Ok(delta_ids)
}

/// Returns the member's effective role, walking up the ancestor chain.
/// Direct membership takes priority; if not found, checks parent groups.
/// Returns the most privileged role found (Admin > Member > ReadOnly).
pub fn get_group_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    get_direct_member_role(store, group_id, identity)
}

pub fn get_group_member_value(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberValue>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.get(&key)?)
}

/// Returns `true` if the member has a `ReadOnly` role in the group that owns this context.
/// Returns `false` if the context has no group, the member is not found, or the member
/// has `Admin` or `Member` role.
pub fn is_read_only_for_context(
    store: &Store,
    context_id: &calimero_primitives::context::ContextId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    match get_group_member_role(store, &group_id, identity)? {
        Some(GroupMemberRole::ReadOnly) => Ok(true),
        _ => Ok(false),
    }
}

/// Read the parent group for a given group (legacy, used by `resolve_namespace`).
pub fn get_parent_group(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<ContextGroupId>> {
    let handle = store.handle();
    let key = GroupParentRef::new(group_id.to_bytes());
    Ok(handle.get(&key)?.map(ContextGroupId::from))
}

// ---------------------------------------------------------------------------
// Group nesting (organizational metadata, no permission inheritance)
// ---------------------------------------------------------------------------

/// Record that `child` is nested inside `parent`. Both directions are stored
/// so we can query parent→children and child→parent.
///
/// Rejects the operation if it would create a cycle (child is already an
/// ancestor of parent) or if child already has a parent.
pub fn nest_group(
    store: &Store,
    parent_group_id: &ContextGroupId,
    child_group_id: &ContextGroupId,
) -> EyreResult<()> {
    if parent_group_id == child_group_id {
        bail!("cannot nest a group under itself");
    }

    if get_parent_group(store, child_group_id)?.is_some() {
        bail!(
            "group {:?} already has a parent; unnest it first",
            child_group_id
        );
    }

    // Walk up from parent to detect if child is already an ancestor of parent.
    let mut current = *parent_group_id;
    let mut depth = 0usize;
    while let Some(ancestor) = get_parent_group(store, &current)? {
        if ancestor == *child_group_id {
            bail!("nesting would create a cycle");
        }
        depth += 1;
        if depth > 256 {
            bail!("nesting depth exceeds 256, possible data corruption");
        }
        current = ancestor;
    }

    let mut handle = store.handle();
    let ref_key = GroupParentRef::new(child_group_id.to_bytes());
    handle.put(&ref_key, &parent_group_id.to_bytes())?;
    let idx_key = GroupChildIndex::new(parent_group_id.to_bytes(), child_group_id.to_bytes());
    handle.put(&idx_key, &())?;
    Ok(())
}

/// Remove a nesting relationship.
pub fn unnest_group(
    store: &Store,
    parent_group_id: &ContextGroupId,
    child_group_id: &ContextGroupId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let ref_key = GroupParentRef::new(child_group_id.to_bytes());
    handle.delete(&ref_key)?;
    let idx_key = GroupChildIndex::new(parent_group_id.to_bytes(), child_group_id.to_bytes());
    handle.delete(&idx_key)?;
    Ok(())
}

/// List all direct children of a group.
pub fn list_child_groups(
    store: &Store,
    parent_group_id: &ContextGroupId,
) -> EyreResult<Vec<ContextGroupId>> {
    let handle = store.handle();
    let parent_bytes = parent_group_id.to_bytes();
    let start_key = GroupChildIndex::new(parent_bytes, [0u8; 32]);
    let mut iter = handle.iter::<GroupChildIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != calimero_store::key::GROUP_CHILD_INDEX_PREFIX {
            break;
        }
        if key.parent_group_id() != parent_bytes {
            break;
        }
        results.push(ContextGroupId::from(key.child_group_id()));
    }

    Ok(results)
}

/// Collect ALL descendant group IDs (recursive BFS). Returns them in
/// breadth-first order, excluding the starting group itself.
pub fn collect_descendant_groups(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<ContextGroupId>> {
    let mut descendants = Vec::new();
    let mut queue = vec![*group_id];

    while let Some(current) = queue.pop() {
        let children = list_child_groups(store, &current)?;
        for child in children {
            descendants.push(child);
            queue.push(child);
        }
    }

    Ok(descendants)
}

/// Create invitations for a group AND all its descendant groups.
///
/// Returns a list of `(group_id, SignedGroupOpenInvitation)` pairs — one
/// per group the member is being invited to. The caller publishes a
/// `RootOp::MemberJoined` for each.
pub fn create_recursive_invitations(
    store: &Store,
    root_group_id: &ContextGroupId,
    inviter_sk: &PrivateKey,
    expiration_secs: u64,
    invited_role: u8,
) -> EyreResult<Vec<(ContextGroupId, calimero_context_config::types::SignedGroupOpenInvitation)>> {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };

    let mut groups = vec![*root_group_id];
    groups.extend(collect_descendant_groups(store, root_group_id)?);

    let inviter_signer_id = SignerId::from(*inviter_sk.public_key());
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiration = now_secs + expiration_secs;

    let mut result = Vec::with_capacity(groups.len());
    for gid in groups {
        let secret_salt: [u8; 32] = {
            use rand::Rng;
            rand::thread_rng().gen()
        };

        let invitation = GroupInvitationFromAdmin {
            inviter_identity: inviter_signer_id,
            group_id: gid,
            expiration_timestamp: expiration,
            secret_salt,
            invited_role,
        };

        let inv_bytes =
            borsh::to_vec(&invitation).map_err(|e| eyre::eyre!("borsh: {e}"))?;
        let hash = sha2::Sha256::digest(&inv_bytes);
        let sig = inviter_sk
            .sign(&hash)
            .map_err(|e| eyre::eyre!("signing: {e}"))?;

        let signed = SignedGroupOpenInvitation {
            invitation,
            inviter_signature: hex::encode(sig.to_bytes()),
        };

        result.push((gid, signed));
    }

    Ok(result)
}

/// Remove a member from a group AND all its descendant groups.
///
/// Returns the list of group IDs the member was removed from.
pub fn recursive_remove_member(
    store: &Store,
    root_group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Vec<ContextGroupId>> {
    let mut groups = vec![*root_group_id];
    groups.extend(collect_descendant_groups(store, root_group_id)?);

    let mut removed_from = Vec::new();
    for gid in &groups {
        if check_group_membership(store, gid, member)? {
            remove_group_member(store, gid, member)?;
            cascade_remove_member_from_group_tree(store, gid, member)?;
            removed_from.push(*gid);
        }
    }

    Ok(removed_from)
}

// ---------------------------------------------------------------------------
// Namespace identity: per-root-group node keypair
// ---------------------------------------------------------------------------

const MAX_NAMESPACE_DEPTH: usize = 16;

/// Walk the parent chain to find the root group (namespace).
/// Returns the root group id (the one with no parent).
pub fn resolve_namespace(store: &Store, group_id: &ContextGroupId) -> EyreResult<ContextGroupId> {
    let mut current = *group_id;
    for _ in 0..MAX_NAMESPACE_DEPTH {
        match get_parent_group(store, &current)? {
            Some(parent) => current = parent,
            None => return Ok(current),
        }
    }
    eyre::bail!(
        "namespace resolution exceeded max depth ({MAX_NAMESPACE_DEPTH}), possible circular reference"
    )
}

/// Read this node's identity for a namespace from the store.
pub fn get_namespace_identity(
    store: &Store,
    namespace_id: &ContextGroupId,
) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
    let handle = store.handle();
    let key = NamespaceIdentity::new(namespace_id.to_bytes());
    match handle.get(&key)? {
        Some(val) => Ok(Some((
            PublicKey::from(val.public_key),
            val.private_key,
            val.sender_key,
        ))),
        None => Ok(None),
    }
}

/// Store this node's identity for a namespace.
pub fn store_namespace_identity(
    store: &Store,
    namespace_id: &ContextGroupId,
    public_key: &PublicKey,
    private_key: &[u8; 32],
    sender_key: &[u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = NamespaceIdentity::new(namespace_id.to_bytes());
    handle.put(
        &key,
        &NamespaceIdentityValue {
            public_key: **public_key,
            private_key: *private_key,
            sender_key: *sender_key,
        },
    )?;
    Ok(())
}

/// Resolve the namespace for a group and return this node's identity.
/// Returns None if no identity has been stored for that namespace.
pub fn resolve_namespace_identity(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
    let ns_id = resolve_namespace(store, group_id)?;
    get_namespace_identity(store, &ns_id)
}

/// Resolve the namespace for a group and return this node's identity,
/// generating and storing a new keypair if none exists.
///
/// # Concurrency
///
/// The read-then-write pattern here is intentionally non-atomic. All callers
/// within the node run through the `ContextManager` actix actor, whose
/// single-threaded mailbox serializes message processing -- so concurrent
/// calls from different handlers never race. The one external call site
/// (`fleet_join.rs`) operates on a group that hasn't been admitted yet, so
/// no other handler is operating on the same namespace concurrently.
pub fn get_or_create_namespace_identity(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32], [u8; 32])> {
    let ns_id = resolve_namespace(store, group_id)?;
    if let Some((pk, sk, sender)) = get_namespace_identity(store, &ns_id)? {
        return Ok((ns_id, pk, sk, sender));
    }

    let private_key = calimero_primitives::identity::PrivateKey::random(&mut rand::thread_rng());
    let public_key = private_key.public_key();
    let sender_key = calimero_primitives::identity::PrivateKey::random(&mut rand::thread_rng());

    store_namespace_identity(store, &ns_id, &public_key, &private_key, &sender_key)?;

    Ok((ns_id, public_key, *private_key, *sender_key))
}

/// Cascade `target_application_id` and `app_key` to all descendant subgroups
/// starting from `group_id` downward (breadth-first, bounded by
/// `MAX_NAMESPACE_DEPTH`).
///
/// Called when a group's application is upgraded via governance. Each
/// descendant's metadata is updated in a separate write. If the process is
/// interrupted mid-cascade, some descendants may still reference the old
/// application -- the next upgrade attempt will bring them up to date.
// ---------------------------------------------------------------------------
// Invitation commitment helpers
// ---------------------------------------------------------------------------


/// In the namespace model, application cascading to child groups is handled
/// at the namespace governance level rather than via tree traversal.
fn cascade_target_application(
    _store: &Store,
    _group_id: &ContextGroupId,
    _target_application_id: &calimero_primitives::application::ApplicationId,
    _app_key: &[u8; 32],
) -> EyreResult<()> {
    Ok(())
}

fn has_direct_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.has(&key)?)
}

fn get_direct_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.get(&key)?.map(|v: GroupMemberValue| v.role))
}

pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    has_direct_member(store, group_id, identity)
}

/// Returns `true` if `identity` is a direct admin of this specific group
/// (no ancestor walk). Used for operations where inherited admin authority
/// should NOT apply (e.g., managing Restricted context allowlists).
pub fn is_direct_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    match get_direct_member_role(store, group_id, identity)? {
        Some(GroupMemberRole::Admin) => Ok(true),
        _ => Ok(false),
    }
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

/// Returns `true` if `identity` is a group admin **or** holds the given capability bit.
/// Admins always pass regardless of capability bits.
pub fn is_group_admin_or_has_capability(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    capability_bit: u32,
) -> EyreResult<bool> {
    if is_group_admin(store, group_id, identity)? {
        return Ok(true);
    }
    let caps = get_member_capability(store, group_id, identity)?.unwrap_or(0);
    Ok(caps & capability_bit != 0)
}

/// Enforces that `identity` is a group admin or holds the given capability bit.
pub fn require_group_admin_or_capability(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    capability_bit: u32,
    operation: &str,
) -> EyreResult<()> {
    if !is_group_admin_or_has_capability(store, group_id, identity, capability_bit)? {
        bail!(
            "requester lacks permission to {operation} in group '{group_id:?}' \
             (not an admin and capability bit 0x{capability_bit:x} is not set)"
        );
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
        let val: GroupMemberValue = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        if val.role == GroupMemberRole::Admin {
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

        let val: GroupMemberValue = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        results.push((key.identity(), val.role));
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

/// Scans the ContextIdentity column for the given context and returns the first
/// `PublicKey` for which the node holds a local private key. Used to find a
/// valid signer when performing group upgrades on behalf of a context that the
/// group admin may not be a member of.
pub fn find_local_signing_identity(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<PublicKey>> {
    let handle = store.handle();
    let start_key = ContextIdentity::new(*context_id, [0u8; 32].into());
    let mut iter = handle.iter::<ContextIdentity>()?;
    let first = iter.seek(start_key).transpose();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.context_id() != *context_id {
            break;
        }
        let Some(value) = handle.get(&key)? else {
            continue;
        };
        if value.private_key.is_some() {
            return Ok(Some(key.public_key()));
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Group signing key helpers
// ---------------------------------------------------------------------------

pub fn store_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
    private_key: &[u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    handle.put(
        &key,
        &GroupSigningKeyValue {
            private_key: *private_key,
        },
    )?;
    Ok(())
}

pub fn get_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<Option<[u8; 32]>> {
    let handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.private_key))
}

pub fn delete_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    handle.delete(&key)?;
    Ok(())
}

/// Verify that the node holds a signing key for the given requester in this group.
pub fn require_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    requester: &PublicKey,
) -> EyreResult<()> {
    if get_group_signing_key(store, group_id, requester)?.is_none() {
        bail!(
            "node does not hold a signing key for requester in group '{group_id:?}'; \
             register one via POST /admin-api/groups/<id>/signing-key"
        );
    }
    Ok(())
}

/// Delete all signing keys for a group (used during group deletion).
pub fn delete_all_group_signing_keys(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupSigningKey::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupSigningKey>()?;
    let first = iter.seek(start_key).transpose();

    let mut keys_to_delete = Vec::new();
    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_SIGNING_KEY_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        keys_to_delete.push(key);
    }
    drop(iter);

    let mut handle = store.handle();
    for key in keys_to_delete {
        handle.delete(&key)?;
    }

    Ok(())
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

fn cascade_remove_member_from_group_tree(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    let contexts = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    for context_id in &contexts {
        let mut handle = store.handle();
        let identity_key = ContextIdentity::new(*context_id, (*member).into());
        if handle.has(&identity_key)? {
            handle.delete(&identity_key)?;
            tracing::info!(
                group_id = %hex::encode(group_id.to_bytes()),
                context_id = %hex::encode(context_id.as_ref()),
                member = %member,
                "cascade-removed member from context"
            );
        }
    }

    Ok(())
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

/// Stores a human-readable alias for a context within a group.
pub fn set_context_alias(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    alias: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupContextAlias::new(group_id.to_bytes(), *context_id),
        &alias.to_owned(),
    )?;
    Ok(())
}

/// Returns the alias for a context within a group, if one was set.
pub fn get_context_alias(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupContextAlias::new(group_id.to_bytes(), *context_id))
        .map_err(Into::into)
}

/// Returns context IDs together with their optional aliases.
pub fn enumerate_group_contexts_with_aliases(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(ContextId, Option<String>)>> {
    let ids = enumerate_group_contexts(store, group_id, offset, limit)?;
    ids.into_iter()
        .map(|ctx_id| {
            let alias = get_context_alias(store, group_id, &ctx_id)?;
            Ok((ctx_id, alias))
        })
        .collect()
}

/// Stores a human-readable alias for a group member within a group.
pub fn set_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    alias: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupMemberAlias::new(group_id.to_bytes(), *member),
        &alias.to_owned(),
    )?;
    Ok(())
}

/// Returns the alias for a group member within a group, if one was set.
pub fn get_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupMemberAlias::new(group_id.to_bytes(), *member))
        .map_err(Into::into)
}

/// Stores a human-readable alias for the group itself.
pub fn set_group_alias(store: &Store, group_id: &ContextGroupId, alias: &str) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupAlias::new(group_id.to_bytes()), &alias.to_owned())?;
    Ok(())
}

/// Build a `NamespaceSummary` for a root group, fetching counts from the store.
///
/// Returns `None` if the group has a parent (not a namespace root) or if
/// `node_identity` is not a member.
pub fn build_namespace_summary(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    node_identity: &PublicKey,
) -> EyreResult<Option<calimero_context_primitives::group::NamespaceSummary>> {
    if get_parent_group(store, group_id)?.is_some() {
        return Ok(None);
    }
    if !check_group_membership(store, group_id, node_identity)? {
        return Ok(None);
    }

    let alias = get_group_alias(store, group_id).ok().flatten();
    let member_count = count_group_members(store, group_id).unwrap_or(0);
    let context_count = enumerate_group_contexts(store, group_id, 0, usize::MAX)
        .unwrap_or_default()
        .len();
    let subgroup_count = 0;

    Ok(Some(calimero_context_primitives::group::NamespaceSummary {
        namespace_id: *group_id,
        app_key: meta.app_key.into(),
        target_application_id: meta.target_application_id,
        upgrade_policy: meta.upgrade_policy.clone(),
        created_at: meta.created_at,
        alias,
        member_count,
        context_count,
        subgroup_count,
    }))
}

/// Returns the alias for a group, if one was set.
pub fn get_group_alias(store: &Store, group_id: &ContextGroupId) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupAlias::new(group_id.to_bytes()))
        .map_err(Into::into)
}

/// Returns all member aliases stored for a group as `(PublicKey, alias_string)` pairs.
pub fn enumerate_member_aliases(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, String)>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberAlias::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberAlias>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_ALIAS_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        let Some(alias) = handle.get(&key)? else {
            continue;
        };
        results.push((key.member(), alias));
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

#[cfg(test)]
fn extract_application_id(
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

// ---------------------------------------------------------------------------
// Group upgrade helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

pub fn get_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<u32>> {
    let handle = store.handle();
    let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.capabilities))
}

pub fn set_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    caps: u32,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
    handle.put(&key, &GroupMemberCapabilityValue { capabilities: caps })?;
    Ok(())
}

pub fn enumerate_member_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, u32)>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberCapability::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberCapability>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_MEMBER_CAPABILITY_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        let Some(val) = handle.get(&key)? else {
            continue;
        };

        results.push((PublicKey::from(*key.identity()), val.capabilities));
    }

    Ok(results)
}

pub fn get_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u32>> {
    let handle = store.handle();
    let key = GroupDefaultCaps::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.capabilities))
}

pub fn set_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
    caps: u32,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupDefaultCaps::new(group_id.to_bytes());
    handle.put(&key, &GroupDefaultCapsValue { capabilities: caps })?;
    Ok(())
}

pub fn get_default_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<Option<u8>> {
    let handle = store.handle();
    let key = GroupDefaultVis::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.mode))
}

pub fn set_default_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    mode: u8,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupDefaultVis::new(group_id.to_bytes());
    handle.put(&key, &GroupDefaultVisValue { mode })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-context migration tracking
// ---------------------------------------------------------------------------

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

pub fn delete_group_alias(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupAlias::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_default_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupDefaultCaps::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_default_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupDefaultVis::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_all_member_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberCapability::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberCapability>()?;
    let first = iter.seek(start_key).transpose();
    let mut keys_to_delete = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_CAPABILITY_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        keys_to_delete.push(key);
    }
    drop(iter);

    let mut handle = store.handle();
    for key in keys_to_delete {
        handle.delete(&key)?;
    }
    Ok(())
}

pub fn delete_all_member_aliases(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberAlias::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberAlias>()?;
    let first = iter.seek(start_key).transpose();
    let mut keys_to_delete = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_ALIAS_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        keys_to_delete.push(key);
    }
    drop(iter);

    let mut handle = store.handle();
    for key in keys_to_delete {
        handle.delete(&key)?;
    }
    Ok(())
}

pub fn delete_all_context_last_migrations(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key =
        GroupContextLastMigration::new(group_id_bytes, ContextId::from([0u8; 32]).into());
    let mut iter = handle.iter::<GroupContextLastMigration>()?;
    let first = iter.seek(start_key).transpose();
    let mut keys_to_delete = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_LAST_MIGRATION_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        keys_to_delete.push(key);
    }
    drop(iter);

    let mut handle = store.handle();
    for key in keys_to_delete {
        handle.delete(&key)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-context per-member capability helpers
// ---------------------------------------------------------------------------

pub fn set_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
    capabilities: u8,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
    handle.put(&key, &capabilities)?;
    Ok(())
}

pub fn get_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<Option<u8>> {
    let handle = store.handle();
    let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
    Ok(handle.get(&key)?)
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
            migration: None,
            auto_join: true,
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
    fn apply_local_signed_group_op_nonce_and_admin() {
        use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let mut rng = OsRng;
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();
        add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

        let member_pk = PrivateKey::random(&mut rng).public_key();

        let op1 = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member: member_pk,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op1).unwrap();
        assert!(check_group_membership(&store, &gid, &member_pk).unwrap());

        let op_dup_nonce =
            SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], [0u8; 32], 1, GroupOp::Noop).unwrap();
        assert!(
            apply_local_signed_group_op(&store, &op_dup_nonce).is_ok(),
            "duplicate nonce should be silently accepted (idempotent)"
        );

        let op2 =
            SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], [0u8; 32], 2, GroupOp::Noop).unwrap();
        apply_local_signed_group_op(&store, &op2).unwrap();

        let non_admin_sk = PrivateKey::random(&mut rng);
        add_group_member(
            &store,
            &gid,
            &non_admin_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();
        let op_bad = SignedGroupOp::sign(
            &non_admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member: PrivateKey::random(&mut rng).public_key(),
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();
        assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
    }

    #[test]
    fn apply_local_member_alias_member_signer_or_admin() {
        use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let mut rng = OsRng;
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();
        save_group_meta(&store, &gid, &test_meta()).unwrap();
        add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

        let member_sk = PrivateKey::random(&mut rng);
        let member_pk = member_sk.public_key();
        add_group_member(&store, &gid, &member_pk, GroupMemberRole::Member).unwrap();

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAliasSet {
                member: member_pk,
                alias: "alice".to_owned(),
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();
        assert_eq!(
            get_member_alias(&store, &gid, &member_pk)
                .unwrap()
                .as_deref(),
            Some("alice")
        );

        let other_sk = PrivateKey::random(&mut rng);
        let op_bad = SignedGroupOp::sign(
            &other_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAliasSet {
                member: member_pk,
                alias: "bob".to_owned(),
            },
        )
        .unwrap();
        assert!(apply_local_signed_group_op(&store, &op_bad).is_err());

        let admin_op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAliasSet {
                member: member_pk,
                alias: "carol".to_owned(),
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &admin_op).unwrap();
        assert_eq!(
            get_member_alias(&store, &gid, &member_pk)
                .unwrap()
                .as_deref(),
            Some("carol")
        );
    }

    #[test]
    fn apply_local_context_alias_admin_or_creator() {
        use calimero_context_config::MemberCapabilities;
        use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let mut rng = OsRng;
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();
        save_group_meta(&store, &gid, &test_meta()).unwrap();
        add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

        let creator_sk = PrivateKey::random(&mut rng);
        let creator_pk = creator_sk.public_key();
        add_group_member(&store, &gid, &creator_pk, GroupMemberRole::Member).unwrap();
        set_member_capability(
            &store,
            &gid,
            &creator_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )
        .unwrap();

        let context_id = ContextId::from([0x33; 32]);

        let op_reg = SignedGroupOp::sign(
            &creator_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::ContextRegistered { context_id },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op_reg).unwrap();

        let op_creator_alias = SignedGroupOp::sign(
            &creator_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            2,
            GroupOp::ContextAliasSet {
                context_id,
                alias: "from-creator".to_owned(),
            },
        )
        .unwrap();
        assert!(
            apply_local_signed_group_op(&store, &op_creator_alias).is_err(),
            "non-admin creator should be rejected"
        );

        let op_admin = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::ContextAliasSet {
                context_id,
                alias: "from-admin".to_owned(),
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op_admin).unwrap();
        assert_eq!(
            get_context_alias(&store, &gid, &context_id)
                .unwrap()
                .as_deref(),
            Some("from-admin")
        );
    }

    #[test]
    fn apply_local_signed_group_op_capabilities_upgrade_policy_and_delete() {
        use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let mut rng = OsRng;
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();

        save_group_meta(&store, &gid, &test_meta()).unwrap();
        add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

        let member_m = PrivateKey::random(&mut rng).public_key();
        add_group_member(&store, &gid, &member_m, GroupMemberRole::Member).unwrap();

        let op_caps = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberCapabilitySet {
                member: member_m,
                capabilities: 0x7,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op_caps).unwrap();
        assert_eq!(
            get_member_capability(&store, &gid, &member_m)
                .unwrap()
                .unwrap(),
            0x7
        );

        let op_policy = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            2,
            GroupOp::UpgradePolicySet {
                policy: UpgradePolicy::Automatic,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op_policy).unwrap();
        assert_eq!(
            load_group_meta(&store, &gid)
                .unwrap()
                .unwrap()
                .upgrade_policy,
            UpgradePolicy::Automatic
        );

        let op_del = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            3,
            GroupOp::GroupDelete,
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op_del).unwrap();
        assert!(load_group_meta(&store, &gid).unwrap().is_none());
    }

    #[test]
    fn apply_local_signed_group_op_rejects_last_admin_removal() {
        use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let mut rng = OsRng;
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_pk = admin_sk.public_key();

        save_group_meta(&store, &gid, &test_meta()).unwrap();
        add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

        let op_bad = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberRemoved { member: admin_pk },
        )
        .unwrap();
        assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
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
    // Signing key tests
    // -----------------------------------------------------------------------

    #[test]
    fn store_and_get_signing_key() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);
        let sk = [0xAA; 32];

        assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());

        store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
        let loaded = get_group_signing_key(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(loaded, sk);
    }

    #[test]
    fn delete_signing_key() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);
        let sk = [0xAA; 32];

        store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
        delete_group_signing_key(&store, &gid, &pk).unwrap();
        assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());
    }

    #[test]
    fn require_signing_key_fails_when_missing() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);

        assert!(require_group_signing_key(&store, &gid, &pk).is_err());
    }

    #[test]
    fn delete_all_group_signing_keys_removes_all() {
        let store = test_store();
        let gid = test_group_id();
        let pk1 = PublicKey::from([0x10; 32]);
        let pk2 = PublicKey::from([0x11; 32]);

        store_group_signing_key(&store, &gid, &pk1, &[0xAA; 32]).unwrap();
        store_group_signing_key(&store, &gid, &pk2, &[0xBB; 32]).unwrap();

        delete_all_group_signing_keys(&store, &gid).unwrap();

        assert!(get_group_signing_key(&store, &gid, &pk1).unwrap().is_none());
        assert!(get_group_signing_key(&store, &gid, &pk2).unwrap().is_none());
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
                    completed_at: Some(1_700_001_000),
                },
            },
        )
        .unwrap();

        let results = enumerate_in_progress_upgrades(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, gid_in_progress);
    }

    // -----------------------------------------------------------------------
    // enumerate_all_groups — prefix guard regression test
    // -----------------------------------------------------------------------

    /// Regression test: `enumerate_all_groups` must stop at GroupMeta keys and
    /// not spill into adjacent GroupMember keys (prefix 0x21).  Before the fix,
    /// the function would attempt to deserialise a `GroupMemberRole` value as
    /// `GroupMetaValue`, panicking with "failed to fill whole buffer".
    #[test]
    fn enumerate_all_groups_stops_before_member_keys() {
        let store = test_store();
        let gid = test_group_id();
        let meta = test_meta();
        let member = PublicKey::from([0x10; 32]);

        save_group_meta(&store, &gid, &meta).unwrap();
        // Add a group member — this writes a GroupMember key (prefix 0x21)
        // into the same column, right after GroupMeta keys (prefix 0x20).
        add_group_member(&store, &gid, &member, GroupMemberRole::Admin).unwrap();

        // Must return exactly one group without panicking.
        let groups = enumerate_all_groups(&store, 0, 100).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, gid.to_bytes());
    }

    #[test]
    fn enumerate_all_groups_multiple_groups_with_members() {
        let store = test_store();
        let gid1 = ContextGroupId::from([0x01; 32]);
        let gid2 = ContextGroupId::from([0x02; 32]);
        let meta = test_meta();

        save_group_meta(&store, &gid1, &meta).unwrap();
        save_group_meta(&store, &gid2, &meta).unwrap();
        add_group_member(
            &store,
            &gid1,
            &PublicKey::from([0xAA; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid2,
            &PublicKey::from([0xBB; 32]),
            GroupMemberRole::Member,
        )
        .unwrap();

        let groups = enumerate_all_groups(&store, 0, 100).unwrap();
        assert_eq!(groups.len(), 2);

        // Pagination
        let page = enumerate_all_groups(&store, 1, 1).unwrap();
        assert_eq!(page.len(), 1);
    }

    // -----------------------------------------------------------------------
    // extract_application_id — base58 decoding regression test
    // -----------------------------------------------------------------------

    /// Regression test: `extract_application_id` must decode the `id` field
    /// using base58 (via `Repr<ApplicationId>`), not hex.  Before the fix,
    /// `hex::decode` was called on a base58 string, producing
    /// "Invalid character 'g' at position 1" errors at runtime.
    #[test]
    fn extract_application_id_decodes_base58() {
        // Repr<[u8; 32]> serialises as base58 (canonical `Repr` serialization for the id field).
        use calimero_context_config::repr::Repr;

        let raw: [u8; 32] = [0xDE; 32];
        let encoded = Repr::new(raw).to_string(); // base58 string

        let json = serde_json::json!({ "id": encoded });
        let result = extract_application_id(&json).unwrap();
        assert_eq!(*result, raw);
    }

    #[test]
    fn extract_application_id_rejects_hex() {
        // A hex string decodes to ~46 bytes via base58, causing a length
        // mismatch against the required 32-byte ApplicationId.
        let hex_str = hex::encode([0xDE; 32]);
        let json = serde_json::json!({ "id": hex_str });
        assert!(extract_application_id(&json).is_err());
    }

    #[test]
    fn extract_application_id_missing_field_returns_error() {
        let json = serde_json::json!({});
        assert!(extract_application_id(&json).is_err());
    }

    // -----------------------------------------------------------------------
    // Member capability tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_member_capability() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);

        // No capability stored yet
        assert!(get_member_capability(&store, &gid, &pk).unwrap().is_none());

        // Set capabilities
        set_member_capability(&store, &gid, &pk, 0b101).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0b101);

        // Update capabilities
        set_member_capability(&store, &gid, &pk, 0b111).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0b111);
    }

    #[test]
    fn capability_zero_means_no_permissions() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x11; 32]);

        set_member_capability(&store, &gid, &pk, 0).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0);
        // All capability bits are off
        assert_eq!(caps & (1 << 0), 0); // CAN_CREATE_CONTEXT
        assert_eq!(caps & (1 << 1), 0); // CAN_INVITE_MEMBERS
        assert_eq!(caps & (1 << 2), 0); // CAN_JOIN_OPEN_CONTEXTS
    }

    #[test]
    fn capabilities_isolated_per_member() {
        let store = test_store();
        let gid = test_group_id();
        let alice = PublicKey::from([0x12; 32]);
        let bob = PublicKey::from([0x13; 32]);

        set_member_capability(&store, &gid, &alice, 0b001).unwrap();
        set_member_capability(&store, &gid, &bob, 0b110).unwrap();

        assert_eq!(
            get_member_capability(&store, &gid, &alice)
                .unwrap()
                .unwrap(),
            0b001
        );
        assert_eq!(
            get_member_capability(&store, &gid, &bob).unwrap().unwrap(),
            0b110
        );
    }

    // -----------------------------------------------------------------------
    // Default capabilities and visibility tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_default_capabilities() {
        let store = test_store();
        let gid = test_group_id();

        assert!(get_default_capabilities(&store, &gid).unwrap().is_none());

        set_default_capabilities(&store, &gid, 0b100).unwrap();
        assert_eq!(
            get_default_capabilities(&store, &gid).unwrap().unwrap(),
            0b100
        );

        // Update
        set_default_capabilities(&store, &gid, 0b111).unwrap();
        assert_eq!(
            get_default_capabilities(&store, &gid).unwrap().unwrap(),
            0b111
        );
    }

    #[test]
    fn set_and_get_default_visibility() {
        let store = test_store();
        let gid = test_group_id();

        assert!(get_default_visibility(&store, &gid).unwrap().is_none());

        // Open = 0
        set_default_visibility(&store, &gid, 0).unwrap();
        assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 0);

        // Restricted = 1
        set_default_visibility(&store, &gid, 1).unwrap();
        assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 1);
    }

    #[test]
    fn defaults_isolated_per_group() {
        let store = test_store();
        let g1 = ContextGroupId::from([0x40; 32]);
        let g2 = ContextGroupId::from([0x41; 32]);

        set_default_capabilities(&store, &g1, 0b001).unwrap();
        set_default_capabilities(&store, &g2, 0b110).unwrap();
        set_default_visibility(&store, &g1, 0).unwrap();
        set_default_visibility(&store, &g2, 1).unwrap();

        assert_eq!(
            get_default_capabilities(&store, &g1).unwrap().unwrap(),
            0b001
        );
        assert_eq!(
            get_default_capabilities(&store, &g2).unwrap().unwrap(),
            0b110
        );
        assert_eq!(get_default_visibility(&store, &g1).unwrap().unwrap(), 0);
        assert_eq!(get_default_visibility(&store, &g2).unwrap().unwrap(), 1);
    }

    // -----------------------------------------------------------------------
    // Auto-group: node identity as admin (regression test for fix)
    // -----------------------------------------------------------------------

    /// When an auto-group is created, the node's identity (not a random one)
    /// should be added as Admin. This test verifies that after
    /// `add_group_member_with_keys` the identity is a member and admin of the
    /// group — the same check that `listGroupMembers` and `joinGroupContext`
    /// perform via `check_group_membership`.
    #[test]
    fn auto_group_node_identity_is_admin_member() {
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let store = test_store();
        let context_id = ContextId::from([0xDD; 32]);
        let auto_group_id = ContextGroupId::from(*context_id.as_ref());

        // Simulate what create_context does: use node's group identity
        let node_sk = PrivateKey::random(&mut OsRng);
        let node_pk = node_sk.public_key();
        let sender_key = PrivateKey::random(&mut OsRng);

        // Save group meta (as create_context does for auto-groups)
        save_group_meta(
            &store,
            &auto_group_id,
            &GroupMetaValue {
                app_key: [0u8; 32],
                target_application_id: ApplicationId::from([0xCC; 32]),
                upgrade_policy: UpgradePolicy::Automatic,
                created_at: 1_700_000_000,
                admin_identity: node_pk,
                migration: None,
                auto_join: true,
            },
        )
        .unwrap();

        // Add node identity as admin with keys (as create_context does)
        add_group_member_with_keys(
            &store,
            &auto_group_id,
            &node_pk,
            GroupMemberRole::Admin,
            Some(*node_sk),
            Some(*sender_key),
        )
        .unwrap();

        // Register the context in the group
        register_context_in_group(&store, &auto_group_id, &context_id).unwrap();

        // The node's identity should be recognized as a group member
        assert!(check_group_membership(&store, &auto_group_id, &node_pk).unwrap());
        assert!(is_group_admin(&store, &auto_group_id, &node_pk).unwrap());

        // The group should have exactly 1 member
        assert_eq!(count_group_members(&store, &auto_group_id).unwrap(), 1);

        // The context should be registered in the group
        assert_eq!(
            get_group_for_context(&store, &context_id).unwrap().unwrap(),
            auto_group_id
        );
    }

    /// A random identity that is NOT the node's group identity should NOT
    /// pass membership checks — this is the bug scenario before the fix.
    #[test]
    fn auto_group_random_identity_not_found_by_node_check() {
        use calimero_primitives::identity::PrivateKey;
        use rand::rngs::OsRng;

        let store = test_store();
        let auto_group_id = ContextGroupId::from([0xEE; 32]);

        // A random creator identity was added as admin
        let random_sk = PrivateKey::random(&mut OsRng);
        let random_pk = random_sk.public_key();
        add_group_member(&store, &auto_group_id, &random_pk, GroupMemberRole::Admin).unwrap();

        // The node's ACTUAL group identity is different
        let node_sk = PrivateKey::random(&mut OsRng);
        let node_pk = node_sk.public_key();

        // The random identity IS a member
        assert!(check_group_membership(&store, &auto_group_id, &random_pk).unwrap());

        // But the node's identity is NOT a member — this is the bug
        assert!(!check_group_membership(&store, &auto_group_id, &node_pk).unwrap());
    }
}

// ---------------------------------------------------------------------------
// TEE admission policy helpers
// ---------------------------------------------------------------------------

/// Reconstructed TEE admission policy from the governance DAG.
#[derive(Debug)]
pub struct TeeAdmissionPolicy {
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
}

/// Read the most recent `TeeAdmissionPolicySet` from the group's governance op log.
pub fn read_tee_admission_policy(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<TeeAdmissionPolicy>> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;
    let mut latest: Option<TeeAdmissionPolicy> = None;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::TeeAdmissionPolicySet {
                allowed_mrtd,
                allowed_rtmr0,
                allowed_rtmr1,
                allowed_rtmr2,
                allowed_rtmr3,
                allowed_tcb_statuses,
                accept_mock,
            } = op.op
            {
                latest = Some(TeeAdmissionPolicy {
                    allowed_mrtd,
                    allowed_rtmr0,
                    allowed_rtmr1,
                    allowed_rtmr2,
                    allowed_rtmr3,
                    allowed_tcb_statuses,
                    accept_mock,
                });
            }
        }
    }

    Ok(latest)
}

/// Check whether a TEE attestation quote hash has already been used in a
/// `MemberJoinedViaTeeAttestation` op for this group.
pub fn is_quote_hash_used(
    store: &Store,
    group_id: &ContextGroupId,
    quote_hash: &[u8; 32],
) -> EyreResult<bool> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::MemberJoinedViaTeeAttestation {
                quote_hash: ref existing_hash,
                ..
            } = op.op
            {
                if existing_hash == quote_hash {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}
