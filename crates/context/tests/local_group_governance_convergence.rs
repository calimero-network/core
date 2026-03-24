//! Two logical nodes (separate stores) receive the same gossip payloads and converge to identical group membership.
//!
//! Mirrors the node path: `BroadcastMessage::SignedGroupOpV1` carries `borsh(SignedGroupOp)` bytes; each peer decodes
//! and applies via `group_store::apply_local_signed_group_op` (see `network_event.rs` + `apply_signed_group_op` handler).
//! Real libp2p gossip on `group/<hex>` is covered by `calimero-network` (`tests/gossipsub_group_topic.rs`).

use std::sync::Arc;

use borsh::to_vec as borsh_to_vec;
use calimero_context::governance_dag::{signed_op_to_delta, GroupGovernanceApplier};
use calimero_context::group_store::{
    self, add_group_member, apply_local_signed_group_op, get_op_head, list_group_members,
    load_group_meta, read_op_log_after, save_group_meta,
};
use calimero_dag::DagStore;
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, GroupRevealPayloadData, SignedGroupOpenInvitation,
    SignerId,
};
use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::MemberCapabilities;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn sample_group_id() -> ContextGroupId {
    ContextGroupId::from([0x77u8; 32])
}

fn sample_meta(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        migration: None,
    }
}

fn sorted_members(store: &Store, gid: &ContextGroupId) -> Vec<(PublicKey, GroupMemberRole)> {
    let mut v = list_group_members(store, gid, 0, usize::MAX).expect("list_group_members");
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn assert_same_group_view(a: &Store, b: &Store, gid: &ContextGroupId) {
    assert_eq!(
        sorted_members(a, gid),
        sorted_members(b, gid),
        "group membership should match on both nodes"
    );
}

/// Decode and apply as inbound gossip would after `SignedGroupOp` verification.
fn apply_wire_payload(store: &Store, payload: &[u8]) {
    let op: SignedGroupOp = borsh::from_slice(payload).expect("borsh decode SignedGroupOp");
    apply_local_signed_group_op(store, &op).expect("apply_local_signed_group_op");
}

#[test]
fn two_nodes_converge_on_same_signed_op_sequence() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    }

    let new_member = PrivateKey::random(&mut rng).public_key();

    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::MemberAdded {
            member: new_member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign op1");

    let payload1 = borsh_to_vec(&op1).expect("borsh encode op1");

    apply_wire_payload(&store_a, &payload1);
    apply_wire_payload(&store_b, &payload1);

    assert_same_group_view(&store_a, &store_b, &gid);
    assert!(group_store::check_group_membership(&store_a, &gid, &new_member).unwrap());
    assert!(group_store::check_group_membership(&store_b, &gid, &new_member).unwrap());

    let op2 = SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], 2, GroupOp::Noop).expect("sign op2");
    let payload2 = borsh_to_vec(&op2).expect("borsh encode op2");

    apply_wire_payload(&store_a, &payload2);
    apply_wire_payload(&store_b, &payload2);

    assert_same_group_view(&store_a, &store_b, &gid);

    assert_eq!(
        group_store::get_local_gov_nonce(&store_a, &gid, &admin_pk)
            .unwrap()
            .unwrap(),
        2
    );
    assert_eq!(
        group_store::get_local_gov_nonce(&store_b, &gid, &admin_pk)
            .unwrap()
            .unwrap(),
        2
    );
}

