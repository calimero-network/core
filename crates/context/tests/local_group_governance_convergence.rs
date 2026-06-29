//! Two logical nodes (separate stores) receive the same gossip payloads and converge to identical group membership.
//!
//! Each peer applies `borsh(SignedGroupOp)` payloads via `group_store::apply_local_signed_group_op`
//! (same path as `ContextClient::apply_signed_group_op`).
//! Real libp2p gossip on `group/<hex>` is covered by `calimero-network` (`tests/gossipsub_group_topic.rs`).

use std::sync::Arc;

use borsh::to_vec as borsh_to_vec;
use calimero_context::governance_dag::{signed_op_to_delta, GroupGovernanceApplier};
use calimero_context::group_store::{
    self, apply_local_signed_group_op, get_op_head, read_op_log_after, MembershipRepository,
    MetaRepository,
};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_dag::DagStore;
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

fn sign_invitation(
    admin_sk: &PrivateKey,
    group_id: ContextGroupId,
    expiration_timestamp: u64,
    invited_role: u8,
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id,
        expiration_timestamp,
        secret_salt: [0x42; 32],
        invited_role,
    };
    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_sig = admin_sk
        .sign(&Sha256::digest(&inv_bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

/// `MemberRemoved` with placeholder cross-DAG claims for tests that
/// only exercise convergence on the membership-row mutation. The
/// hashes intentionally don't match real post-apply state — these
/// tests don't verify the mismatch-detection path (that's covered
/// separately by `compute_group_state_hash_after_remove` unit tests).
fn dummy_member_removed(member: PublicKey) -> GroupOp {
    GroupOp::MemberRemoved {
        member,
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    }
}

fn sample_meta(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

fn sorted_members(store: &Store, gid: &ContextGroupId) -> Vec<(PublicKey, GroupMemberRole)> {
    let mut v = MembershipRepository::new(store)
        .list(gid, 0, usize::MAX)
        .expect("list_group_members");
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
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
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
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_a)
            .is_member(&gid, &new_member)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_b)
            .is_member(&gid, &new_member)
            .unwrap()
    );

    let op2 =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], 2, GroupOp::Noop).expect("sign op2");
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
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
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

    let meta_a = MetaRepository::new(&store_a)
        .load(&gid)
        .unwrap()
        .expect("meta a");
    let meta_b = MetaRepository::new(&store_b)
        .load(&gid)
        .unwrap()
        .expect("meta b");

    assert_eq!(meta_a.target_application_id, new_target);
    assert_eq!(meta_a.app_key, [0x11; 32]);
    assert_eq!(meta_a.migration, Some(b"v1-migration".to_vec()));
    assert_eq!(meta_a.target_application_id, meta_b.target_application_id);
    assert_eq!(meta_a.app_key, meta_b.app_key);
    assert_eq!(meta_a.migration, meta_b.migration);
}

#[test]
fn two_nodes_converge_on_namespace_member_joined() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();

    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    for store in [&store_a, &store_b] {
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_pk.digest()),
        group_id: gid,
        expiration_timestamp: 0,
        secret_salt: [0x42; 32],
        invited_role: 1,
    };

    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_hash = Sha256::digest(&inv_bytes);
    let inv_sig = admin_sk.sign(&inv_hash).expect("sign invitation");
    let signed_invitation = SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    };

    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner_pk,
            signed_invitation,
        }),
    )
    .expect("sign MemberJoined");

    group_store::apply_signed_namespace_op(&store_a, &ns_op).unwrap();
    group_store::apply_signed_namespace_op(&store_b, &ns_op).unwrap();

    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_a)
            .is_member(&gid, &joiner_pk)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_b)
            .is_member(&gid, &joiner_pk)
            .unwrap()
    );
}

