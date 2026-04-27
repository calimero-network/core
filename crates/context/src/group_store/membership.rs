use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{AutoFollowFlags, GroupMember, GroupMemberValue, GROUP_MEMBER_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::namespace::{get_parent_group, MAX_NAMESPACE_DEPTH};
use super::{
    collect_keys_with_prefix, collect_keys_with_prefix_paginated, count_keys_with_prefix,
    get_member_capability, get_subgroup_visibility, load_group_meta, set_member_capability,
    GroupStoreError,
};

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
    // Preserve auto_follow across updates — add_group_member is used for
    // upserts (e.g. MemberRoleSet), and users will have set their
    // auto-follow flags independently of role changes.
    let existing_auto_follow = handle
        .get::<GroupMember>(&key)?
        .map(|v| v.auto_follow)
        .unwrap_or_default();
    handle.put(
        &key,
        &GroupMemberValue {
            role,
            private_key,
            sender_key,
            auto_follow: existing_auto_follow,
        },
    )?;
    drop(handle);

    if !is_admin {
        if let Some(defaults) = get_member_default_capabilities(store, group_id)? {
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

/// Update the auto-follow flags for an existing member. Caller must
/// have already verified the member exists (this function bails if
/// not) and that the signer is authorized to mutate them.
pub fn set_member_auto_follow(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    auto_follow: AutoFollowFlags,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let existing = handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("member not found in group"))?;
    handle.put(
        &key,
        &GroupMemberValue {
            role: existing.role,
            private_key: existing.private_key,
            sender_key: existing.sender_key,
            auto_follow,
        },
    )?;
    Ok(())
}

/// Returns the member's direct role in this group, if present.
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

/// How a positive membership decision was reached. See
/// [`check_group_membership_path`] for full walk semantics.
///
/// Used by audit-sensitive callers (e.g., `join_context`) that want to
/// distinguish a directly-stored membership row from one synthesized by
/// the parent-chain inheritance walk, since inherited members do not
/// appear in `list_group_members` for the subgroup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipPath {
    /// Identity is not a member of the subgroup, directly or by inheritance.
    None,
    /// Identity has a direct membership row in the subgroup.
    Direct,
    /// Identity inherits membership from the closest ancestor where they
    /// hold a direct row (`anchor`). `via_admin` is `true` when the
    /// inheritance came from an admin grant; `false` when it came from
    /// `CAN_JOIN_OPEN_SUBGROUPS`.
    Inherited {
        anchor: ContextGroupId,
        via_admin: bool,
    },
}

/// Returns the [`MembershipPath`] by which `identity` is a member of
/// `group_id`, or `None` if they are not a member.
///
/// Walk semantics (issue #2256):
///
/// 1. Direct membership in `group_id` → [`MembershipPath::Direct`].
/// 2. Else, if `group_id` is `Restricted` (or has no `subgroup_visibility`
///    set, treated as `Restricted`), return [`MembershipPath::None`].
/// 3. Else (`Open`), look up the parent. No parent → [`MembershipPath::None`]
///    (we hit the namespace root without finding the identity).
/// 4. If `identity` is a direct member of the parent, that's the **anchor**.
///    Admins inherit unconditionally; non-admins need
///    [`MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS`].
/// 5. Otherwise, recurse: walk one level up.
///
/// The walk terminates at the first `Restricted` ancestor (a wall) or at
/// the namespace root. Bounded by [`MAX_NAMESPACE_DEPTH`] to defend
/// against corrupted store state with cyclic parent edges. The namespace
/// root's *own* `subgroup_visibility` is intentionally never read — the
/// walk reaches it only when a direct membership at the root is the
/// anchor, at which point the `has_direct_member` check at step 4 returns
/// before step 2 is re-evaluated for that level.
///
/// **Architectural note:** this same parent-walk logic anchors several
/// other subsystems that all need to recognize Open-subgroup inheritance
/// consistently:
///
/// - [`super::permission_checker::PermissionChecker::is_admin`] and
///   `is_authorized_with_capability` — governance-op authorization for
///   inherited admins / capability-holders.
/// - `crates/context/src/handlers/execute/mod.rs` — picks the namespace
///   key (instead of the subgroup key) when encrypting context state
///   deltas for Open subgroups.
/// - `crates/node/src/handlers/state_delta/mod.rs` — falls back to the
///   namespace keyring on receiver-side decryption miss.
/// - `crates/node/src/sync/manager/mod.rs` — accepts inheritance-eligible
///   parent members at the responder-side stream-auth gate
///   (`DagHeadsRequest`, `DeltaRequest`, snapshot stream).
///
/// Keeping these in sync is the price of having Open-subgroup
/// inheritance work end-to-end without a separate key-distribution
/// path. If you change the walk semantics here, audit the four call
/// sites above.
pub fn check_group_membership_path(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<MembershipPath> {
    if has_direct_member(store, group_id, identity)? {
        return Ok(MembershipPath::Direct);
    }

    let mut current = *group_id;
    for _ in 0..MAX_NAMESPACE_DEPTH {
        if get_subgroup_visibility(store, &current)? != VisibilityMode::Open {
            return Ok(MembershipPath::None);
        }
        let Some(parent) = get_parent_group(store, &current)? else {
            return Ok(MembershipPath::None);
        };
        if has_direct_member(store, &parent, identity)? {
            if is_group_admin(store, &parent, identity)? {
                return Ok(MembershipPath::Inherited {
                    anchor: parent,
                    via_admin: true,
                });
            }
            let caps = get_member_capability(store, &parent, identity)?.unwrap_or(0);
            if caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS != 0 {
                return Ok(MembershipPath::Inherited {
                    anchor: parent,
                    via_admin: false,
                });
            }
            return Ok(MembershipPath::None);
        }
        current = parent;
    }
    bail!(
        "check_group_membership exceeded MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); \
         possible cycle in store"
    )
}

/// Returns `true` if `identity` is a member of `group_id` either directly
/// or by inheritance. Thin wrapper over [`check_group_membership_path`]
/// for callers that don't need to distinguish the path.
pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    Ok(!matches!(
        check_group_membership_path(store, group_id, identity)?,
        MembershipPath::None,
    ))
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
    if let Some(GroupMemberRole::Admin) = get_group_member_role(store, group_id, identity)? {
        return Ok(true);
    }
    if let Some(meta) = load_group_meta(store, group_id)? {
        if meta.admin_identity == *identity {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn require_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    if !is_group_admin(store, group_id, identity)? {
        bail!(GroupStoreError::NotAdmin {
            group_id: format!("{group_id:?}"),
            identity: format!("{identity:?}"),
        });
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
        bail!(GroupStoreError::Unauthorized {
            group_id: format!("{group_id:?}"),
            operation: operation.to_owned(),
        });
    }
    Ok(())
}

pub fn count_group_admins(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut count = 0usize;
    for key in keys {
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
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix_paginated(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
        offset,
        limit,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let val: GroupMemberValue = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        results.push((key.identity(), val.role));
    }
    Ok(results)
}

pub fn count_group_members(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    count_keys_with_prefix(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )
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

fn get_member_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u32>> {
    super::get_default_capabilities(store, group_id)
}
