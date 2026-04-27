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
    ///
    /// **Caution:** `Direct` does *not* imply the identity lacks
    /// inherited admin authority. The walk in [`check_group_membership_path`]
    /// short-circuits as soon as a direct row is found — even a
    /// non-admin `Member` row — and never inspects the parent chain.
    /// A parent admin who is also added as a regular subgroup member
    /// will appear as `Direct` here while still holding inherited admin
    /// authority. Callers that need to know admin status must call
    /// [`is_inherited_admin`] separately rather than infer "not an
    /// inherited admin" from `Direct`.
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
///    set, treated as `Restricted`), return the recorded anchor decision
///    (or [`MembershipPath::None`] if none was recorded).
/// 3. Else (`Open`), look up the parent. No parent → return the recorded
///    anchor decision (or [`MembershipPath::None`]).
/// 4. **Admin authority cascades.** If `identity` is admin at this
///    parent (direct admin row, or `GroupMeta.admin_identity`), return
///    `Inherited { anchor: parent, via_admin: true }` immediately —
///    parent admins are not revocable by intermediate non-admin direct
///    rows. This keeps `check_group_membership_path` in agreement with
///    [`is_inherited_admin`] on admin authority.
/// 5. **Anchor cap (deepest non-admin direct row).** If `identity` is a
///    direct (non-admin) member of this parent and we have not yet
///    recorded an anchor decision, record one based on its caps:
///    `CAN_JOIN_OPEN_SUBGROUPS` set → `Inherited { via_admin: false }`,
///    else `None`. Do **not** return — keep walking; a parent admin
///    further up still overrides this denial.
/// 6. Walk one level up.
///
/// The walk terminates at the first `Restricted` ancestor (a wall) or
/// at the namespace root. Bounded by [`MAX_NAMESPACE_DEPTH`] to defend
/// against corrupted store state with cyclic parent edges.
///
/// **Namespace-root visibility is read but doesn't drive the outcome.**
/// After step 6 sets `current = parent` to a namespace root, the next
/// iteration's step 2 reads `get_subgroup_visibility(root)`. Either
/// branch terminates with the same value: if `Restricted`, we return
/// the recorded `anchor_decision`; if `Open`, the subsequent
/// `get_parent_group(root)` returns `None` and we return the same
/// `anchor_decision`. No code path consults the root's setting to
/// allow inheritance through it (the root is itself the inheritance
/// boundary), so an admin who sets `subgroup_visibility` on the
/// namespace root sees no behavioral effect. The
/// `set_subgroup_visibility` handler emits a warning when called on
/// the root so operators notice the no-op without breaking existing
/// workflows that issue the call as a harmless setup step.
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

    // Track the deepest non-admin direct-row anchor decision (if any)
    // as a *fallback*. We do not short-circuit on it — admin authority
    // at a higher Open-chain ancestor cascades through and overrides a
    // denial at the anchor. This keeps `check_group_membership_path`
    // and [`is_inherited_admin`] in agreement on admin authority while
    // preserving anchor-cap semantics for ordinary members.
    //
    // Joining a context is a voluntary action. A namespace admin who
    // happened to be added as a regular member of an intermediate
    // group, with that group's join cap cleared, would otherwise be
    // unable to join contexts in descendant subgroups they govern —
    // even though they have full admin authority and could trivially
    // re-grant themselves the cap. The cap-revocation in that case is
    // a per-level participation knob, not a security barrier; honoring
    // it strictly only produces the confusing "can govern but cannot
    // join" UX without offering a real isolation guarantee.
    let mut anchor_decision: Option<MembershipPath> = None;

    let mut current = *group_id;
    // Off-by-one note: `is_open_chain_to_namespace` short-circuits
    // when `parent == namespace_id` (success at iter k for chain
    // length k). This walk has no equivalent shortcut — it "falls
    // off" the chain by reaching `current = root` and then needing
    // one *additional* iteration to read the root's visibility /
    // observe `get_parent_group(root) == None` and return the
    // recorded `anchor_decision`. Bound the loop at
    // `MAX_NAMESPACE_DEPTH + 1` so a chain at exactly
    // `MAX_NAMESPACE_DEPTH` resolves here just as it does in the
    // chain-check; otherwise auth and crypto-key selection
    // disagree at the boundary (encrypt path picks the namespace
    // key, auth bails with a spurious cycle error). Cycle
    // detection is unaffected — a true cycle still exhausts the
    // bound.
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        if get_subgroup_visibility(store, &current)? != VisibilityMode::Open {
            return Ok(anchor_decision.unwrap_or(MembershipPath::None));
        }
        let Some(parent) = get_parent_group(store, &current)? else {
            return Ok(anchor_decision.unwrap_or(MembershipPath::None));
        };
        if is_group_admin(store, &parent, identity)? {
            return Ok(MembershipPath::Inherited {
                anchor: parent,
                via_admin: true,
            });
        }
        if has_direct_member(store, &parent, identity)? && anchor_decision.is_none() {
            // Deepest non-admin anchor: record its cap-based decision
            // but keep walking — a parent admin further up overrides.
            let caps = get_member_capability(store, &parent, identity)?.unwrap_or(0);
            anchor_decision = Some(if caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS != 0 {
                MembershipPath::Inherited {
                    anchor: parent,
                    via_admin: false,
                }
            } else {
                MembershipPath::None
            });
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

/// Returns `true` iff `identity` holds direct admin authority at *any*
/// ancestor in the Open chain rooted at `group_id` (or at `group_id`
/// itself).
///
/// Unlike [`check_group_membership_path`], this walk is **independent of
/// any non-admin direct membership** the identity may have in the
/// target subgroup. A parent admin who has also been added as a
/// regular `Member` of a descendant subgroup still inherits admin
/// authority into that subgroup — without this dedicated walk, the
/// `Direct` short-circuit in `check_group_membership_path` would
/// suppress the inherited-admin signal as soon as any direct
/// membership row existed.
///
/// Walk semantics mirror `check_group_membership_path`:
///   1. Direct admin in `group_id` → `true`.
///   2. Else, if `group_id` is `Restricted` → `false` (wall).
///   3. Else (`Open`), look up parent. No parent → `false`.
///   4. If `identity` is a direct admin of any ancestor in the Open
///      chain → `true`.
///   5. Else continue walking.
///
/// Bounded by [`MAX_NAMESPACE_DEPTH`].
pub fn is_inherited_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    if is_group_admin(store, group_id, identity)? {
        return Ok(true);
    }
    let mut current = *group_id;
    // See `check_group_membership_path` for why this is bounded
    // at `MAX_NAMESPACE_DEPTH + 1` rather than `MAX_NAMESPACE_DEPTH`:
    // the membership walks need one extra iteration past the chain
    // length to "fall off" at the namespace root, where
    // `is_open_chain_to_namespace` exits early via `parent == ns_id`.
    // Matching effective depth here keeps governance authority and
    // crypto-key selection in agreement at the boundary.
    for _ in 0..=MAX_NAMESPACE_DEPTH {
        if get_subgroup_visibility(store, &current)? != VisibilityMode::Open {
            return Ok(false);
        }
        let Some(parent) = get_parent_group(store, &current)? else {
            return Ok(false);
        };
        if is_group_admin(store, &parent, identity)? {
            return Ok(true);
        }
        current = parent;
    }
    bail!(
        "is_inherited_admin exceeded MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); \
         possible cycle in store"
    )
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

/// Public-key-only view of the current member set for a namespace.
///
/// A namespace is identified by the `[u8; 32]` of its root group, so this
/// is `list_group_members(store, ContextGroupId::from(namespace_id), 0, usize::MAX)`
/// projected to the public-key column. Used by `verify_ack` to confirm
/// that an ack signer is a current namespace member at this node's
/// local DAG view.
pub fn namespace_member_pubkeys(
    store: &Store,
    namespace_id: [u8; 32],
) -> EyreResult<Vec<PublicKey>> {
    let group_id = ContextGroupId::from(namespace_id);
    let members = list_group_members(store, &group_id, 0, usize::MAX)?;
    Ok(members.into_iter().map(|(pk, _role)| pk).collect())
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

/// Returns `true` iff `identity` has a **direct** membership row in
/// `group_id` — never walks the parent chain. Use this (not
/// [`check_group_membership`]) when the caller's intent is "would I be
/// creating a duplicate direct row?": idempotency guards before
/// `add_group_member`, repair paths in the namespace-meta handler,
/// TEE-attestation deduplication, etc. Mirrors the role-aware
/// [`is_direct_group_admin`] for the membership-row case.
pub fn has_direct_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    has_direct_member(store, group_id, identity)
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