#[test]
fn member_joined_at_rejects_expired_invitation() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let signed_invitation = sign_invitation(&admin_sk, gid, 1_000_000, 1);

    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: joiner_pk,
            signed_invitation,
            joined_at: 2_000_000,
        }),
    )
    .expect("sign MemberJoinedAt");

    let err = group_store::apply_signed_namespace_op(&store, &ns_op)
        .expect_err("expired MemberJoinedAt must be rejected on apply");
    assert!(
        format!("{err:#}").contains("expired"),
        "rejection must come from the expiry gate, not another check: {err:#}"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&gid, &joiner_pk)
            .unwrap(),
        "joiner with an expired invitation must not be recorded as a member"
    );
}

#[test]
fn member_joined_rejects_when_expiration_set_and_joined_at_absent() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // MemberJoined (legacy, no joined_at) with a non-zero expiration is a
    // malformed op — the caller should have used MemberJoinedAt.
    let signed_invitation = sign_invitation(&admin_sk, gid, 9_999_999_999, 1);
    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner_pk,
            signed_invitation,
        }),
    )
    .expect("sign MemberJoined");

    let err = group_store::apply_signed_namespace_op(&store, &ns_op)
        .expect_err("MemberJoined with non-zero expiration must be rejected");
    assert!(
        format!("{err:#}").contains("joined_at is absent"),
        "rejection must come from the absent-joined_at gate: {err:#}"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&gid, &joiner_pk)
            .unwrap(),
        "joiner must not be recorded as a member"
    );
}

#[test]
fn member_joined_at_accepts_in_window_invitation() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    for store in [&store_a, &store_b] {
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    // Boundary: joined_at exactly equals expiry. The gate is
    // `joined_at > expiration`, so the boundary must be accepted (not `>=`).
    let signed_invitation = sign_invitation(&admin_sk, gid, 1_000_000, 1);
    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: joiner_pk,
            signed_invitation,
            joined_at: 1_000_000,
        }),
    )
    .expect("sign MemberJoinedAt");

    group_store::apply_signed_namespace_op(&store_a, &ns_op).unwrap();
    group_store::apply_signed_namespace_op(&store_b, &ns_op).unwrap();

    for store in [&store_a, &store_b] {
        assert!(MembershipRepository::new(store)
            .is_member(&gid, &joiner_pk)
            .unwrap());
    }
}

#[test]
fn member_joined_at_backdated_joined_at_bypasses_apply_gate_documented_residual() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    // Pins the accepted residual: `joined_at` is self-attested by the joiner
    // (signature-covered but not corroborated), so a custom client can backdate
    // it to fold a membership row past expiry. The authoritative backstop is
    // the responder key-delivery gate (`validate_open_invitation`, exercised in
    // governance-store tests), which uses the responder's own clock. Fully
    // closing this would need an admin co-signature at redemption (out of scope).
    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // Invitation expired at t=1_000_000, but the joiner backdates joined_at to 0.
    let signed_invitation = sign_invitation(&admin_sk, gid, 1_000_000, 1);
    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: joiner_pk,
            signed_invitation,
            joined_at: 0,
        }),
    )
    .expect("sign MemberJoinedAt");

    group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    assert!(
        MembershipRepository::new(&store)
            .is_member(&gid, &joiner_pk)
            .unwrap(),
        "apply gate accepts a backdated joined_at (residual); responder key gate is the backstop"
    );
}

#[test]
fn member_joined_at_in_window_converges_when_expiration_already_past_wallclock() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store_a = empty_store();
    let store_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    for store in [&store_a, &store_b] {
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    // Expiry is in 1970, far before any real wall clock, but the joiner's
    // claimed `joined_at` is still within the window. A local-clock check
    // would reject this on every node; the deterministic gate accepts it,
    // so two independently-applying nodes converge instead of split-brain.
    let signed_invitation = sign_invitation(&admin_sk, gid, 1_000_000, 1);
    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: joiner_pk,
            signed_invitation,
            joined_at: 999_999,
        }),
    )
    .expect("sign MemberJoinedAt");

    group_store::apply_signed_namespace_op(&store_a, &ns_op).unwrap();
    group_store::apply_signed_namespace_op(&store_b, &ns_op).unwrap();

    for store in [&store_a, &store_b] {
        assert!(MembershipRepository::new(store)
            .is_member(&gid, &joiner_pk)
            .unwrap());
    }
}

