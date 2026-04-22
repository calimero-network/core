use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    GroupChildIndex, GroupParentRef, NamespaceIdentity, NamespaceIdentityValue,
    GROUP_CHILD_INDEX_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use rand::rngs::OsRng;
use rand::Rng;
use sha2::Digest;

use super::{
    cascade_remove_member_from_group_tree, check_group_membership, collect_keys_with_prefix,
    get_group_for_context, get_group_member_role, get_op_head, remove_group_member,
};

pub(crate) const MAX_NAMESPACE_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct NamespaceIdentityRecord {
    pub public_key: PublicKey,
    pub private_key: [u8; 32],
    pub sender_key: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedNamespaceIdentity {
    pub namespace_id: ContextGroupId,
    pub identity: NamespaceIdentityRecord,
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

/// Returns `true` if the member has a read-only role (`ReadOnly` or `ReadOnlyTee`)
/// in the group that owns this context.
/// Returns `false` if the context has no group, the member is not found, or the member
/// has `Admin` or `Member` role.
pub fn is_read_only_for_context(
    store: &Store,
    context_id: &ContextId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    match get_group_member_role(store, &group_id, identity)? {
        Some(
            calimero_primitives::context::GroupMemberRole::ReadOnly
            | calimero_primitives::context::GroupMemberRole::ReadOnlyTee,
        ) => Ok(true),
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
    let pid = parent_group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupChildIndex::new(pid, [0u8; 32]),
        GROUP_CHILD_INDEX_PREFIX,
        |k| k.parent_group_id() == pid,
    )?;
    Ok(keys
        .into_iter()
        .map(|k| ContextGroupId::from(k.child_group_id()))
        .collect())
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
) -> EyreResult<
    Vec<(
        ContextGroupId,
        calimero_context_config::types::SignedGroupOpenInvitation,
    )>,
> {
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
        let secret_salt: [u8; 32] = OsRng.gen();

        let invitation = GroupInvitationFromAdmin {
            inviter_identity: inviter_signer_id,
            group_id: gid,
            expiration_timestamp: expiration,
            secret_salt,
            invited_role,
        };

        let inv_bytes = borsh::to_vec(&invitation).map_err(|e| eyre::eyre!("borsh: {e}"))?;
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

/// Atomically swap the parent of `child` to `new_parent`.
///
/// Replaces the old `nest_group` + `unnest_group` two-step pattern with a
/// single op so orphan state is no longer expressible. Enforces:
/// - `child` must currently have a parent (cannot reparent the namespace root).
/// - `new_parent` must exist in the store (have a `GroupMeta` entry).
/// - `new_parent` must not be a descendant of `child` (no cycles).
/// - Idempotent on `new_parent == old_parent`.
///
/// All edge mutations happen in one store handle so a partial state is
/// never observable.
pub fn reparent_group(
    store: &Store,
    child: &ContextGroupId,
    new_parent: &ContextGroupId,
) -> EyreResult<()> {
    let old_parent = get_parent_group(store, child)?
        .ok_or_else(|| eyre::eyre!("cannot reparent the namespace root: '{child:?}' has no parent"))?;

    if old_parent == *new_parent {
        return Ok(());
    }

    if super::load_group_meta(store, new_parent)?.is_none() {
        eyre::bail!("new parent group '{new_parent:?}' not found in this namespace");
    }

    if is_descendant_of(store, new_parent, child)? {
        eyre::bail!(
            "cycle: new_parent '{new_parent:?}' is a descendant of child '{child:?}'"
        );
    }

    let mut handle = store.handle();
    handle.delete(&GroupChildIndex::new(old_parent.to_bytes(), child.to_bytes()))?;
    handle.put(&GroupParentRef::new(child.to_bytes()), &new_parent.to_bytes())?;
    handle.put(&GroupChildIndex::new(new_parent.to_bytes(), child.to_bytes()), &())?;
    Ok(())
}

/// Returns `true` iff `candidate` is a (transitive) descendant of
/// `potential_ancestor`. Returns `false` for `candidate == potential_ancestor`.
/// Bounded by `MAX_NAMESPACE_DEPTH`; returns `Err` if the walk exceeds the cap
/// (indicates store corruption / cycle).
///
/// Used by `reparent_group` to reject moves that would create a cycle.
pub fn is_descendant_of(
    store: &Store,
    candidate: &ContextGroupId,
    potential_ancestor: &ContextGroupId,
) -> EyreResult<bool> {
    if candidate == potential_ancestor {
        return Ok(false);
    }
    let mut current = *candidate;
    for _ in 0..MAX_NAMESPACE_DEPTH {
        match get_parent_group(store, &current)? {
            Some(parent) => {
                if parent == *potential_ancestor {
                    return Ok(true);
                }
                current = parent;
            }
            None => return Ok(false),
        }
    }
    eyre::bail!(
        "is_descendant_of exceeded MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); possible cycle in store"
    )
}

/// Read this node's identity for a namespace from the store.
pub fn get_namespace_identity(
    store: &Store,
    namespace_id: &ContextGroupId,
) -> EyreResult<Option<(PublicKey, [u8; 32], [u8; 32])>> {
    Ok(get_namespace_identity_record(store, namespace_id)?
        .map(|record| (record.public_key, record.private_key, record.sender_key)))
}

pub fn get_namespace_identity_record(
    store: &Store,
    namespace_id: &ContextGroupId,
) -> EyreResult<Option<NamespaceIdentityRecord>> {
    let handle = store.handle();
    let key = NamespaceIdentity::new(namespace_id.to_bytes());
    match handle.get(&key)? {
        Some(val) => Ok(Some(NamespaceIdentityRecord {
            public_key: PublicKey::from(val.public_key),
            private_key: val.private_key,
            sender_key: val.sender_key,
        })),
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
    Ok(resolve_namespace_identity_record(store, group_id)?
        .map(|record| (record.public_key, record.private_key, record.sender_key)))
}

pub fn resolve_namespace_identity_record(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<NamespaceIdentityRecord>> {
    let ns_id = resolve_namespace(store, group_id)?;
    get_namespace_identity_record(store, &ns_id)
}

/// Resolve the namespace for a group and return this node's identity,
/// generating and storing a new keypair if none exists.
pub fn get_or_create_namespace_identity(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<(ContextGroupId, PublicKey, [u8; 32], [u8; 32])> {
    let bundle = get_or_create_namespace_identity_bundle(store, group_id)?;
    Ok((
        bundle.namespace_id,
        bundle.identity.public_key,
        bundle.identity.private_key,
        bundle.identity.sender_key,
    ))
}

pub fn get_or_create_namespace_identity_bundle(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<ResolvedNamespaceIdentity> {
    let ns_id = resolve_namespace(store, group_id)?;
    if let Some(identity) = get_namespace_identity_record(store, &ns_id)? {
        return Ok(ResolvedNamespaceIdentity {
            namespace_id: ns_id,
            identity,
        });
    }

    let private_key = PrivateKey::random(&mut OsRng);
    let public_key = private_key.public_key();
    let sender_key = PrivateKey::random(&mut OsRng);

    store_namespace_identity(store, &ns_id, &public_key, &private_key, &sender_key)?;

    Ok(ResolvedNamespaceIdentity {
        namespace_id: ns_id,
        identity: NamespaceIdentityRecord {
            public_key,
            private_key: *private_key,
            sender_key: *sender_key,
        },
    })
}