#[test]
fn two_nodes_converge_on_target_application_and_migration() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    }

    let new_target = ApplicationId::from([0xEE; 32]);

    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::TargetApplicationSet {
            app_key: [0x11; 32],
            target_application_id: new_target,
        },
    )
    .expect("sign TargetApplicationSet");

    let op2 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        2,
        GroupOp::GroupMigrationSet {
            migration: Some(b"v1-migration".to_vec()),
        },
    )
    .expect("sign GroupMigrationSet");

    let payload1 = borsh_to_vec(&op1).expect("encode op1");
    let payload2 = borsh_to_vec(&op2).expect("encode op2");

    apply_wire_payload(&store_a, &payload1);
    apply_wire_payload(&store_b, &payload1);
    apply_wire_payload(&store_a, &payload2);
    apply_wire_payload(&store_b, &payload2);

    let meta_a = load_group_meta(&store_a, &gid).unwrap().expect("meta a");
    let meta_b = load_group_meta(&store_b, &gid).unwrap().expect("meta b");

    assert_eq!(meta_a.target_application_id, new_target);
    assert_eq!(meta_a.app_key, [0x11; 32]);
    assert_eq!(meta_a.migration, Some(b"v1-migration".to_vec()));
    assert_eq!(meta_a.target_application_id, meta_b.target_application_id);
    assert_eq!(meta_a.app_key, meta_b.app_key);
    assert_eq!(meta_a.migration, meta_b.migration);
}

#[test]
fn two_nodes_converge_on_join_with_invitation_claim() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    }

    // Matches `create_group_invitation` when `group_governance = local` (synthetic coordinates).
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_pk.digest()),
        group_id: gid,
        expiration_height: 9_999_999,
        secret_salt: [0x42; 32],
        protocol: "local".to_owned(),
        network: "local".to_owned(),
        contract_id: "local".to_owned(),
    };

    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_hash = Sha256::digest(&inv_bytes);
    let inv_sig = admin_sk.sign(&inv_hash).expect("sign invitation");
    let signed_invitation = SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
    };

    let reveal = GroupRevealPayloadData {
        signed_open_invitation: signed_invitation.clone(),
        new_member_identity: SignerId::from(*joiner_pk.digest()),
    };
    let reveal_bytes = borsh::to_vec(&reveal).expect("borsh reveal");
    let reveal_hash = Sha256::digest(&reveal_bytes);
    let join_sig = joiner_sk.sign(&reveal_hash).expect("sign reveal");
    let invitee_signature_hex = hex::encode(join_sig.to_bytes());

    let op = SignedGroupOp::sign(
        &joiner_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::JoinWithInvitationClaim {
            signed_invitation,
            invitee_signature_hex,
        },
    )
    .expect("sign JoinWithInvitationClaim");

    let payload = borsh_to_vec(&op).expect("encode join op");

    apply_wire_payload(&store_a, &payload);
    apply_wire_payload(&store_b, &payload);

    assert_same_group_view(&store_a, &store_b, &gid);
    assert!(group_store::check_group_membership(&store_a, &gid, &joiner_pk).unwrap());
    assert!(group_store::check_group_membership(&store_b, &gid, &joiner_pk).unwrap());
}

/// Same op order as `create_context` under local governance when default visibility is set:
/// `ContextRegistered` → `ContextVisibilitySet` (group default applied to new context).
#[test]
fn two_nodes_converge_on_context_visibility_after_create() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_pk = PrivateKey::random(&mut rng).public_key();
    let creator_sk = PrivateKey::random(&mut rng);
    let creator_pk = creator_sk.public_key();

    let context_id = ContextId::from([0xCD; 32]);

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
        add_group_member(store, &gid, &creator_pk, GroupMemberRole::Member).unwrap();
        group_store::set_member_capability(
            store,
            &gid,
            &creator_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )
        .unwrap();
        group_store::set_default_visibility(store, &gid, 0).unwrap();
    }

    let op1 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::ContextRegistered { context_id },
    )
    .expect("sign ContextRegistered");
    let op2 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        2,
        GroupOp::ContextVisibilitySet {
            context_id,
            mode: 0,
            creator: creator_pk,
        },
    )
    .expect("sign ContextVisibilitySet");

    for payload in [borsh_to_vec(&op1).expect("encode op1"), borsh_to_vec(&op2).expect("encode op2")]
    {
        apply_wire_payload(&store_a, &payload);
        apply_wire_payload(&store_b, &payload);
    }

    let vis_a = group_store::get_context_visibility(&store_a, &gid, &context_id)
        .unwrap()
        .expect("visibility on a");
    let vis_b = group_store::get_context_visibility(&store_b, &gid, &context_id)
        .unwrap()
        .expect("visibility on b");
    assert_eq!(vis_a, vis_b);
    assert_eq!(vis_a.0, 0);
    assert_eq!(vis_a.1, *creator_pk);
}