#[test]
fn member_joined_at_ignores_zero_expiration() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let signed_invitation = sign_invitation(&admin_sk, gid, 0, 1);
    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: joiner_pk,
            signed_invitation,
            joined_at: u64::MAX,
        }),
    )
    .expect("sign MemberJoinedAt");

    group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &joiner_pk)
        .unwrap());
}

#[test]
fn recursive_invite_joins_all_descendant_groups() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let mut rng = OsRng;
    let ns_id = sample_group_id();
    let child_a = ContextGroupId::from([0xAA; 32]);
    let child_b = ContextGroupId::from([0xBB; 32]);
    let grandchild = ContextGroupId::from([0xCC; 32]);

    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    // Setup: create namespace root + child groups, add admin to all
    for gid in [&ns_id, &child_a, &child_b, &grandchild] {
        MetaRepository::new(&store)
            .save(gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    // Setup nesting: ns_id → child_a → grandchild, ns_id → child_b
    calimero_context::group_store::NamespaceRepository::new(&store)
        .nest(&ns_id, &child_a)
        .unwrap();
    calimero_context::group_store::NamespaceRepository::new(&store)
        .nest(&ns_id, &child_b)
        .unwrap();
    calimero_context::group_store::NamespaceRepository::new(&store)
        .nest(&child_a, &grandchild)
        .unwrap();

    // Verify tree structure
    let children = calimero_context::group_store::NamespaceRepository::new(&store)
        .list_children(&ns_id)
        .unwrap();
    assert_eq!(children.len(), 2);
    let descendants = calimero_context::group_store::NamespaceRepository::new(&store)
        .collect_descendants(&ns_id)
        .unwrap();
    assert_eq!(descendants.len(), 3); // child_a, child_b, grandchild

    // Recursive invite for ns_id (covers all 4 groups including ns_id itself)
    let invitations = calimero_context::group_store::NamespaceRepository::new(&store)
        .create_recursive_invitations(&ns_id, &admin_sk, 365 * 24 * 3600, 1)
        .unwrap();

    assert_eq!(invitations.len(), 4); // ns_id + child_a + child_b + grandchild

    // Joiner publishes MemberJoinedAt for each invitation (expiration is set,
    // so joined_at must be provided; use 1 which is safely before any future expiry).
    for (i, (_gid, signed_inv)) in invitations.iter().enumerate() {
        let ns_op = SignedNamespaceOp::sign(
            &joiner_sk,
            ns_id.to_bytes(),
            vec![],
            (i + 1) as u64,
            NamespaceOp::Root(RootOp::MemberJoinedAt {
                member: joiner_pk,
                signed_invitation: signed_inv.clone(),
                joined_at: 1,
            }),
        )
        .expect("sign MemberJoinedAt");

        group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    }

    // Verify joiner is member of ALL groups
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&ns_id, &joiner_pk)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&child_a, &joiner_pk)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&child_b, &joiner_pk)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&grandchild, &joiner_pk)
            .unwrap()
    );

    // Recursive remove from ns_id (should remove from all 4)
    let removed = calimero_context::group_store::NamespaceRepository::new(&store)
        .recursive_remove_member(&ns_id, &joiner_pk)
        .unwrap();
    assert_eq!(removed.len(), 4);

    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&ns_id, &joiner_pk)
            .unwrap()
    );
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&child_a, &joiner_pk)
            .unwrap()
    );
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&child_b, &joiner_pk)
            .unwrap()
    );
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&grandchild, &joiner_pk)
            .unwrap()
    );
}

