//! Membership plane: [`payload_from_group_op`], the account arms it folds, and
//! the add/remove/re-add fold the governance state machine has to agree with.

use calimero_account::{
    sign_account_endorsement, sign_device_cert, AccountGenesis, AccountId, DeviceId, KemPublicKey,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_types::GroupOp;
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_projection::ScopeState;

use crate::tests::support::{authorship_of, hlc};
use crate::{legacy_account_id, payload_from_group_op};

/// A governance `AccountDeviceLinked` must reach the projection's account
/// plane and land as a folded device binding.
///
/// Before the account arms existed this returned `None`, the op folded to
/// `Noop`, and `AclView.devices` stayed empty on the governance path — so the
/// complete, tested account plane in `crates/projection` guarded nothing, and
/// per-device authorization had no way to resolve a signing key to the account
/// it speaks for at a causal cut.
#[test]
fn a_governance_device_link_reaches_the_projection() {
    let root = PrivateKey::from([1u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [5u8; 16]);
    let cert = sign_device_cert(
        &root,
        account,
        device,
        &PrivateKey::from([5u8; 32]).public_key(),
        &KemPublicKey::from([5u8; 32]),
        0,
        0,
    )
    .expect("sign cert");

    let group = ContextGroupId::from([9u8; 32]);
    let payload = payload_from_group_op(
        group,
        &GroupOp::AccountDeviceLinked {
            genesis,
            chain: vec![],
            cert,
            endorsement: sign_account_endorsement(&root, account).expect("sign endorsement"),
        },
    )
    .expect("the account ops must map to a unified payload, not fold to Noop");
    assert!(matches!(payload, OpPayload::DeviceLinked { .. }));

    // And it must actually fold: a payload that never becomes a binding would
    // satisfy the assertion above while leaving the plane just as blind.
    let op = Op::from_parts(
        [7u8; 32],
        ScopeId::from([9u8; 32]),
        vec![],
        authorship_of(AccountId::from([0xA0; 32]), root.public_key()),
        hlc(1),
        payload,
        [0u8; 32],
        [0u8; 64],
    );
    let mut state = ScopeState::default();
    state.apply(&op);
    let view = state.acl_view();
    assert!(
        view.devices.contains_key(&device),
        "the folded view must know the device, or per-device authorization has \
         nothing to resolve against"
    );
    assert_eq!(
        view.devices.get(&device).map(|b| b.account),
        Some(account),
        "and it must know which account the device speaks for"
    );

    // And the reason the binding has to be consulted at all: an account
    // DERIVED from the device's signing key is a different account, and not a
    // member. Resolving a signer through `legacy_account_id` alone therefore
    // answers about somebody who does not exist, which is exactly why a second
    // device could receive scope keys and then not author with them.
    let device_sign_pk = PrivateKey::from([5u8; 32]).public_key();
    assert_ne!(
        legacy_account_id(&device_sign_pk),
        account,
        "if the derived account happened to equal the real one, the check \
         below would pass for the wrong reason"
    );
    assert!(
        !view.is_scope_member(&legacy_account_id(&device_sign_pk)),
        "the account derived from a device key must not be a member"
    );
}

#[test]
fn group_op_encoder_mapping() {
    let group = ContextGroupId::from([3u8; 32]);
    // The op names an account, and the payload carries it through verbatim —
    // there is no derivation left in this encoder to get wrong.
    let m = AccountId::from([0x55; 32]);

    assert_eq!(
        payload_from_group_op(
            group,
            &GroupOp::MemberAdded {
                member: m,
                role: GroupMemberRole::Member,
            },
        ),
        Some(OpPayload::MemberAdded {
            group,
            member: m,
            role: GroupMemberRole::Member,
        })
    );
    // A role change re-asserts membership (ScopeState LWW keeps the latest).
    assert_eq!(
        payload_from_group_op(
            group,
            &GroupOp::MemberRoleSet {
                member: m,
                role: GroupMemberRole::Admin,
            },
        ),
        Some(OpPayload::MemberAdded {
            group,
            member: m,
            role: GroupMemberRole::Admin,
        })
    );
    // A TEE node admitted via attestation is a member with the granted
    // role; the attestation evidence is consumed by the admission gate.
    assert_eq!(
        payload_from_group_op(
            group,
            &GroupOp::MemberJoinedViaTeeAttestation {
                member: m,
                quote_hash: [0u8; 32],
                mrtd: String::new(),
                rtmr0: String::new(),
                rtmr1: String::new(),
                rtmr2: String::new(),
                rtmr3: String::new(),
                tcb_status: String::new(),
                role: GroupMemberRole::ReadOnlyTee,
            },
        ),
        Some(OpPayload::MemberAdded {
            group,
            member: m,
            role: GroupMemberRole::ReadOnlyTee,
        })
    );
    // Ownership transfer sets the group scope's root admin (owner ⇔ ADMIN).
    let new_owner = AccountId::from([0x77; 32]);
    assert_eq!(
        payload_from_group_op(group, &GroupOp::TransferOwnership { new_owner }),
        Some(OpPayload::AdminChanged {
            new_admin: new_owner,
        })
    );
    // Out-of-model ops (metadata, config, …) → None.
    assert_eq!(payload_from_group_op(group, &GroupOp::Noop), None);
    // Capability plane is now folded (gates inherited membership).
    assert_eq!(
        payload_from_group_op(
            group,
            &GroupOp::DefaultCapabilitiesSet {
                capabilities: MemberCapabilities::from_bits_truncate(7)
            }
        ),
        Some(OpPayload::DefaultCapabilitiesSet {
            group,
            capabilities: MemberCapabilities::from_bits_truncate(7),
        })
    );
}

/// A membership op sequence folds through `ScopeState` to the same final
/// membership the governance state machine (what `membership_status_at`
/// resolves) produces: last write wins per member, a removal drops them.
#[test]
fn membership_plane_fold_add_remove_readd() {
    let scope = ScopeId::from([0u8; 32]);
    let group = ContextGroupId::from([3u8; 32]);
    // Principals, named. This test folds add/remove/re-add on the membership
    // plane, which is keyed by account — the keys it used to hash through the
    // legacy stand-in were only a route to one.
    let admin_key = PublicKey::from([1u8; 32]);
    let admin = AccountId::from([1u8; 32]);
    let m = AccountId::from([0x55; 32]);

    let build = |ns: u64, payload: OpPayload| -> Op {
        let h = hlc(ns);
        Op::new(
            scope,
            vec![],
            authorship_of(admin, admin_key),
            h,
            payload,
            [0u8; 32],
            [0u8; 64],
        )
    };

    // Add(Member)@10 → Remove@20 → Add(Admin)@30 → present as Admin.
    let ops = vec![
        build(
            10,
            OpPayload::MemberAdded {
                group,
                member: m,
                role: GroupMemberRole::Member,
            },
        ),
        build(20, OpPayload::MemberRemoved { group, member: m }),
        build(
            30,
            OpPayload::MemberAdded {
                group,
                member: m,
                role: GroupMemberRole::Admin,
            },
        ),
    ];
    let groups = ScopeState::from_ops(&ops).acl_view().groups;
    assert_eq!(
        groups.get(&group).and_then(|g| g.get(&m)),
        Some(&GroupMemberRole::Admin),
        "re-add after remove wins with the new role"
    );

    // Same set ending in Remove@40 → member absent.
    let mut ops2 = ops;
    ops2.push(build(40, OpPayload::MemberRemoved { group, member: m }));
    let groups2 = ScopeState::from_ops(&ops2).acl_view().groups;
    assert_eq!(
        groups2.get(&group).and_then(|g| g.get(&m)),
        None,
        "final removal drops the member"
    );
}
