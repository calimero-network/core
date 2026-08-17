//! Fixtures shared by the test files beside this one.
//!
//! Everything here builds a synthetic [`AclView`] or an [`Op`] to decide against
//! it. Nothing folds an op log — that is `calimero-projection`'s job, and the
//! whole reason this crate takes the view as an argument.

use std::collections::BTreeMap;

use core::num::NonZeroU128;

use calimero_account::{AccountId, DeviceId, KemPublicKey};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

use crate::view::{AclView, DeviceBinding, SubgroupEdge};

pub(crate) fn hlc0() -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(0),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

/// The device a test account authors from. Deterministic so
/// [`bind_test_devices`] can register exactly the binding [`op_with`] uses.
pub(crate) fn test_device(account: AccountId) -> DeviceId {
    DeviceId::from(*account.as_bytes())
}

/// The key that device signs with. Never verified — `authorize` assumes
/// `Op::verify` already ran — so any deterministic value will do.
pub(crate) fn test_device_key(account: AccountId) -> PublicKey {
    PublicKey::from(*account.as_bytes())
}

pub(crate) fn op_with(author: AccountId, payload: OpPayload) -> Op {
    Op::new(
        ScopeId::from([0u8; 32]),
        vec![],
        Authorship {
            account: author,
            device: test_device(author),
            device_key: test_device_key(author),
        },
        hlc0(),
        payload,
        [0u8; 32],
        [0u8; 64],
    )
}

/// Register a binding for one account the view does not otherwise mention.
///
/// Needed wherever a test drives an *outsider* — a stranger, a non-admin — at
/// the policy layer: without a binding they would be turned away by the device
/// precondition, and the test would stop proving anything about the policy rule
/// it names.
pub(crate) fn bind_account(mut view: AclView, account: AccountId) -> AclView {
    let _ = view.devices.insert(
        test_device(account),
        DeviceBinding {
            account,
            sign_pk: test_device_key(account),
            kem_pk: KemPublicKey::from([0u8; 32]),
            device_epoch: 0,
            key_epoch: 0,
        },
    );
    view
}

/// Register a device binding for every account the view mentions, standing in
/// for the `DeviceLinked` ops a real log would carry.
///
/// These tests predate accounts and exercise the *policy* layer — who may write,
/// who is an admin, how membership inherits. Binding every mentioned account
/// keeps them aimed at that, instead of every one of them tripping the device
/// precondition first.
pub(crate) fn bind_test_devices(mut view: AclView) -> AclView {
    let mut accounts: Vec<AccountId> = Vec::new();
    accounts.extend(view.acl.values().flat_map(|w| w.keys().copied()));
    accounts.extend(view.groups.values().flat_map(|m| m.keys().copied()));
    accounts.extend(view.root_admin);
    accounts.extend(view.group_admin.values().copied());
    accounts.extend(view.member_caps.keys().map(|(_, m)| *m));
    for account in accounts {
        let _ = view.devices.insert(
            test_device(account),
            DeviceBinding {
                account,
                sign_pk: test_device_key(account),
                kem_pk: KemPublicKey::from([0u8; 32]),
                device_epoch: 0,
                key_epoch: 0,
            },
        );
    }
    view
}

pub(crate) fn view_with_writer(entity: Id, who: AccountId, mask: OpMask) -> AclView {
    let mut acl = BTreeMap::new();
    acl.insert(entity, [(who, mask)].into_iter().collect());
    bind_test_devices(AclView {
        acl,
        ..Default::default()
    })
}

// Build a view: parent group with `member` (holding `caps`), an open
// subgroup `child` nested under `parent`. Mirrors the inheritance scenario.
pub(crate) fn inheritance_view(
    parent: ContextGroupId,
    child: ContextGroupId,
    member: AccountId,
    caps: u32,
    child_restricted: bool,
    parent_has_member: bool,
) -> AclView {
    let mut groups: BTreeMap<ContextGroupId, BTreeMap<AccountId, GroupMemberRole>> =
        BTreeMap::new();
    if parent_has_member {
        groups.insert(
            parent,
            [(member, GroupMemberRole::Member)].into_iter().collect(),
        );
    }
    let mut member_caps = BTreeMap::new();
    member_caps.insert((parent, member), caps);
    let mut subgroups = BTreeMap::new();
    subgroups.insert(
        ScopeId::from(child.to_bytes()),
        SubgroupEdge {
            parent: ScopeId::from(parent.to_bytes()),
            restricted: child_restricted,
        },
    );
    AclView {
        groups,
        member_caps,
        subgroups,
        ..Default::default()
    }
}

pub(crate) fn membership_view(
    group: ContextGroupId,
    member: AccountId,
    role: GroupMemberRole,
) -> AclView {
    let mut groups = BTreeMap::new();
    groups.insert(group, [(member, role)].into_iter().collect());
    // Carol (the non-member these tests contrast against) is bound too, so
    // her rejection proves the membership rule rather than the device one.
    bind_account(
        bind_test_devices(AclView {
            groups,
            ..Default::default()
        }),
        AccountId::from([0xC0; 32]),
    )
}