#[test]
fn nest_group_rejects_cycles() {
    let store = empty_store();

    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let group_a = ContextGroupId::from([0xA0; 32]);
    let group_b = ContextGroupId::from([0xB0; 32]);
    let group_c = ContextGroupId::from([0xC0; 32]);

    for gid in [&group_a, &group_b, &group_c] {
        MetaRepository::new(&store)
            .save(gid, &sample_meta(admin_pk))
            .unwrap();
    }

    // A → B → C
    calimero_context::group_store::NamespaceRepository::new(&store)
        .nest(&group_a, &group_b)
        .unwrap();
    calimero_context::group_store::NamespaceRepository::new(&store)
        .nest(&group_b, &group_c)
        .unwrap();

    // C → A would create A → B → C → A cycle
    let result =
        calimero_context::group_store::NamespaceRepository::new(&store).nest(&group_c, &group_a);
    assert!(result.is_err(), "should reject cycle");
    assert!(
        result.unwrap_err().to_string().contains("cycle"),
        "error should mention cycle"
    );

    // Self-nesting
    let result =
        calimero_context::group_store::NamespaceRepository::new(&store).nest(&group_a, &group_a);
    assert!(result.is_err(), "should reject self-nesting");

    // B already has a parent (A), can't give it a second
    let result =
        calimero_context::group_store::NamespaceRepository::new(&store).nest(&group_c, &group_b);
    assert!(result.is_err(), "should reject double-parent");
}

#[test]
fn two_nodes_converge_on_context_alias_as_admin() {
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
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &creator_pk, GroupMemberRole::Member)
            .unwrap();
        calimero_context::group_store::CapabilitiesRepository::new(store)
            .set_member_capability(&gid, &creator_pk, MemberCapabilities::CAN_CREATE_CONTEXT)
            .unwrap();
    }

    let op1 = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::ContextRegistered {
            context_id,
            application_id: calimero_primitives::application::ApplicationId::from([0xAA; 32]),
            blob_id: calimero_primitives::blobs::BlobId::from([0xBB; 32]),
            source: String::new(),
            service_name: None,
        },
    )
    .expect("sign ContextRegistered");
    let op2 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        1,
        GroupOp::ContextMetadataSet {
            context_id,
            name: Some("wire-alias".to_owned()),
            data: Default::default(),
        },
    )
    .expect("sign ContextMetadataSet");

    for payload in [
        borsh_to_vec(&op1).expect("encode op1"),
        borsh_to_vec(&op2).expect("encode op2"),
    ] {
        apply_wire_payload(&store_a, &payload);
        apply_wire_payload(&store_b, &payload);
    }

    assert_eq!(
        calimero_context::group_store::MetadataRepository::new(&store_a)
            .context_metadata(&gid, &context_id)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("wire-alias")
    );
    assert_eq!(
        calimero_context::group_store::MetadataRepository::new(&store_b)
            .context_metadata(&gid, &context_id)
            .unwrap()
            .and_then(|r| r.name)
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
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

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

    let op2 =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], 2, GroupOp::Noop).expect("sign op2");
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
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

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
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    let member1 = PrivateKey::random(&mut rng).public_key();
    let member2 = PrivateKey::random(&mut rng).public_key();

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

    for op in [&op1, &op2] {
        apply_local_signed_group_op(&store_online, op).unwrap();
    }

    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_online)
            .is_member(&gid, &member1)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_online)
            .is_member(&gid, &member2)
            .unwrap()
    );
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store_offline)
            .is_member(&gid, &member1)
            .unwrap()
    );

    let missed_ops = read_op_log_after(&store_online, &gid, 0, 100).unwrap();
    assert_eq!(missed_ops.len(), 2);

    for (_seq, op_bytes) in &missed_ops {
        let op: SignedGroupOp = borsh::from_slice(op_bytes).unwrap();
        apply_local_signed_group_op(&store_offline, &op).unwrap();
    }

    assert_same_group_view(&store_online, &store_offline, &gid);
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_offline)
            .is_member(&gid, &member1)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store_offline)
            .is_member(&gid, &member2)
            .unwrap()
    );
}