/// `create_context` uses **Open (0)** when the group has no `GroupDefaultVis`; same wire shape.
#[test]
fn two_nodes_converge_on_context_visibility_without_group_default() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_pk = PrivateKey::random(&mut rng).public_key();
    let creator_sk = PrivateKey::random(&mut rng);
    let creator_pk = creator_sk.public_key();

    let context_id = ContextId::from([0xCE; 32]);

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
        add_group_member(store, &gid, &creator_pk, GroupMemberRole::Member).unwrap();
        group_store::set_member_capability(
            store,
            &gid,
            &creator_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )
        .unwrap();
    }

    let op1 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::ContextRegistered { context_id },
    )
    .expect("sign ContextRegistered");
    let op2 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        2,
        GroupOp::ContextVisibilitySet {
            context_id,
            mode: 0,
            creator: creator_pk,
        },
    )
    .expect("sign ContextVisibilitySet");

    for payload in [borsh_to_vec(&op1).expect("encode op1"), borsh_to_vec(&op2).expect("encode op2")]
    {
        apply_wire_payload(&store_a, &payload);
        apply_wire_payload(&store_b, &payload);
    }

    let vis_a = group_store::get_context_visibility(&store_a, &gid, &context_id)
        .unwrap()
        .expect("visibility on a");
    let vis_b = group_store::get_context_visibility(&store_b, &gid, &context_id)
        .unwrap()
        .expect("visibility on b");
    assert_eq!(vis_a, vis_b);
    assert_eq!(vis_a.0, 0);
    assert_eq!(vis_a.1, *creator_pk);
}

#[test]
fn two_nodes_converge_on_context_alias_as_creator() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let creator_sk = PrivateKey::random(&mut rng);
    let creator_pk = creator_sk.public_key();

    let context_id = ContextId::from([0xAB; 32]);

    for store in [&store_a, &store_b] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
        add_group_member(store, &gid, &creator_pk, GroupMemberRole::Member).unwrap();
        group_store::set_member_capability(
            store,
            &gid,
            &creator_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )
        .unwrap();
    }

    let op1 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::ContextRegistered { context_id },
    )
    .expect("sign ContextRegistered");
    let op2 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        2,
        GroupOp::ContextVisibilitySet {
            context_id,
            mode: 0,
            creator: creator_pk,
        },
    )
    .expect("sign ContextVisibilitySet");
    let op3 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        3,
        GroupOp::ContextAliasSet {
            context_id,
            alias: "wire-alias".to_owned(),
        },
    )
    .expect("sign ContextAliasSet");

    for payload in [
        borsh_to_vec(&op1).expect("encode op1"),
        borsh_to_vec(&op2).expect("encode op2"),
        borsh_to_vec(&op3).expect("encode op3"),
    ] {
        apply_wire_payload(&store_a, &payload);
        apply_wire_payload(&store_b, &payload);
    }

    assert_eq!(
        group_store::get_context_alias(&store_a, &gid, &context_id)
            .unwrap()
            .as_deref(),
        Some("wire-alias")
    );
    assert_eq!(
        group_store::get_context_alias(&store_b, &gid, &context_id)
            .unwrap()
            .as_deref(),
        Some("wire-alias")
    );
}

#[test]
fn op_log_records_applied_ops_and_head_advances() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    assert!(get_op_head(&store, &gid).unwrap().is_none());

    let new_member = PrivateKey::random(&mut rng).public_key();
    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::MemberAdded {
            member: new_member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign op1");

    apply_local_signed_group_op(&store, &op1).unwrap();

    let head = get_op_head(&store, &gid).unwrap().expect("head after op1");
    assert_eq!(head.sequence, 1);
    assert!(head.dag_heads.contains(&op1.content_hash().unwrap()));

    let log = read_op_log_after(&store, &gid, 0, 100).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, 1);
    let decoded: SignedGroupOp = borsh::from_slice(&log[0].1).unwrap();
    assert_eq!(decoded.nonce, op1.nonce);

    let op2 = SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], 2, GroupOp::Noop).expect("sign op2");
    apply_local_signed_group_op(&store, &op2).unwrap();

    let head2 = get_op_head(&store, &gid).unwrap().expect("head after op2");
    assert_eq!(head2.sequence, 2);

    let log_after_1 = read_op_log_after(&store, &gid, 1, 100).unwrap();
    assert_eq!(log_after_1.len(), 1);
    assert_eq!(log_after_1[0].0, 2);

    let full_log = read_op_log_after(&store, &gid, 0, 100).unwrap();
    assert_eq!(full_log.len(), 2);
}

