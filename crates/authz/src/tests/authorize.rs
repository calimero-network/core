//! One test per authority plane the fold decides: data, ownership, group
//! admin, root admin, the account plane, and the default-write rule that
//! decides everything with no explicit ACL.

use std::collections::BTreeMap;

use calimero_account::{AccountId, DeviceId, KemPublicKey, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

use super::support::{
    bind_account, bind_test_devices, hlc0, membership_view, op_with, view_with_writer,
};
use crate::authorize::authorize;
use crate::error::Rejected;
use crate::view::AclView;

#[test]
fn put_requires_write_capability() {
    let author = AccountId::from([1u8; 32]);
    let entity = Id::new([2u8; 32]);
    let op = op_with(
        author,
        OpPayload::Put {
            entity,
            value: vec![1],
        },
    );

    // Writer with WRITE → ok.
    assert!(authorize(&op, &view_with_writer(entity, author, OpMask::WRITE)).is_ok());
    // No entry → rejected.
    assert_eq!(
        authorize(&op, &bind_account(AclView::default(), author)),
        Err(Rejected::NotPermitted {
            required: OpMask::WRITE
        })
    );
    // A different writer holding the cap doesn't authorize this author.
    let other = AccountId::from([9u8; 32]);
    assert!(authorize(&op, &view_with_writer(entity, other, OpMask::FULL)).is_err());
}

#[test]
fn delete_requires_delete_capability() {
    let author = AccountId::from([1u8; 32]);
    let entity = Id::new([2u8; 32]);
    let op = op_with(author, OpPayload::Delete { entity });
    // WRITE alone is not enough for a delete.
    assert!(authorize(&op, &view_with_writer(entity, author, OpMask::WRITE)).is_err());
    assert!(authorize(&op, &view_with_writer(entity, author, OpMask::FULL)).is_ok());
}

#[test]
fn set_writers_requires_owner_admin_bit() {
    let author = AccountId::from([1u8; 32]);
    let object = Id::new([2u8; 32]);
    let op = op_with(
        author,
        OpPayload::SetWriters {
            object,
            writers: BTreeMap::new(),
        },
    );
    // WRITE-only is not ownership.
    assert_eq!(
        authorize(&op, &view_with_writer(object, author, OpMask::WRITE)),
        Err(Rejected::NotOwner)
    );
    // ADMIN bit confers ownership.
    assert!(authorize(&op, &view_with_writer(object, author, OpMask::ADMIN)).is_ok());
}

#[test]
fn member_change_requires_group_admin() {
    let admin = AccountId::from([1u8; 32]);
    let stranger = AccountId::from([2u8; 32]);
    let group = ContextGroupId::from([3u8; 32]);
    let newcomer = AccountId::from([4u8; 32]);

    let mut groups = BTreeMap::new();
    groups.insert(
        group,
        [(admin, GroupMemberRole::Admin)].into_iter().collect(),
    );
    let view = bind_account(
        bind_test_devices(AclView {
            groups,
            ..Default::default()
        }),
        stranger,
    );

    let by_admin = op_with(
        admin,
        OpPayload::MemberAdded {
            group,
            member: newcomer,
            role: GroupMemberRole::Member,
        },
    );
    let by_stranger = op_with(
        stranger,
        OpPayload::MemberRemoved {
            group,
            member: admin,
        },
    );
    assert!(authorize(&by_admin, &view).is_ok());
    assert_eq!(authorize(&by_stranger, &view), Err(Rejected::NotGroupAdmin));
}

#[test]
fn admin_ops_require_root_admin() {
    let root = AccountId::from([1u8; 32]);
    let other = AccountId::from([2u8; 32]);
    let view = bind_account(
        bind_test_devices(AclView {
            root_admin: Some(root),
            ..Default::default()
        }),
        other,
    );
    let op = op_with(other, OpPayload::AdminChanged { new_admin: other });
    assert_eq!(authorize(&op, &view), Err(Rejected::NotRootAdmin));
    let op_ok = op_with(
        root,
        OpPayload::PolicyUpdated {
            policy_bytes: vec![],
        },
    );
    assert!(authorize(&op_ok, &view).is_ok());
}

// ---- account plane ----

#[test]
fn a_rotation_authored_by_another_account_is_refused_with_the_roles_named() {
    // Only the account may roll its own key. The refusal used to reuse
    // `DeviceAccountMismatch`, whose fields mean "the account the device is bound
    // to" and "the account the op claimed" — and it passed the handoff's account
    // as `bound`. That is backwards: `check_device_speaks_for_author` has already
    // established the author's account, so the author is the settled identity and
    // the handoff's account is the claim. The message then named both accounts in
    // the wrong roles, on a rotation where no device binding is in question at
    // all.
    //
    // Asserting the payload, not just the rejection: getting refused was never
    // the bug.
    let owner = AccountId::from([0x11; 32]);
    let stranger = AccountId::from([0x22; 32]);
    let view = bind_account(bind_test_devices(AclView::default()), stranger);

    let handoff = RootKeyHandoff {
        account: owner,
        from_epoch: 0,
        new_root_sign_pk: calimero_primitives::identity::PrivateKey::from([0x33; 32]).public_key(),
        signature: [0u8; 64],
    };
    let op = op_with(stranger, OpPayload::AccountKeysRotated { handoff });

    match authorize(&op, &view) {
        Err(Rejected::RotationNotByAccount { account, author }) => {
            assert_eq!(
                account, owner,
                "`account` must name what the handoff rotates"
            );
            assert_eq!(author, stranger, "`author` must name who actually signed");
        }
        other => panic!("expected RotationNotByAccount, got {other:?}"),
    }
}

// ---- default-write = membership ----

#[test]
fn default_write_lets_a_member_write_a_non_restricted_entity() {
    // kv-store context: Bob is a member, no per-key ACL. Bob may Put/Delete
    // any key; Carol (non-member) may not.
    let group = ContextGroupId::from([0x33; 32]);
    let bob = AccountId::from([0xB0; 32]);
    let carol = AccountId::from([0xC0; 32]);
    let view = membership_view(group, bob, GroupMemberRole::Member);
    let x = Id::new([0x11; 32]);

    assert!(authorize(
        &op_with(
            bob,
            OpPayload::Put {
                entity: x,
                value: vec![5]
            }
        ),
        &view
    )
    .is_ok());
    assert!(authorize(&op_with(bob, OpPayload::Delete { entity: x }), &view).is_ok());
    assert_eq!(
        authorize(
            &op_with(
                carol,
                OpPayload::Put {
                    entity: x,
                    value: vec![5]
                }
            ),
            &view
        ),
        Err(Rejected::NotPermitted {
            required: OpMask::WRITE
        })
    );
}

#[test]
fn default_write_does_not_grant_a_member_setwriters() {
    // A plain member gets WRITE+DELETE on default entities but NOT ADMIN —
    // rotating an object's writer set needs an explicit ownership grant.
    let group = ContextGroupId::from([0x33; 32]);
    let bob = AccountId::from([0xB0; 32]);
    let view = membership_view(group, bob, GroupMemberRole::Member);
    let x = Id::new([0x11; 32]);
    assert_eq!(
        authorize(
            &op_with(
                bob,
                OpPayload::SetWriters {
                    object: x,
                    writers: BTreeMap::new()
                }
            ),
            &view
        ),
        Err(Rejected::NotOwner)
    );
}

#[test]
fn explicit_acl_overrides_default_write_for_restricted_objects() {
    // `secret` carries an explicit ACL {Alice: FULL}. Bob is a context
    // member but NOT a writer of `secret` → denied (the old coarse
    // per-delta gate would have let him through; the unified check is
    // strictly tighter). Alice → ok.
    let group = ContextGroupId::from([0x33; 32]);
    let alice = AccountId::from([0xA1; 32]);
    let bob = AccountId::from([0xB0; 32]);
    let secret = Id::new([0x5E; 32]);

    let mut view = membership_view(group, bob, GroupMemberRole::Member);
    // Both are members; only Alice is a writer of the restricted object.
    view.groups
        .get_mut(&group)
        .unwrap()
        .insert(alice, GroupMemberRole::Admin);
    view.acl
        .insert(secret, [(alice, OpMask::FULL)].into_iter().collect());
    // Alice joined the view after `membership_view` built it, so bind her
    // device explicitly.
    let view = bind_account(view, alice);

    assert!(authorize(
        &op_with(
            alice,
            OpPayload::Put {
                entity: secret,
                value: vec![1]
            }
        ),
        &view
    )
    .is_ok());
    assert_eq!(
        authorize(
            &op_with(
                bob,
                OpPayload::Put {
                    entity: secret,
                    value: vec![1]
                }
            ),
            &view
        ),
        Err(Rejected::NotPermitted {
            required: OpMask::WRITE
        })
    );
}

/// **A join must be signed by the device its certificate grants to.**
///
/// This arm ran the cryptographic admissibility check and nothing else, because
/// while an op's authorship was derived from its signing key the device it
/// named was fiction — comparing against it refused every honest join, so the
/// comparison was removed and a comment left in its place. Now that a projected
/// op carries the device its certificate names, the check is decidable again.
///
/// What it stops: a certificate is public once observed. Without requiring
/// possession of the granted key, anyone who saw one could replay it and join
/// on the real device's behalf. Here the certificate is perfectly valid and the
/// op is simply authored by somebody else's device — an admin's, no less, so
/// the policy gate below would wave it through.
#[test]
fn a_join_replayed_by_another_device_is_refused() {
    let admin = AccountId::from([1u8; 32]);
    let group = ContextGroupId::from([9u8; 32]);

    let root_sk = calimero_primitives::identity::PrivateKey::from([0x11u8; 32]);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let joiner_key = calimero_primitives::identity::PrivateKey::from([0x22u8; 32]).public_key();
    let granted_device = DeviceId::from([0x4D; 32]);
    let cert = calimero_account::sign_device_cert(
        &root_sk,
        genesis.account_id(),
        granted_device,
        &joiner_key,
        &KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign the device cert");

    let payload = OpPayload::MemberJoinedWithDevice {
        group,
        member: genesis.account_id(),
        role: GroupMemberRole::Member,
        genesis,
        chain: vec![],
        cert,
    };

    // Authored by the ADMIN's device rather than the granted one: the replay.
    let replayed = op_with(admin, payload.clone());
    let view = bind_account(
        membership_view(group, genesis.account_id(), GroupMemberRole::Admin),
        admin,
    );
    assert!(
        matches!(
            authorize(&replayed, &view),
            Err(Rejected::DeviceKeyStale { .. })
        ),
        "a join carrying somebody else's certificate must be refused for not \
         being signed by the device that certificate grants to"
    );

    // The same op, authored by the device the certificate actually names, is
    // admitted — so the check above rejects the replay rather than the join.
    let honest = Op::new(
        ScopeId::from([0u8; 32]),
        vec![],
        Authorship {
            account: genesis.account_id(),
            device: granted_device,
            device_key: joiner_key,
        },
        hlc0(),
        payload,
        [0u8; 32],
        [0u8; 64],
    );
    assert!(
        authorize(&honest, &view).is_ok(),
        "the honest join must still pass, or the check above proves nothing"
    );
}