#[tokio::test]
async fn dag_applies_ops_in_causal_order() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

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
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member2)
            .unwrap()
    );

    // Apply op1 — should apply immediately AND cascade to apply op2
    let applied = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied, "op1 should apply immediately");

    // Both members should now be present
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member1)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member2)
            .unwrap()
    );

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
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

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

    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member1)
            .unwrap()
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member2)
            .unwrap()
    );

    // Two heads (concurrent branches)
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 2);

    // Merge op referencing both heads
    let hash_a = op_a.content_hash().unwrap();
    let hash_b = op_b.content_hash().unwrap();
    let merge_op =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![hash_a, hash_b], 3, GroupOp::Noop).unwrap();
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
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let op = SignedGroupOp::sign(&admin_sk, gid_bytes, vec![[0u8; 32]], 1, GroupOp::Noop).unwrap();
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
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

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

#[test]
fn rejects_op_with_too_many_parents() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // 256 parents should be accepted
    let parents_256: Vec<[u8; 32]> = (0..256)
        .map(|i| {
            let mut h = [0u8; 32];
            h[0] = (i & 0xFF) as u8;
            h[1] = ((i >> 8) & 0xFF) as u8;
            h
        })
        .collect();
    let op_ok = SignedGroupOp::sign(&admin_sk, gid_bytes, parents_256, 1, GroupOp::Noop).unwrap();
    assert!(apply_local_signed_group_op(&store, &op_ok).is_ok());

    // 257 parents should be rejected
    let parents_257: Vec<[u8; 32]> = (0..257)
        .map(|i| {
            let mut h = [0u8; 32];
            h[0] = (i & 0xFF) as u8;
            h[1] = ((i >> 8) & 0xFF) as u8;
            h
        })
        .collect();
    let op_bad = SignedGroupOp::sign(&admin_sk, gid_bytes, parents_257, 2, GroupOp::Noop).unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
}

#[test]
fn dag_heads_are_capped_at_max() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // Create 70 concurrent ops (all with genesis parent) to exceed MAX_DAG_HEADS (64)
    for i in 1..=70u64 {
        let op =
            SignedGroupOp::sign(&admin_sk, gid_bytes, vec![[0u8; 32]], i, GroupOp::Noop).unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();
    }

    let head = get_op_head(&store, &gid).unwrap().expect("head");
    assert!(
        head.dag_heads.len() <= 64,
        "dag_heads should be capped at 64, got {}",
        head.dag_heads.len()
    );
    // The last op's hash should be present (not truncated)
    let last_op =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![[0u8; 32]], 70, GroupOp::Noop).unwrap();
    let last_hash = last_op.content_hash().unwrap();
    assert!(
        head.dag_heads.contains(&last_hash),
        "most recent op should still be in dag_heads"
    );
}