#[test]
fn duplicate_op_is_idempotent() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let op = SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], 1, GroupOp::Noop).expect("sign op");
    let payload = borsh_to_vec(&op).expect("encode");

    apply_wire_payload(&store, &payload);
    apply_wire_payload(&store, &payload);
    apply_wire_payload(&store, &payload);

    let head = get_op_head(&store, &gid).unwrap().expect("head");
    assert_eq!(head.sequence, 1);
    let log = read_op_log_after(&store, &gid, 0, 100).unwrap();
    assert_eq!(log.len(), 1);
}

#[test]
fn offline_node_replays_missed_ops_from_log() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let store_online = empty_store();
    let store_offline = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    for store in [&store_online, &store_offline] {
        save_group_meta(store, &gid, &sample_meta(admin_pk)).unwrap();
        add_group_member(store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    }

    let member1 = PrivateKey::random(&mut rng).public_key();
    let member2 = PrivateKey::random(&mut rng).public_key();

    let op1 = SignedGroupOp::sign(
        &admin_sk, gid_bytes, vec![], 1,
        GroupOp::MemberAdded { member: member1, role: GroupMemberRole::Member },
    ).unwrap();
    let op2 = SignedGroupOp::sign(
        &admin_sk, gid_bytes, vec![], 2,
        GroupOp::MemberAdded { member: member2, role: GroupMemberRole::Member },
    ).unwrap();

    for op in [&op1, &op2] {
        apply_local_signed_group_op(&store_online, op).unwrap();
    }

    assert!(group_store::check_group_membership(&store_online, &gid, &member1).unwrap());
    assert!(group_store::check_group_membership(&store_online, &gid, &member2).unwrap());
    assert!(!group_store::check_group_membership(&store_offline, &gid, &member1).unwrap());

    let missed_ops = read_op_log_after(&store_online, &gid, 0, 100).unwrap();
    assert_eq!(missed_ops.len(), 2);

    for (_seq, op_bytes) in &missed_ops {
        let op: SignedGroupOp = borsh::from_slice(op_bytes).unwrap();
        apply_local_signed_group_op(&store_offline, &op).unwrap();
    }

    assert_same_group_view(&store_online, &store_offline, &gid);
    assert!(group_store::check_group_membership(&store_offline, &gid, &member1).unwrap());
    assert!(group_store::check_group_membership(&store_offline, &gid, &member2).unwrap());
}

#[tokio::test]
async fn dag_applies_ops_in_causal_order() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member1 = PrivateKey::random(&mut rng).public_key();
    let member2 = PrivateKey::random(&mut rng).public_key();

    // op1: add member1 (genesis parent)
    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        GroupOp::MemberAdded {
            member: member1,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    let delta1 = signed_op_to_delta(&op1).unwrap();

    // op2: add member2 (parent = op1)
    let op1_hash = op1.content_hash().unwrap();
    let op2 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![op1_hash],
        2,
        GroupOp::MemberAdded {
            member: member2,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    let delta2 = signed_op_to_delta(&op2).unwrap();

    let applier = GroupGovernanceApplier::new(store.clone());
    let mut dag = DagStore::new([0u8; 32]);

    // Apply op2 first — should be pending (parent op1 not yet in DAG)
    let applied = dag.add_delta(delta2, &applier).await.unwrap();
    assert!(!applied, "op2 should be pending because op1 hasn't arrived");
    assert!(!group_store::check_group_membership(&store, &gid, &member2).unwrap());

    // Apply op1 — should apply immediately AND cascade to apply op2
    let applied = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied, "op1 should apply immediately");

    // Both members should now be present
    assert!(group_store::check_group_membership(&store, &gid, &member1).unwrap());
    assert!(group_store::check_group_membership(&store, &gid, &member2).unwrap());

    // DAG should have 1 head (op2, since it's the tip)
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 1);
    assert!(heads.contains(&op2.content_hash().unwrap()));
}

