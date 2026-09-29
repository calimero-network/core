//! Apply-level tests that a governance op cannot reach past its own group or namespace.

use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

use crate::test_fixtures::{
    enrol_member, enrolled, nest_for_test, test_group_id, test_meta, test_store,
};
use crate::{
    apply_local_signed_group_op, get_group_for_context, register_context_in_group,
    CapabilitiesRepository, MembershipRepository, MetaRepository,
};

#[test]
fn context_registered_cannot_move_a_context_out_of_another_group() {
    let store = test_store();
    let ns = ContextGroupId::from([0xD0; 32]);
    let group_a = ContextGroupId::from([0xD1; 32]);
    let group_b = ContextGroupId::from([0xD2; 32]);
    MetaRepository::new(&store).save(&ns, &test_meta()).unwrap();
    for group in [group_a, group_b] {
        MetaRepository::new(&store)
            .save(&group, &test_meta())
            .unwrap();
        nest_for_test(&store, &ns, &group);
    }
    let context = ContextId::from([0xD3; 32]);
    register_context_in_group(&store, &group_a, &context).unwrap();

    let creator_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let creator = enrol_member(&store, &ns, &creator_sk.public_key());
    MembershipRepository::new(&store)
        .add_member(&group_b, &creator, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &group_b,
            &creator,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .unwrap();

    let register = |context_id: ContextId, nonce: u64| {
        SignedGroupOp::sign(
            &creator_sk,
            group_b.to_bytes().into(),
            vec![],
            nonce,
            GroupOp::ContextRegistered {
                context_id,
                application_id: calimero_primitives::application::ApplicationId::from([0u8; 32]),
                blob_id: calimero_primitives::blobs::BlobId::from([0u8; 32]),
                source: String::new(),
                service_name: None,
                package: "com.example.app".to_owned(),
                version: "1.0.0".to_owned(),
            },
        )
        .unwrap()
    };
    apply_local_signed_group_op(&store, &register(ContextId::from([0xD4; 32]), 1))
        .expect("control: registering a fresh context in group B applies");

    let res = apply_local_signed_group_op(&store, &register(context, 2));
    let owner = get_group_for_context(&store, &context).unwrap();
    assert!(
        res.is_err() && owner == Some(group_a),
        "group-B ContextRegistered for a group-A context: applied={} still_in_group_a={}",
        res.is_ok(),
        owner == Some(group_a),
    );
}

#[test]
fn noop_group_op_from_a_non_member_is_rejected() {
    let store = test_store();
    let gid = test_group_id();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    let admin_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let admin = enrol_member(&store, &gid, &admin_sk.public_key());
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    let noop = |sk: &PrivateKey| {
        SignedGroupOp::sign(sk, gid.to_bytes().into(), vec![], 1, GroupOp::Noop).unwrap()
    };
    apply_local_signed_group_op(&store, &noop(&admin_sk))
        .expect("control: a member's Noop applies");

    let stranger_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    assert!(
        apply_local_signed_group_op(&store, &noop(&stranger_sk)).is_err(),
        "a signer bound to no account in the group got a Noop applied"
    );
}

#[test]
fn member_added_cannot_demote_an_admin_without_admin_authority() {
    let store = test_store();
    let gid = test_group_id();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    let (_, admin) = enrolled(&store, &gid, 0x61);
    let manager_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let manager = enrol_member(&store, &gid, &manager_sk.public_key());
    let members = MembershipRepository::new(&store);
    members
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    members
        .add_member(&gid, &manager, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &manager, MemberCapabilities::MANAGE_MEMBERS.bits())
        .unwrap();

    let add_as_member = |member, nonce| {
        SignedGroupOp::sign(
            &manager_sk,
            gid.to_bytes().into(),
            vec![],
            nonce,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap()
    };
    let (_, newcomer) = enrolled(&store, &gid, 0x63);
    apply_local_signed_group_op(&store, &add_as_member(newcomer, 1))
        .expect("control: the manager adds a new member");

    let res = apply_local_signed_group_op(&store, &add_as_member(admin, 2));
    let role = members.role_of(&gid, &admin).unwrap();
    assert!(
        res.is_err() && role == Some(GroupMemberRole::Admin),
        "MANAGE_MEMBERS holder re-adding an admin as Member: applied={} admin_role_now={role:?}",
        res.is_ok(),
    );
}