#[test]
fn concurrent_independent_member_adds_converge() {
    // Two admins concurrently apply INDEPENDENT ops (each adds a different member)
    // against the same genesis state. Both nodes must converge to the same member
    // set regardless of apply order — the CRDT convergence property the cutover
    // relies on (no op-level staleness gate rejects a legitimate concurrent
    // sibling). Signature + the per-signer nonce window are the only apply gates.
    //
    // Independent ADDS (not an admin-removes-admin scenario) deliberately avoid an
    // authorization-order confound: under the live-fallback authorizer a removed
    // admin's later op would fail on *authorization*, masking the convergence
    // property this test pins down.
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let node_b = empty_store();
    let node_c = empty_store();

    let admin_a_sk = PrivateKey::random(&mut rng);
    let admin_a_pk = admin_a_sk.public_key();
    let admin_c_sk = PrivateKey::random(&mut rng);
    let admin_c_pk = admin_c_sk.public_key();
    let new_member_d = PrivateKey::random(&mut rng).public_key();
    let new_member_e = PrivateKey::random(&mut rng).public_key();

    for store in [&node_b, &node_c] {
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_a_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_a_pk, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_c_pk, GroupMemberRole::Admin)
            .unwrap();
    }

    let meta_hash_b = MetaRepository::new(&node_b)
        .compute_state_hash(&gid)
        .unwrap();
    let meta_hash_c = MetaRepository::new(&node_c)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        meta_hash_b, meta_hash_c,
        "nodes start with identical group-meta hash"
    );

    // A adds D; C adds E — independent, non-conflicting, both authored against the
    // SAME genesis state (concurrent).
    let op_a = SignedGroupOp::sign(
        &admin_a_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        GroupOp::MemberAdded {
            member: new_member_d,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    // nonce is per-signer: admin_c's nonce=1 does not collide with admin_a's
    // nonce=1 — each signer has its own independent nonce window.
    let op_c = SignedGroupOp::sign(
        &admin_c_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        GroupOp::MemberAdded {
            member: new_member_e,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();

    // Node B applies op_a then op_c; node C applies them in the opposite order.
    assert!(
        apply_local_signed_group_op(&node_b, &op_a).is_ok(),
        "op_a applies on node_b"
    );
    assert!(
        apply_local_signed_group_op(&node_b, &op_c).is_ok(),
        "concurrent op_c also applies on node_b (no staleness gate)"
    );

    assert!(
        apply_local_signed_group_op(&node_c, &op_c).is_ok(),
        "op_c applies on node_c"
    );
    assert!(
        apply_local_signed_group_op(&node_c, &op_a).is_ok(),
        "concurrent op_a also applies on node_c"
    );

    // Both nodes converge to {A, C admins + D, E members}, regardless of order.
    for (label, store) in [("node_b", &node_b), ("node_c", &node_c)] {
        let m = calimero_context::group_store::MembershipRepository::new(store);
        assert!(
            m.is_member(&gid, &admin_a_pk).unwrap(),
            "{label}: A present"
        );
        assert!(
            m.is_member(&gid, &admin_c_pk).unwrap(),
            "{label}: C present"
        );
        assert!(
            m.is_member(&gid, &new_member_d).unwrap(),
            "{label}: D added"
        );
        assert!(
            m.is_member(&gid, &new_member_e).unwrap(),
            "{label}: E added"
        );
    }

    // `compute_state_hash` sorts members by pubkey, so the converged hashes match
    // — order-independent convergence.
    let final_b = MetaRepository::new(&node_b)
        .compute_state_hash(&gid)
        .unwrap();
    let final_c = MetaRepository::new(&node_c)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        final_b, final_c,
        "nodes converge to identical state regardless of apply order"
    );

    // The per-signer NONCE WINDOW is the anti-replay guard. Re-applying op_a
    // (admin_a, nonce 1) is a no-op — the nonce is already in the window — so a DAG
    // replay of a seen op cannot double-apply its mutation.
    let members_before_replay = MembershipRepository::new(&node_b)
        .list(&gid, 0, usize::MAX)
        .unwrap()
        .len();
    assert!(
        apply_local_signed_group_op(&node_b, &op_a).is_ok(),
        "replaying op_a is accepted (deduped, not errored)"
    );
    let after_replay_b = MetaRepository::new(&node_b)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        final_b, after_replay_b,
        "replaying a seen op is a no-op — nonce window dedups it, state is unchanged"
    );
    // Pin the actual member set, not just its hash: the dedup must return before
    // `apply_group_op_mutations`, so no duplicate member row escapes (a future
    // regression where the dedup fires but a side-effect still mutates would slip
    // past a hash-only check if the side-effect were hash-neutral).
    let members_after_replay = MembershipRepository::new(&node_b)
        .list(&gid, 0, usize::MAX)
        .unwrap()
        .len();
    assert_eq!(
        members_before_replay, members_after_replay,
        "replay must not add a duplicate member row"
    );
}

#[test]
fn cascade_removal_on_member_kick() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();

    let context_id = ContextId::from([0xCC; 32]);

    group_store::register_context_in_group(&store, &gid, &context_id).unwrap();

    // Write a ContextIdentity entry for the member (simulating context access).
    {
        use calimero_store::key::ContextIdentity;
        let mut handle = store.handle();
        let key = ContextIdentity::new(context_id, member_pk);
        handle
            .put(
                &key,
                &calimero_store::types::ContextIdentity {
                    private_key: None,
                    sender_key: None,
                },
            )
            .unwrap();
    }

    {
        use calimero_store::key::ContextIdentity;
        let handle = store.handle();
        let key = ContextIdentity::new(context_id, member_pk);
        assert!(
            handle.has(&key).unwrap(),
            "member should be in context before kick"
        );
    }

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        dummy_member_removed(member_pk),
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();

    assert!(
        !calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member_pk)
            .unwrap()
    );

    {
        use calimero_store::key::ContextIdentity;
        let handle = store.handle();
        let key = ContextIdentity::new(context_id, member_pk);
        assert!(
            !handle.has(&key).unwrap(),
            "member should be cascade-removed from context"
        );
    }
}