#[tokio::test]
async fn dag_concurrent_ops_create_two_heads() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member1 = PrivateKey::random(&mut rng).public_key();
    let member2 = PrivateKey::random(&mut rng).public_key();

    // Two concurrent ops with genesis as parent
    let op_a = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        GroupOp::MemberAdded {
            member: member1,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    let op_b = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![[0u8; 32]],
        2,
        GroupOp::MemberAdded {
            member: member2,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();

    let applier = GroupGovernanceApplier::new(store.clone());
    let mut dag = DagStore::new([0u8; 32]);

    dag.add_delta(signed_op_to_delta(&op_a).unwrap(), &applier)
        .await
        .unwrap();
    dag.add_delta(signed_op_to_delta(&op_b).unwrap(), &applier)
        .await
        .unwrap();

    assert!(group_store::check_group_membership(&store, &gid, &member1).unwrap());
    assert!(group_store::check_group_membership(&store, &gid, &member2).unwrap());

    // Two heads (concurrent branches)
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 2);

    // Merge op referencing both heads
    let hash_a = op_a.content_hash().unwrap();
    let hash_b = op_b.content_hash().unwrap();
    let merge_op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![hash_a, hash_b],
        3,
        GroupOp::Noop,
    )
    .unwrap();
    dag.add_delta(signed_op_to_delta(&merge_op).unwrap(), &applier)
        .await
        .unwrap();

    // After merge: single head
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 1);
    assert!(heads.contains(&merge_op.content_hash().unwrap()));
}

#[tokio::test]
async fn dag_duplicate_delta_is_idempotent() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let op =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![[0u8; 32]], 1, GroupOp::Noop).unwrap();
    let delta = signed_op_to_delta(&op).unwrap();

    let applier = GroupGovernanceApplier::new(store.clone());
    let mut dag = DagStore::new([0u8; 32]);

    let first = dag.add_delta(delta.clone(), &applier).await.unwrap();
    assert!(first);
    let second = dag.add_delta(delta, &applier).await.unwrap();
    assert!(!second, "duplicate delta should return false");
}

#[tokio::test]
async fn dag_deep_chain_with_out_of_order_delivery() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    save_group_meta(&store, &gid, &sample_meta(admin_pk)).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    // Build chain: op1 → op2 → op3 → op4 → op5
    let mut ops = Vec::new();
    let mut prev_hash: Vec<[u8; 32]> = vec![[0u8; 32]];
    for i in 1..=5u64 {
        let op =
            SignedGroupOp::sign(&admin_sk, gid_bytes, prev_hash.clone(), i, GroupOp::Noop).unwrap();
        prev_hash = vec![op.content_hash().unwrap()];
        ops.push(op);
    }

    let applier = GroupGovernanceApplier::new(store.clone());
    let mut dag = DagStore::new([0u8; 32]);

    // Deliver in reverse order: op5, op4, op3, op2
    for op in ops[1..].iter().rev() {
        let result = dag
            .add_delta(signed_op_to_delta(op).unwrap(), &applier)
            .await
            .unwrap();
        assert!(!result, "should be pending");
    }

    // Deliver op1 — should cascade all
    let result = dag
        .add_delta(signed_op_to_delta(&ops[0]).unwrap(), &applier)
        .await
        .unwrap();
    assert!(result, "op1 should apply and cascade");

    // All 5 ops applied, single head (op5)
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 1);
    assert!(heads.contains(&ops[4].content_hash().unwrap()));

    // Nonce should be at 5 for the admin
    assert_eq!(
        group_store::get_local_gov_nonce(&store, &gid, &admin_pk)
            .unwrap()
            .unwrap(),
        5
    );
}
