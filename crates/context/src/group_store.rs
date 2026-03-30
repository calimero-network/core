use calimero_context_config::types::{
    ContextGroupId, GroupRevealPayloadData, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, ContextIdentity, GroupAlias, GroupChildIndex, GroupContextAlias,
    GroupContextAllowlist, GroupContextIndex, GroupContextLastMigration,
    GroupContextLastMigrationValue, GroupContextMemberCap, GroupContextVisibility,
    GroupContextVisibilityValue, GroupDefaultCaps, GroupDefaultCapsValue, GroupDefaultVis,
    GroupDefaultVisValue, GroupLocalGovNonce, GroupMember, GroupMemberAlias, GroupMemberCapability,
    GroupMemberCapabilityValue, GroupMemberContext, GroupMemberValue, GroupMeta, GroupMetaValue,
    GroupOpHead, GroupOpHeadValue, GroupOpLog, GroupParentRef, GroupSigningKey,
    GroupSigningKeyValue, GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue,
    GROUP_CHILD_INDEX_PREFIX, GROUP_CONTEXT_ALLOWLIST_PREFIX, GROUP_CONTEXT_INDEX_PREFIX,
    GROUP_CONTEXT_LAST_MIGRATION_PREFIX, GROUP_CONTEXT_VISIBILITY_PREFIX,
    GROUP_MEMBER_ALIAS_PREFIX, GROUP_MEMBER_CAPABILITY_PREFIX, GROUP_MEMBER_CONTEXT_PREFIX,
    GROUP_MEMBER_PREFIX, GROUP_META_PREFIX, GROUP_OP_LOG_PREFIX, GROUP_SIGNING_KEY_PREFIX,
    GROUP_UPGRADE_PREFIX,
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

fn validate_visibility_mode(mode: u8) -> EyreResult<()> {
    if mode > 1 {
        bail!("visibility mode must be 0 (Open) or 1 (Restricted), got {mode}");
    }
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

fn apply_join_with_invitation_claim(
    store: &Store,
    group_id: &ContextGroupId,
    joiner: &PublicKey,
    signed_invitation: &SignedGroupOpenInvitation,
    invitee_signature_hex: &str,
) -> EyreResult<()> {
    let inv = &signed_invitation.invitation;
    if inv.group_id != *group_id {
        bail!("invitation group_id does not match operation");
    }
    let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
    if !is_group_admin_or_has_capability(
        store,
        group_id,
        &inviter_pk,
        MemberCapabilities::CAN_INVITE_MEMBERS,
    )? {
        bail!("inviter lacks permission (not admin and missing CAN_INVITE_MEMBERS)");
    }

    let inv_bytes = borsh::to_vec(inv).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    let inv_hash = Sha256::digest(&inv_bytes);
    let inv_sig_hex = signed_invitation.inviter_signature.trim_start_matches("0x");
    let inv_sig =
        hex::decode(inv_sig_hex).map_err(|e| eyre::eyre!("inviter signature hex: {e}"))?;
    let inv_sig_bytes: [u8; 64] = inv_sig
        .try_into()
        .map_err(|_| eyre::eyre!("inviter signature must be 64 bytes"))?;
    inviter_pk
        .verify_raw_signature(&inv_hash, &inv_sig_bytes)
        .map_err(|e| eyre::eyre!("inviter signature verification failed: {e}"))?;

    let reveal = GroupRevealPayloadData {
        signed_open_invitation: signed_invitation.clone(),
        new_member_identity: SignerId::from(*joiner.digest()),
    };
    let reveal_bytes = borsh::to_vec(&reveal).map_err(|e| eyre::eyre!("borsh: {e}"))?;
    let reveal_hash = Sha256::digest(&reveal_bytes);
    let join_hex = invitee_signature_hex.trim_start_matches("0x");
    let join_sig = hex::decode(join_hex).map_err(|e| eyre::eyre!("invitee signature hex: {e}"))?;
    let join_sig_bytes: [u8; 64] = join_sig
        .try_into()
        .map_err(|_| eyre::eyre!("invitee signature must be 64 bytes"))?;
    joiner
        .verify_raw_signature(&reveal_hash, &join_sig_bytes)
        .map_err(|e| eyre::eyre!("invitee signature verification failed: {e}"))?;

    if check_group_membership(store, group_id, joiner)? {
        bail!("identity is already a member of this group");
    }
    add_group_member(store, group_id, joiner, GroupMemberRole::Member)?;
    Ok(())
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
            validate_visibility_mode(*mode)?;
            set_default_visibility(store, &group_id, *mode)?;
        }
        GroupOp::ContextVisibilitySet {
            context_id,
            mode,
            creator,
        } => {
            let is_direct_admin = is_direct_group_admin(store, &group_id, &op.signer)?;
            if !is_direct_admin && op.signer != *creator {
                bail!(
                    "only a direct group admin or context creator can set context visibility \
                     (inherited parent admin authority does not apply)"
                );
            }
            validate_visibility_mode(*mode)?;
            set_context_visibility(store, &group_id, context_id, *mode, *creator.as_ref())?;
            if *mode == 1 {
                if !check_context_allowlist(store, &group_id, context_id, creator)? {
                    add_to_context_allowlist(store, &group_id, context_id, creator)?;
                }
            }
        }
        GroupOp::ContextAllowlistReplaced {
            context_id,
            members,
        } => {
            let is_direct_admin = is_direct_group_admin(store, &group_id, &op.signer)?;
            if !is_direct_admin {
                if let Some((_, creator_bytes)) =
                    get_context_visibility(store, &group_id, context_id)?
                {
                    if creator_bytes != *op.signer {
                        bail!(
                            "only a direct group admin or context creator can manage allowlists \
                             (inherited parent admin authority does not apply)"
                        );
                    }
                } else {
                    bail!("context visibility not found for context in group");
                }
            }
            clear_context_allowlist(store, &group_id, context_id)?;
            for m in members {
                add_to_context_allowlist(store, &group_id, context_id, m)?;
            }
        }
        GroupOp::ContextAliasSet { context_id, alias } => {
            let is_admin = is_group_admin(store, &group_id, &op.signer)?;
            if !is_admin {
                if let Some((_, creator_bytes)) =
                    get_context_visibility(store, &group_id, context_id)?
                {
                    if creator_bytes != *op.signer {
                        bail!("only admin or context creator can set context alias");
                    }
                } else {
                    bail!("context visibility not found for context in group");
                }
            }
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
        GroupOp::JoinWithInvitationClaim {
            signed_invitation,
            invitee_signature_hex,
        } => {
            apply_join_with_invitation_claim(
                store,
                &group_id,
                &op.signer,
                signed_invitation,
                invitee_signature_hex,
            )?;
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
        GroupOp::MemberJoinedViaContextInvitation {
            context_id: _,
            inviter_id,
            invitation_payload,
            inviter_signature,
        } => {
            if !is_group_admin_or_has_capability(
                store,
                &group_id,
                inviter_id,
                MemberCapabilities::CAN_INVITE_MEMBERS,
            )? {
                bail!("context invitation inviter lacks permission (not admin and missing CAN_INVITE_MEMBERS)");
            }
            let inv_hash = Sha256::digest(invitation_payload);
            let sig_hex = inviter_signature.trim_start_matches("0x");
            let sig_bytes_vec =
                hex::decode(sig_hex).map_err(|e| eyre::eyre!("inviter sig hex: {e}"))?;
            let sig_bytes: [u8; 64] = sig_bytes_vec
                .try_into()
                .map_err(|_| eyre::eyre!("inviter signature must be 64 bytes"))?;
            inviter_id
                .verify_raw_signature(&inv_hash, &sig_bytes)
                .map_err(|e| eyre::eyre!("context invitation inviter signature invalid: {e}"))?;

            if !check_group_membership(store, &group_id, &op.signer)? {
                add_group_member(store, &group_id, &op.signer, GroupMemberRole::Member)?;
            }
        }
        GroupOp::SubgroupCreated { child_group_id } => {
            require_group_admin(store, &group_id, &op.signer)?;
            let child_gid = ContextGroupId::from(*child_group_id);
            set_parent_group(store, &child_gid, &group_id)?;
        }
        GroupOp::SubgroupRemoved { child_group_id } => {
            require_group_admin(store, &group_id, &op.signer)?;
            let child_gid = ContextGroupId::from(*child_group_id);
            if get_parent_group(store, &child_gid)?.as_ref() != Some(&group_id) {
                bail!("child group is not a subgroup of this group");
            }
            remove_parent_group(store, &child_gid)?;
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
            if !policy.allowed_mrtd.is_empty()
                && !policy.allowed_mrtd.iter().any(|a| a == mrtd)
            {
                bail!("MemberJoinedViaTeeAttestation rejected: MRTD not in policy allowlist");
            }
            if !policy.allowed_tcb_statuses.is_empty()
                && !policy.allowed_tcb_statuses.iter().any(|a| a == tcb_status)
            {
                bail!("MemberJoinedViaTeeAttestation rejected: TCB status not in policy allowlist");
            }
            if !check_group_membership(store, &group_id, member)? {
                add_group_member(store, &group_id, member, *role)?;
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

/// Returns the member's effective role, walking up the ancestor chain.
/// Direct membership takes priority; if not found, checks parent groups.
/// Returns the most privileged role found (Admin > Member > ReadOnly).
pub fn get_group_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    let mut current = *group_id;
    for _ in 0..MAX_SUBGROUP_DEPTH {
        if let Some(role) = get_direct_member_role(store, &current, identity)? {
            return Ok(Some(role));
        }
        match get_parent_group(store, &current)? {
            Some(parent) => current = parent,
            None => return Ok(None),
        }
    }
    Ok(None)
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

const MAX_SUBGROUP_DEPTH: usize = 16;

pub fn get_parent_group(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<ContextGroupId>> {
    let handle = store.handle();
    let key = GroupParentRef::new(group_id.to_bytes());
    Ok(handle.get(&key)?.map(ContextGroupId::from))
}

pub fn set_parent_group(
    store: &Store,
    child_group_id: &ContextGroupId,
    parent_group_id: &ContextGroupId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let ref_key = GroupParentRef::new(child_group_id.to_bytes());
    handle.put(&ref_key, &parent_group_id.to_bytes())?;
    let idx_key = GroupChildIndex::new(parent_group_id.to_bytes(), child_group_id.to_bytes());
    handle.put(&idx_key, &())?;
    Ok(())
}

pub fn remove_parent_group(store: &Store, child_group_id: &ContextGroupId) -> EyreResult<()> {
    if let Some(parent_id) = get_parent_group(store, child_group_id)? {
        let mut handle = store.handle();
        let ref_key = GroupParentRef::new(child_group_id.to_bytes());
        handle.delete(&ref_key)?;
        let idx_key = GroupChildIndex::new(parent_id.to_bytes(), child_group_id.to_bytes());
        handle.delete(&idx_key)?;
    }
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
    let mut current = *group_id;
    for _ in 0..MAX_SUBGROUP_DEPTH {
        if has_direct_member(store, &current, identity)? {
            return Ok(true);
        }
        match get_parent_group(store, &current)? {
            Some(parent) => current = parent,
            None => return Ok(false),
        }
    }
    Ok(false)
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

pub fn enumerate_child_groups(
    store: &Store,
    parent_group_id: &ContextGroupId,
) -> EyreResult<Vec<ContextGroupId>> {
    let handle = store.handle();
    let parent_bytes: [u8; 32] = parent_group_id.to_bytes();
    let start_key = GroupChildIndex::new(parent_bytes, [0u8; 32]);
    let mut iter = handle.iter::<GroupChildIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_CHILD_INDEX_PREFIX {
            break;
        }
        if key.parent_group_id() != parent_bytes {
            break;
        }
        results.push(ContextGroupId::from(key.child_group_id()));
    }

    Ok(results)
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

    let children = enumerate_child_groups(store, group_id)?;
    for child_id in &children {
        if !has_direct_member(store, child_id, member)? {
            cascade_remove_member_from_group_tree(store, child_id, member)?;
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

/// Returns (mode, creator_pk). mode: 0 = Open, 1 = Restricted.
pub fn get_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<(u8, [u8; 32])>> {
    let handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    let value = handle.get(&key)?;
    Ok(value.map(|v| (v.mode, v.creator)))
}

pub fn set_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    mode: u8,
    creator: [u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    handle.put(&key, &GroupContextVisibilityValue { mode, creator })?;
    Ok(())
}

pub fn check_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    // If the key exists (even with unit value), the member is on the allowlist
    let value: Option<()> = handle.get(&key)?;
    Ok(value.is_some())
}

pub fn add_to_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    handle.put(&key, &())?;
    Ok(())
}

pub fn remove_from_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    handle.delete(&key)?;
    Ok(())
}

pub fn list_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Vec<PublicKey>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key =
        GroupContextAllowlist::new(group_id_bytes, *context_id, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextAllowlist>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_ALLOWLIST_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if key.context_id() != *context_id {
            break;
        }

        results.push(PublicKey::from(*key.member()));
    }

    Ok(results)
}

pub fn delete_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    handle.delete(&key)?;
    Ok(())
}

pub fn clear_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let members = list_context_allowlist(store, group_id, context_id)?;
    for member in &members {
        remove_from_context_allowlist(store, group_id, context_id, member)?;
    }
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

pub fn enumerate_context_visibilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, u8, [u8; 32])>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextVisibility::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextVisibility>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_VISIBILITY_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        let Some(val) = handle.get(&key)? else {
            continue;
        };

        results.push((ContextId::from(*key.context_id()), val.mode, val.creator));
    }

    Ok(results)
}

pub fn enumerate_contexts_with_allowlists(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, Vec<PublicKey>)>> {
    let context_ids = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    let mut results = Vec::new();

    for context_id in context_ids {
        let members = list_context_allowlist(store, group_id, &context_id)?;
        if !members.is_empty() {
            results.push((context_id, members));
        }
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

        set_context_visibility(&store, &gid, &context_id, 0, *creator_pk).unwrap();

        let op_alias = SignedGroupOp::sign(
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
        apply_local_signed_group_op(&store, &op_alias).unwrap();
        assert_eq!(
            get_context_alias(&store, &gid, &context_id)
                .unwrap()
                .as_deref(),
            Some("from-creator")
        );

        let stranger_sk = PrivateKey::random(&mut rng);
        add_group_member(
            &store,
            &gid,
            &stranger_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();
        let op_bad = SignedGroupOp::sign(
            &stranger_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::ContextAliasSet {
                context_id,
                alias: "hijack".to_owned(),
            },
        )
        .unwrap();
        assert!(apply_local_signed_group_op(&store, &op_bad).is_err());

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
    // Context visibility tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_context_visibility() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x20; 32]);
        let creator: [u8; 32] = [0x01; 32];

        // No visibility stored yet
        assert!(get_context_visibility(&store, &gid, &ctx)
            .unwrap()
            .is_none());

        // Set to Open (0)
        set_context_visibility(&store, &gid, &ctx, 0, creator).unwrap();
        let (mode, stored_creator) = get_context_visibility(&store, &gid, &ctx).unwrap().unwrap();
        assert_eq!(mode, 0);
        assert_eq!(stored_creator, creator);

        // Update to Restricted (1)
        set_context_visibility(&store, &gid, &ctx, 1, creator).unwrap();
        let (mode, _) = get_context_visibility(&store, &gid, &ctx).unwrap().unwrap();
        assert_eq!(mode, 1);
    }

    #[test]
    fn visibility_isolated_per_context() {
        let store = test_store();
        let gid = test_group_id();
        let ctx1 = ContextId::from([0x21; 32]);
        let ctx2 = ContextId::from([0x22; 32]);
        let creator: [u8; 32] = [0x01; 32];

        set_context_visibility(&store, &gid, &ctx1, 0, creator).unwrap();
        set_context_visibility(&store, &gid, &ctx2, 1, creator).unwrap();

        let (mode1, _) = get_context_visibility(&store, &gid, &ctx1)
            .unwrap()
            .unwrap();
        let (mode2, _) = get_context_visibility(&store, &gid, &ctx2)
            .unwrap()
            .unwrap();
        assert_eq!(mode1, 0);
        assert_eq!(mode2, 1);
    }

    // -----------------------------------------------------------------------
    // Context allowlist tests
    // -----------------------------------------------------------------------

    #[test]
    fn add_check_remove_context_allowlist() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x30; 32]);
        let member = PublicKey::from([0x31; 32]);

        // Not on allowlist initially
        assert!(!check_context_allowlist(&store, &gid, &ctx, &member).unwrap());

        // Add to allowlist
        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        assert!(check_context_allowlist(&store, &gid, &ctx, &member).unwrap());

        // Remove from allowlist
        remove_from_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        assert!(!check_context_allowlist(&store, &gid, &ctx, &member).unwrap());
    }

    #[test]
    fn list_context_allowlist_returns_all_members() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x32; 32]);
        let m1 = PublicKey::from([0x33; 32]);
        let m2 = PublicKey::from([0x34; 32]);
        let m3 = PublicKey::from([0x35; 32]);

        add_to_context_allowlist(&store, &gid, &ctx, &m1).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &m2).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &m3).unwrap();

        let members = list_context_allowlist(&store, &gid, &ctx).unwrap();
        assert_eq!(members.len(), 3);
        assert!(members.contains(&m1));
        assert!(members.contains(&m2));
        assert!(members.contains(&m3));
    }

    #[test]
    fn list_context_allowlist_isolated_per_context() {
        let store = test_store();
        let gid = test_group_id();
        let ctx1 = ContextId::from([0x36; 32]);
        let ctx2 = ContextId::from([0x37; 32]);
        let m1 = PublicKey::from([0x38; 32]);
        let m2 = PublicKey::from([0x39; 32]);

        add_to_context_allowlist(&store, &gid, &ctx1, &m1).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx2, &m2).unwrap();

        let ctx1_members = list_context_allowlist(&store, &gid, &ctx1).unwrap();
        assert_eq!(ctx1_members.len(), 1);
        assert!(ctx1_members.contains(&m1));

        let ctx2_members = list_context_allowlist(&store, &gid, &ctx2).unwrap();
        assert_eq!(ctx2_members.len(), 1);
        assert!(ctx2_members.contains(&m2));
    }

    #[test]
    fn allowlist_add_idempotent() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x3A; 32]);
        let member = PublicKey::from([0x3B; 32]);

        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();

        let members = list_context_allowlist(&store, &gid, &ctx).unwrap();
        assert_eq!(members.len(), 1);
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
///
/// Scans the log tracking only the sequence number of the latest policy op,
/// then clones its data once at the end to avoid intermediate allocations.
pub fn read_tee_admission_policy(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<TeeAdmissionPolicy>> {
    let handle = store.handle();
    let start = GroupOpLog::new(group_id.to_bytes(), 0);
    let mut latest_seq: Option<u64> = None;

    for entry in handle.iter::<GroupOpLog>(&start)? {
        let (key, value) = entry?;
        if key.group_id() != group_id.to_bytes() {
            break;
        }
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(&value.op_bytes) {
            if matches!(op.op, GroupOp::TeeAdmissionPolicySet { .. }) {
                latest_seq = Some(key.sequence());
            }
        }
    }

    let seq = match latest_seq {
        Some(s) => s,
        None => return Ok(None),
    };

    let key = GroupOpLog::new(group_id.to_bytes(), seq);
    let value = handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("op log entry disappeared"))?;
    let op = borsh::from_slice::<SignedGroupOp>(&value.op_bytes)
        .map_err(|e| eyre::eyre!("borsh: {e}"))?;

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
        Ok(Some(TeeAdmissionPolicy {
            allowed_mrtd,
            allowed_rtmr0,
            allowed_rtmr1,
            allowed_rtmr2,
            allowed_rtmr3,
            allowed_tcb_statuses,
            accept_mock,
        }))
    } else {
        Ok(None)
    }
}