#[test]
fn cascade_removal_deterministic_across_nodes() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let gid_bytes = gid.to_bytes();

    let node_a = empty_store();
    let node_b = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let member_pk = PrivateKey::random(&mut rng).public_key();

    let ctx1 = ContextId::from([0xC1; 32]);
    let ctx2 = ContextId::from([0xC2; 32]);

    for store in [&node_a, &node_b] {
        MetaRepository::new(store)
            .save(&gid, &sample_meta(admin_pk))
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(store)
            .add_member(&gid, &member_pk, GroupMemberRole::Member)
            .unwrap();
        group_store::register_context_in_group(store, &gid, &ctx1).unwrap();
        group_store::register_context_in_group(store, &gid, &ctx2).unwrap();

        use calimero_store::key::ContextIdentity;
        let mut handle = store.handle();
        for ctx in [ctx1, ctx2] {
            let key = ContextIdentity::new(ctx, member_pk);
            handle
                .put(
                    &key,
                    &calimero_store::types::ContextIdentity {
                        private_key: None,
                        sender_key: None,
                    },
                )
                .unwrap();
        }
    }

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![[0u8; 32]],
        1,
        dummy_member_removed(member_pk),
    )
    .unwrap();
    apply_local_signed_group_op(&node_a, &op).unwrap();
    apply_local_signed_group_op(&node_b, &op).unwrap();

    for (label, store) in [("node_a", &node_a), ("node_b", &node_b)] {
        use calimero_store::key::ContextIdentity;
        let handle = store.handle();
        for ctx in [ctx1, ctx2] {
            let key = ContextIdentity::new(ctx, member_pk);
            assert!(
                !handle.has(&key).unwrap(),
                "{label}: member should be cascade-removed from context {}",
                hex::encode(ctx.as_ref())
            );
        }
        assert!(
            !calimero_context::group_store::MembershipRepository::new(store)
                .is_member(&gid, &member_pk)
                .unwrap(),
            "{label}: member should be removed from group"
        );
    }
}

// member_joined_context_op_propagates test removed:
// MemberJoinedContext governance op was removed — context membership
// is now derived from group membership + visibility.

#[test]
fn group_member_with_keys_persists_and_retrieves() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let sender_sk = PrivateKey::random(&mut rng);

    calimero_context::group_store::MembershipRepository::new(&store)
        .add_member_with_keys(
            &gid,
            &member_pk,
            GroupMemberRole::Member,
            Some(*member_sk),
            Some(*sender_sk),
        )
        .unwrap();

    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &member_pk)
            .unwrap()
    );

    let value = calimero_context::group_store::MembershipRepository::new(&store)
        .member_value(&gid, &member_pk)
        .unwrap()
        .expect("member value should exist");

    assert_eq!(value.role, GroupMemberRole::Member);
    assert_eq!(value.private_key, Some(*member_sk));
    assert_eq!(value.sender_key, Some(*sender_sk));
}

#[test]
fn group_member_without_keys_has_none_keys() {
    let mut rng = OsRng;
    let gid = sample_group_id();
    let store = empty_store();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();

    let remote_pk = PrivateKey::random(&mut rng).public_key();
    calimero_context::group_store::MembershipRepository::new(&store)
        .add_member(&gid, &remote_pk, GroupMemberRole::Member)
        .unwrap();

    let value = calimero_context::group_store::MembershipRepository::new(&store)
        .member_value(&gid, &remote_pk)
        .unwrap()
        .expect("member value should exist");

    assert_eq!(value.role, GroupMemberRole::Member);
    assert_eq!(value.private_key, None);
    assert_eq!(value.sender_key, None);
}

/// Regression for #2327: a node that applies a namespace governance op it
/// already has (e.g. its own published op coming back via sync backfill —
/// the `group_store` apply path doesn't dedup against the actor's in-memory
/// `DagStore`) must NOT accumulate a duplicate in its namespace DAG head
/// set. A duplicated head set makes `GovernanceParentEdge::new` fail, so the
/// node ships state deltas with an empty governance position and every peer
/// rejects all of its writes ("author is not a member of the group at
/// governance cut").
#[test]
fn reapplying_namespace_op_keeps_dag_head_set_clean_and_position_embeddable() {
    use calimero_context::group_store::NamespaceDagService;
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::types::GovernanceParentEdge;

    let mut rng = OsRng;
    let gid = sample_group_id();
    let ns_id = gid.to_bytes();

    let store = empty_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner_pk = joiner_sk.public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // A real MemberJoined op (signer = joiner, with an admin-signed invitation),
    // matching the `two_nodes_converge_on_namespace_member_joined` setup.
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_pk.digest()),
        group_id: gid,
        expiration_timestamp: 0,
        secret_salt: [0x42; 32],
        invited_role: 1,
    };
    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_hash = Sha256::digest(&inv_bytes);
    let inv_sig = admin_sk.sign(&inv_hash).expect("sign invitation");
    let signed_invitation = SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    };

    let ns_op = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_id,
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: joiner_pk,
            signed_invitation,
        }),
    )
    .expect("sign MemberJoined");
    let op_hash = ns_op.content_hash().expect("content_hash");

    // The bug is in one store's head-set bookkeeping, so a single store that
    // re-applies its own op is the full repro (a two-node variant — A publishes,
    // B receives via gossip, A receives back via sync — adds nothing here).
    // Check after *every* apply so a defect that only surfaces on the Nth
    // replay (e.g. a counter-based dedup that tolerates one duplicate) is
    // still caught, not just the final state.
    let read_state = |label: &str| {
        let (heads, _next_nonce) = NamespaceDagService::new(&store, ns_id)
            .read_head()
            .expect("read namespace dag head");
        assert_eq!(
            heads,
            vec![op_hash],
            "namespace DAG head set must stay duplicate-free ({label})"
        );
        // A node at this cut can embed a non-empty GovernanceParentEdge — i.e.
        // `governance_dag_heads_len == 1`, so peers accept its state deltas.
        let edge = GovernanceParentEdge::new(heads)
            .unwrap_or_else(|e| panic!("GovernanceParentEdge must be embeddable ({label}): {e}"));
        assert_eq!(edge.governance_dag_heads, vec![op_hash]);
    };

    // 1) "Publish locally": apply the op once.
    group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    read_state("after publish");
    // 2) "Re-receive via sync backfill": the same op arrives again, twice.
    group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    read_state("after 1st re-receive");
    group_store::apply_signed_namespace_op(&store, &ns_op).unwrap();
    read_state("after 2nd re-receive");

    assert!(
        calimero_context::group_store::MembershipRepository::new(&store)
            .is_member(&gid, &joiner_pk)
            .unwrap()
    );
}
