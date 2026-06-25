//! Tests for `group_store::namespace::*`. Extracted from the monolithic
//! `group_store/tests.rs` as part of issue #2480 (epic #2300).
//!
//! Helpers shared with non-namespace tests (`test_store`, `test_group_id`,
//! `test_meta`, `dummy_member_removed_op`, `nest_for_test`,
//! `sample_meta_with_admin`) are imported from the parent
//! `group_store::test_fixtures` module. Namespace-only inline helpers
//! (`raw_namespace_dag_heads`) came along with the move.

use crate::{
    CapabilitiesRepository, GroupDeletedRejection, GroupKeyring, MembershipRepository,
    MetaRepository, NamespaceRepository,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;

use super::super::test_fixtures::{
    nest_for_test, sample_meta_with_admin, test_group_id, test_meta, test_store,
};
use super::super::*;

#[test]
fn namespace_dag_service_store_operation_rejects_namespace_mismatch() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let governance_ns = [0x71; 32];
    let op_ns = [0x72; 32];
    let signer_sk = PrivateKey::random(&mut rng);

    let signed = SignedNamespaceOp::sign(
        &signer_sk,
        op_ns,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![1, 2, 3],
        }),
    )
    .unwrap();

    let err = NamespaceDagService::new(&store, governance_ns)
        .store_operation(&signed)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("namespace mismatch when storing op"),
        "unexpected error: {err}"
    );
}

fn test_signed_invitation(
    inviter_sk: &PrivateKey,
    group_id: ContextGroupId,
    expiration_timestamp: u64,
) -> calimero_context_config::types::SignedGroupOpenInvitation {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use sha2::{Digest, Sha256};

    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*inviter_sk.public_key().digest()),
        group_id,
        expiration_timestamp,
        secret_salt: [0x42; 32],
        invited_role: 1,
    };
    let inv_bytes = borsh::to_vec(&invitation).unwrap();
    let inv_sig = inviter_sk.sign(&Sha256::digest(&inv_bytes)).unwrap();
    SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

#[test]
fn validate_open_invitation_rejects_expired() {
    let mut rng = rand::rngs::OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let gid = test_group_id();
    let ns_id = gid.to_bytes();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let signed = test_signed_invitation(&admin_sk, gid, 1_000_000);
    let svc = NamespaceMembershipService::new(&store, ns_id);

    assert!(
        svc.validate_open_invitation(&signed, 2_000_000).is_err(),
        "invitation past expiry must be rejected by the responder gate"
    );
    assert!(
        svc.validate_open_invitation(&signed, 999_999).is_ok(),
        "in-window invitation must be accepted"
    );
    // Boundary: the gate is `now > expiration`, so now exactly at expiry is
    // accepted and one second past is rejected.
    assert!(
        svc.validate_open_invitation(&signed, 1_000_000).is_ok(),
        "now == expiry must be accepted (gate is `>`, not `>=`)"
    );
    assert!(
        svc.validate_open_invitation(&signed, 1_000_001).is_err(),
        "one second past expiry must be rejected"
    );
}

#[test]
fn validate_open_invitation_rejects_forged_inviter_signature() {
    let mut rng = rand::rngs::OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let gid = test_group_id();
    let ns_id = gid.to_bytes();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let mut signed = test_signed_invitation(&admin_sk, gid, 9_999_999_999);
    signed.inviter_signature = hex::encode([0u8; 64]);
    let svc = NamespaceMembershipService::new(&store, ns_id);

    assert!(
        svc.validate_open_invitation(&signed, 1_000).is_err(),
        "forged inviter signature must be rejected"
    );
}

#[test]
fn validate_open_invitation_rejects_unauthorized_inviter() {
    let mut rng = rand::rngs::OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let stranger_sk = PrivateKey::random(&mut rng);
    let gid = test_group_id();
    let ns_id = gid.to_bytes();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let signed = test_signed_invitation(&stranger_sk, gid, 9_999_999_999);
    let svc = NamespaceMembershipService::new(&store, ns_id);

    assert!(
        svc.validate_open_invitation(&signed, 1_000).is_err(),
        "invitation whose inviter lacks invite permission must be rejected"
    );
}

#[test]
fn namespace_dag_service_advance_dag_head_prunes_parent_hashes() {
    let store = test_store();
    let namespace_id = [0x73; 32];
    let dag = NamespaceDagService::new(&store, namespace_id);

    let delta_a = [0xA1; 32];
    let delta_b = [0xB2; 32];
    let delta_c = [0xC3; 32];

    dag.advance_dag_head(delta_a, &[], 1).unwrap();
    dag.advance_dag_head(delta_b, &[], 2).unwrap();
    dag.advance_dag_head(delta_c, &[delta_a], 3).unwrap();

    let head = dag.read_head_record().unwrap();
    assert_eq!(head.parent_hashes, vec![delta_b, delta_c]);
    assert_eq!(head.next_nonce, 4);
}

#[test]
fn namespace_dag_service_advance_dag_head_is_idempotent_for_same_delta() {
    let store = test_store();
    let namespace_id = [0x77; 32];
    let dag = NamespaceDagService::new(&store, namespace_id);

    let delta_a = [0xA1; 32];
    let delta_b = [0xB2; 32];

    dag.advance_dag_head(delta_a, &[], 1).unwrap();
    dag.advance_dag_head(delta_b, &[], 2).unwrap();
    // Same op replayed — parents are unchanged, so it doesn't supersede any
    // existing head; it must still not duplicate itself.
    dag.advance_dag_head(delta_b, &[], 2).unwrap();
    dag.advance_dag_head(delta_b, &[], 2).unwrap();

    let raw = raw_namespace_dag_heads(&store, namespace_id);
    assert_eq!(
        raw,
        vec![delta_a, delta_b],
        "head set must stay duplicate-free"
    );
}

#[test]
fn namespace_dag_service_heals_pre_existing_duplicate_heads() {
    let store = test_store();
    let namespace_id = [0x78; 32];

    let delta_a = [0xA1; 32];
    let delta_b = [0xB2; 32];
    let delta_c = [0xC3; 32];

    // Plant a corrupted head set directly.
    let mut handle = store.handle();
    handle
        .put(
            &calimero_store::key::NamespaceGovHead::new(namespace_id),
            &calimero_store::key::NamespaceGovHeadValue {
                sequence: 5,
                dag_heads: vec![delta_a, delta_b, delta_a, delta_b],
            },
        )
        .unwrap();
    drop(handle);

    // Read de-dups on the fly (preserving first-seen order).
    let dag = NamespaceDagService::new(&store, namespace_id);
    let head = dag.read_head_record().unwrap();
    assert_eq!(head.parent_hashes, vec![delta_a, delta_b]);
    assert_eq!(head.next_nonce, 6);

    // The next governance op heals the persisted value too: it supersedes
    // `delta_b` (a parent) and appends `delta_c` exactly once.
    dag.advance_dag_head(delta_c, &[delta_b], 6).unwrap();
    let raw = raw_namespace_dag_heads(&store, namespace_id);
    assert_eq!(raw, vec![delta_a, delta_c]);
}

fn raw_namespace_dag_heads(store: &Store, namespace_id: [u8; 32]) -> Vec<[u8; 32]> {
    store
        .handle()
        .get(&calimero_store::key::NamespaceGovHead::new(namespace_id))
        .unwrap()
        .map(|h| h.dag_heads)
        .unwrap_or_default()
}

#[test]
fn namespace_dag_service_collects_skeleton_delta_ids_by_group() {
    use calimero_context_client::local_governance::{OpaqueSkeleton, StoredNamespaceEntry};

    let store = test_store();
    let namespace_id = [0x74; 32];
    let group_a = ContextGroupId::from([0x75; 32]);
    let group_b = ContextGroupId::from([0x76; 32]);
    let dag = NamespaceDagService::new(&store, namespace_id);
    let delta_a = [0xA1; 32];
    let delta_b = [0xB2; 32];
    let delta_other_ns = [0xC3; 32];
    let signer = PublicKey::from([0x61; 32]);

    let mut handle = store.handle();
    let key_a = calimero_store::key::NamespaceGovOp::new(namespace_id, delta_a);
    let key_b = calimero_store::key::NamespaceGovOp::new(namespace_id, delta_b);
    let key_other_ns = calimero_store::key::NamespaceGovOp::new([0x99; 32], delta_other_ns);

    let val_a = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Opaque(OpaqueSkeleton {
            delta_id: delta_a,
            parent_op_hashes: vec![],
            group_id: group_a.to_bytes(),
            signer,
        }))
        .unwrap(),
    };
    let val_b = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Opaque(OpaqueSkeleton {
            delta_id: delta_b,
            parent_op_hashes: vec![delta_a],
            group_id: group_b.to_bytes(),
            signer,
        }))
        .unwrap(),
    };
    // Different namespace id should be ignored by the iteration.
    let val_other_ns = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Opaque(OpaqueSkeleton {
            delta_id: delta_other_ns,
            parent_op_hashes: vec![],
            group_id: group_a.to_bytes(),
            signer,
        }))
        .unwrap(),
    };
    handle.put(&key_a, &val_a).unwrap();
    handle.put(&key_b, &val_b).unwrap();
    handle.put(&key_other_ns, &val_other_ns).unwrap();
    drop(handle);

    let collected = dag
        .collect_skeleton_delta_ids_for_group(group_a.to_bytes())
        .unwrap();
    assert_eq!(collected, vec![delta_a]);
}

#[test]
fn namespace_op_log_service_reads_signed_and_skeleton_entries() {
    use calimero_context_client::local_governance::{
        NamespaceOp, OpaqueSkeleton, SignedNamespaceOp, StoredNamespaceEntry,
    };
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let namespace_id = [0x77; 32];
    let group_a = ContextGroupId::from([0x78; 32]);
    let group_b = ContextGroupId::from([0x79; 32]);
    let signer_sk = PrivateKey::random(&mut rng);

    let signed_group = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: group_a.to_bytes(),
            key_id: [0x01; 32],
            encrypted: GroupKeyring::encrypt_op(&[0xA1; 32], &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    let signed_delta = signed_group.content_hash().unwrap();

    let mut handle = store.handle();
    let key_signed = calimero_store::key::NamespaceGovOp::new(namespace_id, signed_delta);
    let val_signed = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Signed(signed_group)).unwrap(),
    };
    handle.put(&key_signed, &val_signed).unwrap();

    let skeleton_delta = [0xB2; 32];
    let key_skeleton = calimero_store::key::NamespaceGovOp::new(namespace_id, skeleton_delta);
    let val_skeleton = calimero_store::key::NamespaceGovOpValue {
        skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Opaque(OpaqueSkeleton {
            delta_id: skeleton_delta,
            parent_op_hashes: vec![],
            group_id: group_b.to_bytes(),
            signer: signer_sk.public_key(),
        }))
        .unwrap(),
    };
    handle.put(&key_skeleton, &val_skeleton).unwrap();
    drop(handle);

    let op_log = NamespaceOpLogService::new(&store, namespace_id);

    let decoded_signed = op_log
        .collect_signed_group_ops_for_group(group_a.to_bytes())
        .unwrap();
    assert_eq!(decoded_signed.len(), 1);
    assert_eq!(
        decoded_signed[0].signed_op.content_hash().unwrap(),
        signed_delta,
        "signed op should be decoded with stable delta id",
    );
    assert_eq!(decoded_signed[0].key_id, [0x01; 32]);

    let decoded_skeleton = op_log
        .collect_opaque_skeleton_delta_ids_for_group(group_b.to_bytes())
        .unwrap();
    assert_eq!(decoded_skeleton, vec![skeleton_delta]);
}

#[test]
fn namespace_op_log_service_reads_tagged_and_legacy_rows() {
    use calimero_context_client::local_governance::{
        NamespaceOp, OpaqueSkeleton, SignedNamespaceOp, StoredNamespaceEntry,
    };
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let namespace_id = [0x70; 32];
    let group = ContextGroupId::from([0x71; 32]);
    let signer_sk = PrivateKey::random(&mut rng);

    let tagged_signed = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: group.to_bytes(),
            key_id: [0x12; 32],
            encrypted: GroupKeyring::encrypt_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    let tagged_delta = tagged_signed.content_hash().unwrap();
    let tagged_signed_key_id = match tagged_signed.op {
        NamespaceOp::Group { key_id, .. } => key_id,
        _ => panic!("expected group-scoped namespace op"),
    };

    let legacy_skeleton_delta = [0x13; 32];
    let legacy_skeleton = OpaqueSkeleton {
        delta_id: legacy_skeleton_delta,
        parent_op_hashes: vec![],
        group_id: group.to_bytes(),
        signer: signer_sk.public_key(),
    };

    let mut handle = store.handle();
    let tagged_key = calimero_store::key::NamespaceGovOp::new(namespace_id, tagged_delta);
    handle
        .put(
            &tagged_key,
            &calimero_store::key::NamespaceGovOpValue {
                skeleton_bytes: borsh::to_vec(&StoredNamespaceEntry::Signed(tagged_signed))
                    .unwrap(),
            },
        )
        .unwrap();

    let legacy_key = calimero_store::key::NamespaceGovOp::new(namespace_id, legacy_skeleton_delta);
    handle
        .put(
            &legacy_key,
            &calimero_store::key::NamespaceGovOpValue {
                skeleton_bytes: borsh::to_vec(&legacy_skeleton).unwrap(),
            },
        )
        .unwrap();
    drop(handle);

    let op_log = NamespaceOpLogService::new(&store, namespace_id);
    let signed = op_log
        .collect_signed_group_ops_for_group(group.to_bytes())
        .unwrap();
    assert_eq!(signed.len(), 1);
    assert_eq!(signed[0].key_id, tagged_signed_key_id);

    let skeletons = op_log
        .collect_opaque_skeleton_delta_ids_for_group(group.to_bytes())
        .unwrap();
    assert_eq!(skeletons, vec![legacy_skeleton_delta]);
}

#[test]
fn namespace_op_log_service_collects_group_scoped_signed_ops() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let namespace_id = [0x7A; 32];
    let group_a = ContextGroupId::from([0x7B; 32]);
    let group_b = ContextGroupId::from([0x7C; 32]);
    let signer_sk = PrivateKey::random(&mut rng);

    let op_a = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: group_a.to_bytes(),
            key_id: [0x11; 32],
            encrypted: GroupKeyring::encrypt_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let op_b = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: group_b.to_bytes(),
            key_id: [0x22; 32],
            encrypted: GroupKeyring::encrypt_op(&[0xBB; 32], &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let root = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        3,
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![1, 2, 3],
        }),
    )
    .unwrap();

    let op_log = NamespaceOpLogService::new(&store, namespace_id);
    op_log.store_signed_operation(&op_a).unwrap();
    op_log.store_signed_operation(&op_b).unwrap();
    op_log.store_signed_operation(&root).unwrap();

    let selected = op_log
        .collect_signed_group_ops_for_group(group_a.to_bytes())
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected[0].signed_op.content_hash().unwrap(),
        op_a.content_hash().unwrap()
    );
    assert_eq!(selected[0].key_id, [0x11; 32]);
}

#[test]
fn namespace_retry_service_collects_only_retryable_group_ops() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let namespace_id = [0x81; 32];
    let group_a = ContextGroupId::from([0x82; 32]);
    let group_b = ContextGroupId::from([0x83; 32]);
    let signer_sk = PrivateKey::random(&mut rng);

    let group_key = [0x91; 32];
    let key_id = GroupKeyring::new(&store, group_a)
        .store_key(&group_key)
        .unwrap();

    let encrypted_a = GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap();
    let encrypted_b = GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap();

    let group_a_op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: group_a.to_bytes(),
            key_id,
            encrypted: encrypted_a,
            key_rotation: None,
        },
    )
    .unwrap();

    let group_b_op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: group_b.to_bytes(),
            key_id,
            encrypted: encrypted_b,
            key_rotation: None,
        },
    )
    .unwrap();

    let root_op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        3,
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![7, 8, 9],
        }),
    )
    .unwrap();

    let governance = NamespaceGovernance::new(&store, namespace_id);
    governance.store_operation(&group_a_op).unwrap();
    governance.store_operation(&group_b_op).unwrap();
    governance.store_operation(&root_op).unwrap();

    let retry = NamespaceRetryService::new(&store, namespace_id);
    let retryable = retry
        .collect_retry_candidates_for_group(group_a.to_bytes())
        .unwrap();

    assert_eq!(retryable.len(), 1, "expected only one retryable op");
    match &retryable[0].signed_op.op {
        NamespaceOp::Group { group_id, .. } => assert_eq!(*group_id, group_a.to_bytes()),
        _ => panic!("expected group op"),
    }
}

#[test]
fn namespace_retry_service_orders_candidates_by_signer_nonce() {
    // Regression test for #2349: when a peer buffers several
    // `NamespaceOp::Group` ops from the same signer pending
    // `KeyDelivery`, the retry walk must apply them in nonce-ascending
    // order. Otherwise `apply_group_op_inner`'s
    // `if nonce <= last { skip duplicate }` check turns out-of-order
    // application into permanent data loss: a later op applies first,
    // bumps `last_nonce`, and every earlier op from the same signer
    // gets dropped on the floor. This regression manifested in the
    // group-metadata e2e as `ContextRegistered` (nonce N) being lost
    // when `MemberAdded` (nonce N+5, lower content-hash) retried
    // first, then `ContextMetadataSet` permanently bailing at the
    // "context not registered in this group" check.
    //
    // Test rigor: this test searches for a `signer_sk` whose 4 signed
    // ops' content-hash ordering differs from nonce ordering, so the
    // pre-sort iteration order is provably NOT [1,2,3,4]. That way,
    // the post-sort `assert_eq!(nonces, vec![1,2,3,4])` is a real
    // signal of the fix doing work — without the sort, the assertion
    // fails. (The previous version of this test could have passed by
    // coincidence if the random key happened to produce content
    // hashes in nonce order.)
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let namespace_id = [0x84; 32];
    let group = ContextGroupId::from([0x85; 32]);

    let group_key = [0x95; 32];
    let key_id = GroupKeyring::new(&store, group)
        .store_key(&group_key)
        .unwrap();

    // Search the random-key space for a signer whose 4 signed ops
    // produce a content-hash iteration order DIFFERENT from nonce
    // order. P(success per attempt) ≥ 23/24 ≈ 96 % (any non-identity
    // permutation works), so 64 attempts have a silent-pass
    // probability of (1/24)^64 ≈ 10^-88. The explicit `found` flag
    // and the post-loop `assert!(found, …)` make that path loud
    // rather than silently succeeding with in-order ops.
    let max_attempts = 64;
    let mut found = false;
    let raw_nonces: Vec<u64>;
    for _ in 0..max_attempts {
        let signer_sk = PrivateKey::random(&mut rng);
        let signed_ops: Vec<SignedNamespaceOp> = (1u64..=4)
            .map(|nonce| {
                SignedNamespaceOp::sign(
                    &signer_sk,
                    namespace_id,
                    vec![],
                    [0u8; 32],
                    nonce,
                    NamespaceOp::Group {
                        group_id: group.to_bytes(),
                        key_id,
                        encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
                        key_rotation: None,
                    },
                )
                .unwrap()
            })
            .collect();

        // Predict the content-hash iteration order these ops would
        // produce in the store column (keyed by `(namespace_id,
        // content_hash)`), without actually persisting them yet.
        let mut by_hash: Vec<([u8; 32], u64)> = signed_ops
            .iter()
            .map(|op| (op.content_hash().unwrap(), op.nonce))
            .collect();
        by_hash.sort_by_key(|(h, _)| *h);
        let predicted_nonces: Vec<u64> = by_hash.iter().map(|(_, n)| *n).collect();
        if predicted_nonces == vec![1u64, 2, 3, 4] {
            // This signer happens to produce in-nonce-order content
            // hashes — wouldn't exercise the sort fix. Try again.
            continue;
        }

        // Good signer found. Persist via the same path used by the
        // sibling test (`NamespaceGovernance::store_operation`), then
        // confirm the raw op-log iteration actually came back in the
        // predicted not-nonce-order — i.e. the bug path is reachable.
        let governance = NamespaceGovernance::new(&store, namespace_id);
        for op in &signed_ops {
            governance.store_operation(op).unwrap();
        }
        raw_nonces = NamespaceOpLogService::new(&store, namespace_id)
            .collect_signed_group_ops_for_group(group.to_bytes())
            .unwrap()
            .iter()
            .map(|e| e.signed_op.nonce)
            .collect();
        assert_ne!(
            raw_nonces,
            vec![1u64, 2, 3, 4],
            "test setup broken: raw op-log iteration is in nonce order, so the sort fix would be unreachable"
        );
        found = true;
        break;
    }
    assert!(
        found,
        "after {max_attempts} random signers, none produced content hashes out of nonce order — \
         either a vanishingly improbable coincidence or `content_hash`/op encoding changed. \
         Without an out-of-order raw iteration, the assertion below cannot distinguish a working \
         sort from a missing sort, so this test would silently pass on a regression."
    );

    let retry = NamespaceRetryService::new(&store, namespace_id);
    let candidates = retry
        .collect_retry_candidates_for_group(group.to_bytes())
        .unwrap();

    assert_eq!(candidates.len(), 4, "expected 4 retry candidates");

    // The fix's contract: candidates are sorted by (signer_bytes, nonce)
    // ascending. With a single signer, that's strict nonce order —
    // even though we just proved the raw op-log iteration was NOT in
    // nonce order. Without the sort fix in `NamespaceRetryService`,
    // this assertion would fail.
    let nonces: Vec<u64> = candidates.iter().map(|c| c.signed_op.nonce).collect();
    assert_eq!(
        nonces,
        vec![1, 2, 3, 4],
        "retry candidates must apply in nonce order, not content-hash order"
    );
}

#[test]
fn namespace_nesting_resolve_and_read_only_checks() {
    let store = test_store();
    let parent = ContextGroupId::from([0xA1; 32]);
    let child = ContextGroupId::from([0xA2; 32]);
    let grandchild = ContextGroupId::from([0xA3; 32]);
    let outsider = ContextGroupId::from([0xA4; 32]);
    let context = ContextId::from([0xB1; 32]);
    let ro_member = PublicKey::from([0xB2; 32]);
    let rw_member = PublicKey::from([0xB3; 32]);

    NamespaceRepository::new(&store)
        .nest(&parent, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();
    assert!(NamespaceRepository::new(&store)
        .nest(&grandchild, &parent)
        .is_err());

    let children = NamespaceRepository::new(&store)
        .list_children(&parent)
        .unwrap();
    assert_eq!(children, vec![child]);
    let descendants = NamespaceRepository::new(&store)
        .collect_descendants(&parent)
        .unwrap();
    assert!(descendants.contains(&child));
    assert!(descendants.contains(&grandchild));

    assert_eq!(
        NamespaceRepository::new(&store)
            .resolve(&grandchild)
            .unwrap(),
        parent
    );
    assert_eq!(
        NamespaceRepository::new(&store).resolve(&outsider).unwrap(),
        outsider
    );

    register_context_in_group(&store, &child, &context).unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &ro_member, GroupMemberRole::ReadOnly)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &rw_member, GroupMemberRole::Member)
        .unwrap();
    assert!(NamespaceRepository::new(&store)
        .is_read_only_for_context(&context, &ro_member)
        .unwrap());
    assert!(!NamespaceRepository::new(&store)
        .is_read_only_for_context(&context, &rw_member)
        .unwrap());
}

#[test]
fn authorized_for_state_op_admits_admin_and_member_only() {
    let store = test_store();
    let gid = ContextGroupId::from([0xC0; 32]);
    let context = ContextId::from([0xC1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);
    let ro = PublicKey::from([0x03; 32]);
    let ro_tee = PublicKey::from([0x04; 32]);
    let outsider = PublicKey::from([0x05; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &ro, GroupMemberRole::ReadOnly)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &ro_tee, GroupMemberRole::ReadOnlyTee)
        .unwrap();

    assert!(
        NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &admin)
            .unwrap(),
        "Admin must be authorized to author state ops"
    );
    assert!(
        NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &member)
            .unwrap(),
        "Member must be authorized to author state ops"
    );
    assert!(
        !NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &ro)
            .unwrap(),
        "ReadOnly must NOT be authorized to author state ops"
    );
    assert!(
        !NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &ro_tee)
            .unwrap(),
        "ReadOnlyTee must NOT be authorized to author state ops"
    );
    assert!(
        !NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &outsider)
            .unwrap(),
        "Non-member must NOT be authorized to author state ops"
    );
}

#[test]
fn authorized_for_state_op_rejects_removed_member() {
    let store = test_store();
    let gid = ContextGroupId::from([0xD0; 32]);
    let context = ContextId::from([0xD1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xDD; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();

    // Member is authorized while in the group.
    assert!(NamespaceRepository::new(&store)
        .is_authorized_for_context_state_op(&context, &target)
        .unwrap());

    // After removal: the GroupMember row is gone (apply path deletes it),
    // and the deny-list flags the identity as denied — both ways the
    // check must return `false`. The B3 receive path rejects deltas
    // from this identity at the cut; this check rejects local state ops
    // by the same identity at the WASM-execute path.
    MembershipRepository::new(&store)
        .remove_member(&gid, &target)
        .unwrap();

    assert!(
        !NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &target)
            .unwrap(),
        "Removed member must NOT be authorized to author state ops locally"
    );
}

#[test]
fn authorized_for_state_op_recognises_namespace_creator() {
    // Namespace creator does not have a `GroupMember` row at namespace
    // genesis — their admin authority lives in `GroupMeta::admin_identity`.
    // The check must use the same `is_group_admin` carve-out as the
    // receive-side `membership_status_at`, or the creator's local state
    // ops on a fresh namespace would be wrongly rejected.
    let store = test_store();
    let gid = ContextGroupId::from([0xE0; 32]);
    let context = ContextId::from([0xE1; 32]);
    let creator = PublicKey::from([0xEE; 32]);

    let mut meta = test_meta();
    meta.admin_identity = creator;
    meta.owner_identity = creator;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    // No `GroupMember` row for the creator — relies on the
    // `is_group_admin` carve-out.

    assert!(
        NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &creator)
            .unwrap(),
        "Namespace creator must be authorized via the admin-identity carve-out"
    );
}

#[test]
fn authorized_for_state_op_allows_non_group_context() {
    // A context that isn't registered under any group has no
    // group-membership concept to enforce. The check returns `true`
    // (no enforcement) so legacy / non-group contexts keep working.
    let store = test_store();
    let context = ContextId::from([0xF1; 32]);
    let executor = PublicKey::from([0xF2; 32]);

    assert!(
        NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &executor)
            .unwrap(),
        "Non-group context must allow any executor (nothing to enforce)"
    );
}

#[test]
fn authorized_for_state_op_admits_inherited_members_via_open_subgroup() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    // Members of an Open subgroup don't necessarily have a stored
    // `GroupMember` row at the subgroup level — they reach the subgroup
    // via the parent-walk with `CAN_JOIN_OPEN_SUBGROUPS` at the anchor.
    // The receive-side `membership_status_at` folds Inherited →
    // Member(Member); this check has to agree or the two checks
    // disagree on the same identity (receive accepts their deltas
    // while local-execute drops their state ops). That divergence
    // broke `group-subgroup-visibility-inheritance` and adjacent
    // workflows on an earlier draft of this PR.
    let store = test_store();
    let ns = ContextGroupId::from([0xC0; 32]);
    let child = ContextGroupId::from([0xC1; 32]);
    let context = ContextId::from([0xC2; 32]);
    let admin = PublicKey::from([0xC3; 32]);
    let inherited = PublicKey::from([0xC4; 32]);

    nest_for_test(&store, &ns, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&store).save(&ns, &meta).unwrap();
    MetaRepository::new(&store).save(&child, &meta).unwrap();

    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &inherited, GroupMemberRole::Member)
        .unwrap();

    // Register the context under the (Open) child — `inherited` has
    // no row in `child`, only in `ns`.
    register_context_in_group(&store, &child, &context).unwrap();

    assert!(
        NamespaceRepository::new(&store)
            .is_authorized_for_context_state_op(&context, &inherited)
            .unwrap(),
        "Inherited member of an Open subgroup must be authorized to author state ops"
    );
}

#[test]
fn replica_applies_tee_policy_then_membership_via_namespace_governance() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    // The verifier (owner/admin of the namespace) AUTHORS both ops. On the
    // replica this is a remote signer — the node under test is NOT the author.
    let verifier_sk = PrivateKey::random(&mut rng);
    let verifier_pk = verifier_sk.public_key();

    // The TEE node being admitted as a ReadOnlyTee member.
    let tee_member = PublicKey::from([0xD3; 32]);
    let quote_hash = [0xE1; 32];

    // Namespace root group (policy ops are namespace-scoped: must be the root).
    let namespace_id = [0xA7u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    // Replica bootstrap state: namespace meta + the verifier recorded as an
    // admin member (so `require_tee_attestation_verifier_membership` passes —
    // in the real fleet-join flow this row is seeded from the KeyDelivery
    // signer by `seed_bootstrap_admin_if_absent`), plus the group key the
    // replica received via KeyDelivery so it can decrypt the group ops.
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(verifier_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &verifier_pk, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x97u8; 32];
    let key_id = GroupKeyring::new(&store, ns_gid)
        .store_key(&group_key)
        .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Sanity: before any op is applied the replica has no policy and the TEE
    // node is not yet a member.
    assert!(
        read_tee_admission_policy(&store, &ns_gid)
            .unwrap()
            .is_none(),
        "no policy should exist before any op is applied on the replica"
    );

    // ---- Op 1 (nonce 1): TeeAdmissionPolicySet, authored by the verifier. ----
    // `accept_mock` with allowlists that match the join op's mock measurements
    // (empty RTMR lists allow all; mrtd/tcb_status are matched explicitly).
    let policy_op = GroupKeyring::encrypt_op(
        &group_key,
        &GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m1".to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: true,
        },
    )
    .unwrap();
    let policy_ns_op = SignedNamespaceOp::sign(
        &verifier_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: policy_op,
            key_rotation: None,
        },
    )
    .unwrap();
    gov.apply_signed_op(&policy_ns_op)
        .expect("replica must apply TeeAdmissionPolicySet");

    // After the policy op, a log-scanning reader on the REPLICA must see it —
    // this is the read the membership op depends on. Without the op-log
    // persistence fix the policy op leaves no log entry here.
    let policy = read_tee_admission_policy(&store, &ns_gid)
        .unwrap()
        .expect("policy must be visible on the replica after applying it");
    assert_eq!(policy.allowed_mrtd, vec!["m1".to_owned()]);
    assert!(policy.accept_mock);

    // ---- Op 2 (nonce 2): MemberJoinedViaTeeAttestation, authored by the
    // verifier — applied next, exactly as in the retry batch. Its apply reads
    // the policy from the op-log; with the fix that read succeeds. ----
    let join_op = GroupKeyring::encrypt_op(
        &group_key,
        &GroupOp::MemberJoinedViaTeeAttestation {
            member: tee_member,
            quote_hash,
            mrtd: "m1".to_owned(),
            rtmr0: "r0".to_owned(),
            rtmr1: "r1".to_owned(),
            rtmr2: "r2".to_owned(),
            rtmr3: "r3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let join_ns_op = SignedNamespaceOp::sign(
        &verifier_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: join_op,
            key_rotation: None,
        },
    )
    .unwrap();
    gov.apply_signed_op(&join_ns_op).expect(
        "replica must apply MemberJoinedViaTeeAttestation — the just-applied \
         TeeAdmissionPolicySet must be visible in its op-log (PR #2473 fix)",
    );

    // (a) policy still resolves; (b) the ReadOnlyTee member row exists and the
    // op-log records the admission; (c) the member count reflects it (verifier
    // admin + the newly admitted TEE node).
    assert!(
        read_tee_admission_policy(&store, &ns_gid)
            .unwrap()
            .is_some(),
        "policy must remain readable after the membership op applies"
    );
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &tee_member)
            .unwrap(),
        Some(GroupMemberRole::ReadOnlyTee),
        "the TEE node must be recorded as a ReadOnlyTee member on the replica"
    );
    assert!(
        is_tee_admitted_identity(&store, &ns_gid, &tee_member).unwrap(),
        "the admission op must be visible in the replica's op-log"
    );
    assert!(
        is_quote_hash_used(&store, &ns_gid, &quote_hash).unwrap(),
        "the admission quote hash must be recorded in the replica's op-log"
    );
    assert_eq!(
        MembershipRepository::new(&store).count(&ns_gid).unwrap(),
        2,
        "verifier admin + newly admitted ReadOnlyTee member"
    );
}

#[test]
fn tee_replica_seed_bootstrap_admits_tee_with_open_join_cap() {
    // Regression: a TEE replica that bootstraps the namespace ROOT via the
    // `seed_bootstrap_admin_if_absent` (KeyDelivery-seed) path used to leave the
    // root's `default_capabilities` UNSET. The subsequently-applied
    // `MemberJoinedViaTeeAttestation` op snapshots the group's default caps at
    // apply time, so the ReadOnlyTee row was written with `caps = 0` — and
    // `check_path` of any Open child subgroup then resolves to `None`, so
    // auto-follow never `join_context`s and the Open subgroup never replicates.
    //
    // The fix completes the seed by also seeding the root's default caps to
    // include `CAN_JOIN_OPEN_SUBGROUPS` (mirroring the owner-side
    // `store_group_meta` precedent). This test seeds the root via the bare seed,
    // admits a ReadOnlyTee via the real op path, and asserts (a) the TEE's root
    // row HAS `CAN_JOIN_OPEN_SUBGROUPS` and (b) `check_path(open_child, tee)`
    // resolves to `Inherited`. It FAILS before the fix and PASSES after.
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    // The founder/verifier = the KeyDelivery signer the replica TOFU-trusts.
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();

    // The TEE node being admitted.
    let tee_member = PublicKey::from([0xD7; 32]);
    let quote_hash = [0xE7; 32];

    let namespace_id = [0xB4u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let open_child = ContextGroupId::from([0xB5u8; 32]);

    let gov = NamespaceGovernance::new(&store, namespace_id);

    // ---- Genesis establishes the founder as the authoritative namespace admin
    // (#2474: this used to come from the bootstrap seed's KeyDelivery-signer
    // TOFU; it now comes from the replayable `NamespaceCreated` genesis op). The
    // founder here IS the verifier that authors the TEE ops below, so it must be
    // admin for those ops to apply. ----
    {
        use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
        let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
        let signed_genesis =
            SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
                .expect("sign genesis");
        gov.apply_signed_op(&signed_genesis)
            .expect("genesis NamespaceCreated establishes the founding admin");
    }

    // The bootstrap seed still runs on the real fleet-join KeyDelivery path; it
    // is now a no-op for the (already established) meta but still ensures the
    // root's default caps include CAN_JOIN_OPEN_SUBGROUPS.
    gov.seed_bootstrap_admin_if_absent(namespace_id, &founder)
        .expect("bootstrap seed");

    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&ns_gid)
            .unwrap(),
        Some(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS),
        "genesis/seed must set the root's default caps so members admitted \
         before the DefaultCapabilitiesSet gossip inherit CAN_JOIN_OPEN_SUBGROUPS"
    );

    // An Open child subgroup nested under the root.
    nest_for_test(&store, &ns_gid, &open_child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&open_child, VisibilityMode::Open)
        .unwrap();

    // The replica holds the group key (delivered via KeyDelivery) so it can
    // decrypt the encrypted group ops below.
    let group_key = [0x71u8; 32];
    let key_id = GroupKeyring::new(&store, ns_gid)
        .store_key(&group_key)
        .unwrap();

    // ---- Op 1 (nonce 1): TeeAdmissionPolicySet, authored by the founder. ----
    let policy_op = GroupKeyring::encrypt_op(
        &group_key,
        &GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m1".to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: true,
        },
    )
    .unwrap();
    let policy_ns_op = SignedNamespaceOp::sign(
        &founder_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: policy_op,
            key_rotation: None,
        },
    )
    .unwrap();
    gov.apply_signed_op(&policy_ns_op)
        .expect("replica must apply TeeAdmissionPolicySet");

    // ---- Op 2 (nonce 2): MemberJoinedViaTeeAttestation — the row whose caps
    // are snapshotted from the root's default caps at apply time. ----
    let join_op = GroupKeyring::encrypt_op(
        &group_key,
        &GroupOp::MemberJoinedViaTeeAttestation {
            member: tee_member,
            quote_hash,
            mrtd: "m1".to_owned(),
            rtmr0: "r0".to_owned(),
            rtmr1: "r1".to_owned(),
            rtmr2: "r2".to_owned(),
            rtmr3: "r3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let join_ns_op = SignedNamespaceOp::sign(
        &founder_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: join_op,
            key_rotation: None,
        },
    )
    .unwrap();
    gov.apply_signed_op(&join_ns_op)
        .expect("replica must apply MemberJoinedViaTeeAttestation");

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &tee_member)
            .unwrap(),
        Some(GroupMemberRole::ReadOnlyTee),
        "the TEE node must be recorded as a ReadOnlyTee member on the replica"
    );

    // (a) The TEE's ROOT row must carry CAN_JOIN_OPEN_SUBGROUPS — snapshotted
    // from the seeded default caps at admission time.
    let tee_root_caps = CapabilitiesRepository::new(&store)
        .member_capability(&ns_gid, &tee_member)
        .unwrap()
        .unwrap_or(0);
    assert_ne!(
        tee_root_caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        0,
        "TEE root row must carry CAN_JOIN_OPEN_SUBGROUPS (got caps={tee_root_caps:#b})"
    );

    // (b) check_path of the Open child must resolve to Inherited via the root
    // anchor — i.e. auto-follow would join_context and replicate the subgroup.
    assert!(
        matches!(
            MembershipRepository::new(&store)
                .check_path(&open_child, &tee_member)
                .unwrap(),
            crate::membership::MembershipPath::Inherited { .. }
        ),
        "Open child must resolve to Inherited for the TEE so its context replicates"
    );
}

#[test]
fn replica_genesis_founder_survives_non_owner_seed_and_applies_owner_ops() {
    // #2474 REGRESSION GUARD (was the RED reproduction; now GREEN under Option A).
    //
    // BEFORE the fix: `seed_bootstrap_admin_if_absent` TOFU-seeded the founding
    // admin from the *KeyDelivery signer*. The signer need only HOLD the group
    // key (any current member), so when a NON-OWNER delivered the key the replica
    // pinned the WRONG admin and REJECTED the true owner's first authority-bearing
    // root op (`GroupCreated` under the root), wedging backfill permanently.
    //
    // AFTER the fix (Option A): namespace root creation emits a replayable
    // `RootOp::NamespaceCreated { founder }` GENESIS op. A backfilling replica
    // applies it (the parentless FIRST op in the DAG) BEFORE any owner op, so the correct
    // founding admin is established authoritatively from the synced DAG — the
    // non-owner KeyDelivery seed can no longer pin the wrong admin, and the
    // owner's `GroupCreated` APPLIES.
    //
    // This exercises the realistic backfill order (genesis applied first, then a
    // non-owner KeyDelivery seed lands, then the owner's GroupCreated) against the
    // SAME apply path the backfill uses (`NamespaceGovernance::apply_signed_op`).
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    // The TRUE owner/founder of the namespace (in production: the node whose
    // keypair created the namespace root in `handlers/create_group.rs`).
    let owner_sk = PrivateKey::random(&mut rng);
    let owner = owner_sk.public_key();

    // A DIFFERENT, non-owner member of the namespace — whoever happened to
    // deliver the group key to the bootstrapping replica. It is NOT the owner.
    let non_owner_sk = PrivateKey::random(&mut rng);
    let non_owner = non_owner_sk.public_key();
    assert_ne!(
        owner, non_owner,
        "the key-deliverer must be a different identity than the true owner"
    );

    let namespace_id = [0xC4u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    let gov = NamespaceGovernance::new(&store, namespace_id);

    // ---- Step 1: the GENESIS op the backfill replays FIRST. Signed by the true
    // owner with NO parents (the defining genesis invariant) — exactly what
    // `handlers/create_group.rs` emits on root creation. (`op.nonce` is
    // informational here; sequencing comes from the head record. The 0 below is
    // an arbitrary placeholder the apply path does not consult for ordering.)
    // This establishes the founding admin authoritatively. ----
    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder: owner });
    let signed_genesis =
        SignedNamespaceOp::sign(&owner_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
            .expect("owner signs NamespaceCreated genesis");
    gov.apply_signed_op(&signed_genesis)
        .expect("genesis NamespaceCreated must apply on the bare replica");

    // The true owner is now the recognised founding admin; the non-owner is not.
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &owner)
            .unwrap(),
        "genesis must establish the TRUE owner as the namespace admin"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&ns_gid, &non_owner)
            .unwrap(),
        "the non-owner must NOT be admin after genesis"
    );

    // ---- Step 2: a non-owner KeyDelivery seed lands. It used to overwrite the
    // admin; now it is forbidden from establishing authority — it adds only a
    // non-authoritative member row and never touches the established admin. ----
    gov.seed_bootstrap_admin_if_absent(namespace_id, &non_owner)
        .expect("bootstrap seed from the (non-owner) KeyDelivery signer");

    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&ns_gid, &non_owner)
            .unwrap(),
        "#2474: a non-owner KeyDelivery seed must NOT pin the admin (the wedge is gone)"
    );
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &owner)
            .unwrap(),
        "the true owner remains the admin after the non-owner seed"
    );

    // ---- Step 3: the owner's first authority-bearing root op now APPLIES. ----
    let subgroup_id = [0xC5u8; 32];
    let create_op = NamespaceOp::Root(RootOp::GroupCreated {
        group_id: subgroup_id,
        parent_id: namespace_id,
        restricted: true,
    });
    let signed = SignedNamespaceOp::sign(&owner_sk, namespace_id, vec![], [0u8; 32], 1, create_op)
        .expect("owner signs GroupCreated");

    gov.apply_signed_op(&signed).expect(
        "#2474 GREEN: owner-signed GroupCreated APPLIES once genesis establishes the admin",
    );

    // The subgroup meta is written — backfill is no longer wedged.
    assert!(
        MetaRepository::new(&store)
            .load(&ContextGroupId::from(subgroup_id))
            .unwrap()
            .is_some(),
        "the subgroup must be created — backfill proceeds past the owner's GroupCreated"
    );
}

#[test]
fn namespace_created_genesis_on_bare_store_and_anti_hijack() {
    // Unit coverage for the `NamespaceCreated` apply handler (#2474):
    //  (a) on a BARE store it writes admin == owner == founder with no prior
    //      state and the default CAN_JOIN_OPEN_SUBGROUPS caps;
    //  (b) a SECOND `NamespaceCreated` (forged second genesis) on an established
    //      namespace is a NO-OP — it cannot overwrite the established admin;
    //  (c) a seed-PLACEHOLDER meta (admin == zero) does NOT block genesis —
    //      genesis fills in the real founder over it, proving seed-vs-genesis
    //      ordering converges either way.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();
    let attacker_sk = PrivateKey::random(&mut rng);
    let attacker = attacker_sk.public_key();

    // ---- (a) bare-store genesis ----
    {
        let store = test_store();
        let namespace_id = [0xA1u8; 32];
        let ns_gid = ContextGroupId::from(namespace_id);
        let gov = NamespaceGovernance::new(&store, namespace_id);

        let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
        let signed =
            SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
                .unwrap();
        gov.apply_signed_op(&signed)
            .expect("bare-store genesis applies");

        let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
        assert_eq!(meta.admin_identity, founder, "admin == founder");
        assert_eq!(meta.owner_identity, founder, "owner == founder");
        assert!(
            MembershipRepository::new(&store)
                .is_admin(&ns_gid, &founder)
                .unwrap(),
            "founder is admin"
        );
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .default_capabilities(&ns_gid)
                .unwrap(),
            Some(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS),
            "genesis seeds default CAN_JOIN_OPEN_SUBGROUPS caps"
        );

        // ---- (b) anti-hijack: a second, forged genesis is a no-op ----
        let forged = NamespaceOp::Root(RootOp::NamespaceCreated { founder: attacker });
        let signed_forged =
            SignedNamespaceOp::sign(&attacker_sk, namespace_id, vec![], [0u8; 32], 1, forged)
                .unwrap();
        gov.apply_signed_op(&signed_forged)
            .expect("forged second genesis applies as a no-op (no error)");

        let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
        assert_eq!(
            meta.admin_identity, founder,
            "anti-hijack: established admin is NOT overwritten by a forged genesis"
        );
        assert!(
            !MembershipRepository::new(&store)
                .is_admin(&ns_gid, &attacker)
                .unwrap(),
            "anti-hijack: the attacker did not become admin"
        );
    }

    // ---- (c) placeholder seed meta does NOT block genesis ----
    {
        let store = test_store();
        let namespace_id = [0xA2u8; 32];
        let ns_gid = ContextGroupId::from(namespace_id);
        let gov = NamespaceGovernance::new(&store, namespace_id);

        // A non-owner seed runs first, writing placeholder meta (admin == zero).
        gov.seed_bootstrap_admin_if_absent(namespace_id, &attacker)
            .expect("placeholder seed");
        let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
        assert_eq!(
            meta.admin_identity,
            PublicKey::from([0u8; 32]),
            "seed writes a placeholder (zero) admin, granting authority to nobody"
        );
        assert!(
            !MembershipRepository::new(&store)
                .is_admin(&ns_gid, &attacker)
                .unwrap(),
            "the non-owner deliverer is NOT admin after the seed"
        );

        // Genesis then lands and fills in the real founder over the placeholder.
        let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
        let signed =
            SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
                .unwrap();
        gov.apply_signed_op(&signed)
            .expect("genesis applies over the placeholder seed meta");
        let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
        assert_eq!(
            meta.admin_identity, founder,
            "genesis overwrites the placeholder admin with the real founder"
        );
    }

    // ---- (d) nonce-0 forged genesis on a BARE store is rejected by the
    // signer==founder check, NOT silently accepted as the true genesis ----
    // #2474 reviewer batch 3, item #4: case (b) above covers a forged SECOND
    // genesis (nonce=1) on an established namespace, which the anti-hijack gate
    // turns into a no-op. This sub-case covers a forged FIRST genesis (nonce=0)
    // on a bare namespace: an attacker tries to be the true genesis but names a
    // DIFFERENT founder than they sign with. This is caught EARLIER, by the
    // signer==founder check (before the anti-hijack/established gate ever runs),
    // and is REJECTED (Err) rather than no-op'd — a mismatched forgery must never
    // pin an admin even on a fresh namespace.
    {
        let store = test_store();
        let namespace_id = [0xA3u8; 32];
        let ns_gid = ContextGroupId::from(namespace_id);
        let gov = NamespaceGovernance::new(&store, namespace_id);

        // Attacker signs the genuine FIRST op (nonce=0, empty parents) but names
        // the founder as someone else (here: `founder`).
        let forged = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
        let signed =
            SignedNamespaceOp::sign(&attacker_sk, namespace_id, vec![], [0u8; 32], 0, forged)
                .unwrap();
        assert!(
            gov.apply_signed_op(&signed).is_err(),
            "nonce-0 forged genesis (signer != founder) must be REJECTED by the \
             signer==founder check, not accepted as the true genesis"
        );
        assert!(
            MetaRepository::new(&store).load(&ns_gid).unwrap().is_none(),
            "rejected nonce-0 forgery leaves the bare namespace with no root meta"
        );
        assert!(
            !MembershipRepository::new(&store)
                .is_admin(&ns_gid, &founder)
                .unwrap(),
            "the falsely-declared founder was not made admin"
        );
        assert!(
            !MembershipRepository::new(&store)
                .is_admin(&ns_gid, &attacker)
                .unwrap(),
            "the attacker signer was not made admin"
        );
    }
}

#[test]
fn namespace_created_genesis_proceeds_when_only_admin_is_placeholder() {
    // #2474 reviewer batch 4-5, item #1: the anti-hijack gate keys SOLELY on
    // `admin_identity`. This pins the fix for the earlier OR-of-both gate, which
    // would have treated a meta with `admin_identity == placeholder` but
    // `owner_identity != placeholder` as "established" and wedged the namespace
    // with no real admin forever. The authority-field-only gate must instead let
    // genesis PROCEED on such a partial-write state and write the real founder
    // as admin (repair), since `admin_identity == placeholder` means no real
    // admin exists yet.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();
    let stray_owner_sk = PrivateKey::random(&mut rng);
    let stray_owner = stray_owner_sk.public_key();

    let store = test_store();
    let namespace_id = [0xA4u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Construct a partial-write state: admin_identity is still the placeholder
    // sentinel (no real admin), but owner_identity is a real (non-placeholder)
    // key. The OR-of-both gate would have called this "established" and refused
    // genesis; the authority-field-only gate must not.
    let mut partial = sample_meta_with_admin(founder);
    partial.admin_identity = PublicKey::from([0u8; 32]);
    partial.owner_identity = stray_owner;
    MetaRepository::new(&store).save(&ns_gid, &partial).unwrap();

    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
    let signed = SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
        .expect("founder signs NamespaceCreated genesis");
    gov.apply_signed_op(&signed)
        .expect("genesis proceeds when admin_identity is still the placeholder");

    let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
    assert_eq!(
        meta.admin_identity, founder,
        "gate keys on admin_identity only: genesis repairs the placeholder admin to the founder"
    );
    assert_eq!(
        meta.owner_identity, founder,
        "genesis establishes the founder as owner too"
    );
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "founder is admin after the repairing genesis"
    );
}

#[test]
fn namespace_created_genesis_upgrades_seeded_member_founder_to_admin() {
    // #2474 reviewer batch 2, item #4: when the bootstrap seed runs FIRST for
    // the FOUNDER's own identity, it writes the founder as a non-authoritative
    // `Member` placeholder row (seed never confers authority). The later genesis
    // op must make the handler SELF-CONTAINED: it must UPGRADE that existing
    // `Member` row to `Admin`, not no-op on it leaving a stale `Member`. This
    // guards the upsert semantics of `add_member` the genesis handler relies on.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();

    let store = test_store();
    let namespace_id = [0xD9u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // ---- Seed FIRST, for the founder's OWN identity (the deliverer happens to
    // be the founder). The seed writes placeholder meta + the founder as a
    // non-authoritative `Member`. ----
    gov.seed_bootstrap_admin_if_absent(namespace_id, &founder)
        .expect("bootstrap seed writes founder as a Member placeholder");

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "precondition: seed writes the founder as a non-authoritative Member"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "precondition: the seeded founder is NOT yet admin"
    );

    // ---- Genesis lands and must UPGRADE the founder Member row to Admin. ----
    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
    let signed = SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
        .expect("founder signs NamespaceCreated genesis");
    gov.apply_signed_op(&signed)
        .expect("genesis applies over the founder's seeded Member placeholder");

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        Some(GroupMemberRole::Admin),
        "#2474 item 4: genesis must UPGRADE the seeded Member row to Admin"
    );
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "founder is admin after genesis"
    );
    let meta = MetaRepository::new(&store).load(&ns_gid).unwrap().unwrap();
    assert_eq!(meta.admin_identity, founder, "admin_identity == founder");
    assert_eq!(meta.owner_identity, founder, "owner_identity == founder");
}

#[test]
fn namespace_created_genesis_ensures_member_row_for_established_founder() {
    // #2474 reviewer batch 3, item #2: if some path wrote a NON-placeholder
    // root `admin_identity` == founder BEFORE genesis arrives, the anti-hijack
    // gate takes the early-return "already established" path and does NOT
    // re-write the root meta. But the founder's explicit Admin MEMBER ROW may
    // never have been written by that path. The handler must, on this
    // SAME-founder early-return, still ensure the Admin member row exists
    // (idempotent upsert) so the founder is enumerable as Admin. Crucially this
    // must happen ONLY when the established admin == the op's founder; a
    // different established admin must stay a pure no-op (covered by the
    // anti-hijack case in `namespace_created_genesis_on_bare_store_and_anti_hijack`).
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();

    let store = test_store();
    let namespace_id = [0xE3u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Pre-establish the root meta with admin == owner == founder but write NO
    // member row — simulating a path that set a real `admin_identity` before
    // genesis applied.
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(founder))
        .unwrap();
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        None,
        "precondition: no explicit member row for the founder yet"
    );

    // Genesis arrives for the SAME founder. The established gate short-circuits
    // the meta rewrite but must still ensure the Admin member row.
    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
    let signed = SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 0, genesis)
        .expect("founder signs NamespaceCreated genesis");
    gov.apply_signed_op(&signed)
        .expect("genesis applies as an idempotent same-founder re-arrival");

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        Some(GroupMemberRole::Admin),
        "#2474 item 2: genesis must ensure the founder's Admin member row on the \
         established same-founder path"
    );
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "founder is admin after the idempotent genesis"
    );
    // #2474 reviewer batch 4-5, item #2: the same-founder early-return path must
    // also seed the Open-join default caps, not just the Admin member row. The
    // pre-established meta wrote no caps row, so genesis is responsible for it.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&ns_gid)
            .unwrap(),
        Some(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS),
        "#2474 item 2: same-founder re-arrival seeds default CAN_JOIN_OPEN_SUBGROUPS caps"
    );
}

#[test]
fn namespace_created_genesis_signer_must_equal_founder() {
    // #2474 review follow-up: genesis is self-authorizing (it skips
    // `require_namespace_admin`), so the ONLY thing binding the established
    // admin to a real signing key is the signer==founder check. A non-founder
    // who signs `NamespaceCreated { founder: <someone-else> }` with their own
    // key, on a namespace with no prior genesis, must be REJECTED — never
    // applied (which would pin a forged admin) and never silently no-op'd.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let attacker_sk = PrivateKey::random(&mut rng);
    let victim_sk = PrivateKey::random(&mut rng);
    let victim = victim_sk.public_key();

    let store = test_store();
    let namespace_id = [0xB7u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Attacker declares the victim as founder but signs with their OWN key.
    let forged = NamespaceOp::Root(RootOp::NamespaceCreated { founder: victim });
    let signed =
        SignedNamespaceOp::sign(&attacker_sk, namespace_id, vec![], [0u8; 32], 0, forged).unwrap();

    let res = gov.apply_signed_op(&signed);
    assert!(
        res.is_err(),
        "genesis whose signer != founder must be rejected, not applied"
    );

    // No root meta was written — the forged genesis pinned no admin.
    assert!(
        MetaRepository::new(&store).load(&ns_gid).unwrap().is_none(),
        "rejected genesis must leave the namespace with no root meta (no forged admin)"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&ns_gid, &victim)
            .unwrap(),
        "victim was not made admin by a forged genesis"
    );
}

#[test]
fn namespace_created_with_parents_is_rejected_as_non_genesis() {
    // #2474 batch-7: `NamespaceCreated` is the DAG ROOT — its defining invariant
    // is that it has NO parents. A brand-new namespace has an empty head, so the
    // real genesis (`handlers/create_group.rs` via `sign_apply_and_publish`) is
    // signed with `parent_op_hashes == []`. A `NamespaceCreated` carrying
    // parents was therefore minted against an EXISTING DAG head — injected late
    // onto a namespace with history — and must be REJECTED even when
    // signer == founder, so it can never establish/re-found the founder.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();

    let store = test_store();
    // Distinct from the `[0xD9u8; 32]` used by
    // `namespace_created_genesis_upgrades_seeded_member_founder_to_admin`. Each
    // test uses a fresh `test_store()` so the shared id was never a live bug,
    // but a unique id removes the latent collision and keeps the two tests
    // independent under any future shared-store refactor.
    let namespace_id = [0xDAu8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Self-consistent (signer == founder) but PARENTED genesis: a fabricated
    // parent op-hash makes this not the DAG root.
    let parented = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
    let signed = SignedNamespaceOp::sign(
        &founder_sk,
        namespace_id,
        vec![[0x11u8; 32]],
        [0u8; 32],
        2,
        parented,
    )
    .unwrap();

    let res = gov.apply_signed_op(&signed);
    assert!(
        res.is_err(),
        "a NamespaceCreated carrying parents must be rejected (not the DAG root), \
         even with signer == founder"
    );

    // No meta written — the parented op established no founder.
    assert!(
        MetaRepository::new(&store).load(&ns_gid).unwrap().is_none(),
        "rejected non-genesis NamespaceCreated must leave the namespace with no root meta"
    );
    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "no Admin row may be written for a rejected non-genesis NamespaceCreated"
    );

    // Sanity: the REAL genesis path (no parents, same founder) still applies.
    let genesis = NamespaceOp::Root(RootOp::NamespaceCreated { founder });
    let signed_genesis =
        SignedNamespaceOp::sign(&founder_sk, namespace_id, vec![], [0u8; 32], 1, genesis).unwrap();
    gov.apply_signed_op(&signed_genesis)
        .expect("parentless genesis (the DAG root) still applies");
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ns_gid, &founder)
            .unwrap(),
        "parentless genesis establishes the founder as Admin"
    );
}

#[test]
fn genesis_apply_failure_leaves_namespace_head_unadvanced() {
    // #2931 reviewer B1: pins the HEAD-ATOMICITY contract that lets
    // `handlers/create_group.rs` roll back a failed root-genesis WITHOUT
    // touching the `NamespaceGovHead`.
    //
    // The concern: if `apply_signed_op` advanced the DAG head while applying
    // the genesis and a later step failed, the head would stay advanced; a
    // retry would then re-sign the genesis with a non-empty `parent_op_hashes`
    // and trip the no-parents `NotGenesis` gate, wedging the namespace forever.
    //
    // It cannot happen because the apply is head-atomic BY ORDERING: in
    // `apply_signed_op` the op-kind apply (`apply_root_op` → the
    // `NamespaceCreated` handler) runs FIRST and only on its success does the
    // function reach `advance_dag_head` + `store_operation`. A failing genesis
    // `?`-propagates before the head is ever written. This test drives a
    // genuine genesis APPLY failure (signer != declared founder, a parentless
    // op so it is the would-be DAG root) and asserts the head is left exactly
    // as it was pre-genesis (empty heads, next_nonce == 1), so a retry re-signs
    // a clean parentless genesis that passes the gate.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let mut rng = OsRng;
    let attacker_sk = PrivateKey::random(&mut rng);
    let real_founder_sk = PrivateKey::random(&mut rng);
    let real_founder = real_founder_sk.public_key();

    let store = test_store();
    let namespace_id = [0xDBu8; 32];
    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Pre-genesis head is empty / absent: no heads, next_nonce starts at 1.
    let before = gov.read_head_record().unwrap();
    assert!(
        before.parent_hashes.is_empty(),
        "fresh namespace must have an empty DAG head before genesis"
    );
    assert_eq!(
        before.next_nonce, 1,
        "fresh namespace next_nonce must be 1 before genesis"
    );

    // A PARENTLESS NamespaceCreated (the would-be DAG root) whose apply FAILS:
    // signer (attacker) != declared founder (real_founder) trips the
    // SignerNotFounder gate inside the genesis handler — a real apply error
    // raised AFTER `apply_root_op` is entered but BEFORE `advance_dag_head`.
    let bad_genesis = NamespaceOp::Root(RootOp::NamespaceCreated {
        founder: real_founder,
    });
    let signed_bad = SignedNamespaceOp::sign(
        &attacker_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        bad_genesis,
    )
    .unwrap();

    let res = gov.apply_signed_op(&signed_bad);
    assert!(
        res.is_err(),
        "a genesis whose signer != declared founder must fail to apply"
    );

    // THE KEY ASSERTION: the failed genesis did NOT advance the head.
    let after = gov.read_head_record().unwrap();
    assert!(
        after.parent_hashes.is_empty(),
        "a failed genesis apply must NOT advance the namespace DAG head \
         (head-atomicity, #2931); leaving it advanced would wedge every retry"
    );
    assert_eq!(
        after.next_nonce, 1,
        "a failed genesis apply must leave next_nonce at the pre-genesis value"
    );

    // A clean, parentless retry by the REAL founder now applies — proving the
    // namespace is not wedged after the failed attempt.
    let good_genesis = NamespaceOp::Root(RootOp::NamespaceCreated {
        founder: real_founder,
    });
    let signed_good = SignedNamespaceOp::sign(
        &real_founder_sk,
        namespace_id,
        vec![], // empty parents — read from the still-empty head on retry
        [0u8; 32],
        1,
        good_genesis,
    )
    .unwrap();
    gov.apply_signed_op(&signed_good)
        .expect("clean parentless genesis applies after a prior failed attempt");

    // Genesis succeeded: head is now advanced exactly once.
    let final_head = gov.read_head_record().unwrap();
    assert_eq!(
        final_head.parent_hashes.len(),
        1,
        "successful genesis advances the head to a single root entry"
    );
    assert_eq!(
        final_head.next_nonce, 2,
        "successful genesis bumps next_nonce to 2"
    );
}

#[test]
fn replica_op_log_dedup_survives_head_pruning() {
    use calimero_context_client::local_governance::{
        NamespaceOp, SignedGroupOp, SignedNamespaceOp, SIGNED_GROUP_OP_SCHEMA_VERSION,
    };
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    let signer_sk = PrivateKey::random(&mut rng);
    let signer_pk = signer_sk.public_key();

    let namespace_id = [0xC4u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    // Replica bootstrap state: namespace meta with the signer as admin + the
    // group key so the encrypted ops decrypt.
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(signer_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &signer_pk, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5Au8; 32];
    let key_id = GroupKeyring::new(&store, ns_gid)
        .store_key(&group_key)
        .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);

    // Helper: build the SignedGroupOp exactly as `decrypt_and_apply_group_op`
    // reconstructs it from a namespace op, so we can compute its content hash.
    let group_op_content_hash = |ns_op: &SignedNamespaceOp, inner: &GroupOp| -> [u8; 32] {
        SignedGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: namespace_id,
            parent_op_hashes: ns_op.parent_op_hashes.clone(),
            state_hash: ns_op.state_hash,
            signer: ns_op.signer,
            nonce: ns_op.nonce,
            op: inner.clone(),
            signature: ns_op.signature,
        }
        .content_hash()
        .unwrap()
    };

    let make_policy_op = |mrtd: &str| GroupOp::TeeAdmissionPolicySet {
        allowed_mrtd: vec![mrtd.to_owned()],
        allowed_rtmr0: vec![],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec!["ok".to_owned()],
        accept_mock: true,
    };

    // ---- Op A (nonce 1). ----
    let inner_a = make_policy_op("mA");
    let op_a = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &inner_a).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    let hash_a = group_op_content_hash(&op_a, &inner_a);
    gov.apply_signed_op(&op_a).expect("apply op A");

    // After A: log has one entry, head frontier is [hash_a].
    assert_eq!(read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(), 1);
    assert!(get_op_head(&store, &ns_gid)
        .unwrap()
        .unwrap()
        .dag_heads
        .contains(&hash_a));

    // ---- Op B (nonce 2) supersedes A by listing A's group-op hash as parent.
    // This prunes hash_a out of the op-head's dag_heads. ----
    let inner_b = make_policy_op("mB");
    let op_b = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![hash_a],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &inner_b).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    gov.apply_signed_op(&op_b).expect("apply op B");

    // After B: log has two entries; hash_a is PRUNED from dag_heads (the old
    // `dag_heads.contains` check would now report A as not-logged) but the
    // persisted-log check still sees A.
    assert_eq!(read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(), 2);
    assert!(
        !get_op_head(&store, &ns_gid)
            .unwrap()
            .unwrap()
            .dag_heads
            .contains(&hash_a),
        "op A's hash must be pruned from dag_heads once B supersedes it"
    );
    assert!(
        super::super::local_state::op_log_contains_content_hash(&store, &ns_gid, &hash_a).unwrap(),
        "the persisted-log dedup must still see superseded op A"
    );

    // ---- Re-drive op A through the FULL apply path under the retry/backfill
    // condition the dedup exists for: nonce un-advanced + namespace-level dedup
    // not short-circuiting. ----
    set_local_gov_nonce(&store, &ns_gid, &signer_pk, 0).unwrap();
    {
        let mut handle = store.handle();
        let key =
            calimero_store::key::NamespaceGovOp::new(namespace_id, op_a.content_hash().unwrap());
        handle.delete(&key).unwrap();
    }
    gov.apply_signed_op(&op_a).expect("re-apply op A");

    // The decisive assertion: NO duplicate log entry was appended.
    assert_eq!(
        read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(),
        2,
        "re-applying superseded op A must NOT append a duplicate op-log entry"
    );
}

/// Regression test for #2516 on the namespace receive path (where concurrent
/// same-signer contention is the norm). Two sibling group ops with consecutive
/// nonces share the same (empty) DAG parent set, so they can arrive in either
/// order. Delivering nonce 2 before nonce 1 must still apply BOTH: the old
/// `nonce <= last` guard advanced to 2 on the first and then dropped nonce 1
/// permanently; the windowed guard parks 2 above the floor and applies 1 when
/// it lands.
#[test]
fn replica_concurrent_sibling_ops_apply_out_of_order_2516() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    let signer_sk = PrivateKey::random(&mut rng);
    let signer_pk = signer_sk.public_key();

    let namespace_id = [0xC6u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(signer_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &signer_pk, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5Cu8; 32];
    let key_id = GroupKeyring::new(&store, ns_gid)
        .store_key(&group_key)
        .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);

    let make_policy_op = |mrtd: &str| GroupOp::TeeAdmissionPolicySet {
        allowed_mrtd: vec![mrtd.to_owned()],
        allowed_rtmr0: vec![],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec!["ok".to_owned()],
        accept_mock: true,
    };

    // Both ops are DAG siblings: same empty parent set, consecutive nonces.
    let sign_sibling = |inner: &GroupOp, nonce: u64| {
        SignedNamespaceOp::sign(
            &signer_sk,
            namespace_id,
            vec![],
            [0u8; 32],
            nonce,
            NamespaceOp::Group {
                group_id: namespace_id,
                key_id,
                encrypted: GroupKeyring::encrypt_op(&group_key, inner).unwrap(),
                key_rotation: None,
            },
        )
        .unwrap()
    };

    let inner_low = make_policy_op("mLow");
    let inner_high = make_policy_op("mHigh");
    let op_low = sign_sibling(&inner_low, 1);
    let op_high = sign_sibling(&inner_high, 2);

    // The HIGHER-nonce sibling is delivered first.
    gov.apply_signed_op(&op_high).expect("apply nonce 2");
    assert_eq!(
        read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(),
        1,
        "first sibling logged"
    );
    let window = crate::load_nonce_window(&store, &ns_gid, &signer_pk).unwrap();
    assert_eq!(window.floor(), 0, "floor held behind the missing nonce 1");
    assert!(window.contains(2), "nonce 2 parked above the floor");

    // The LOWER-nonce sibling is delivered second. The old guard would have
    // dropped it as `1 <= last(=2)`.
    gov.apply_signed_op(&op_low).expect("apply nonce 1");
    assert_eq!(
        read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(),
        2,
        "lower-nonce sibling must NOT be dropped (the #2516 bug)"
    );
    let window = crate::load_nonce_window(&store, &ns_gid, &signer_pk).unwrap();
    assert_eq!(window.floor(), 2, "gap closed once both siblings applied");

    // Replays of both siblings are deduped — no extra log entries.
    gov.apply_signed_op(&op_high).expect("replay nonce 2");
    gov.apply_signed_op(&op_low).expect("replay nonce 1");
    assert_eq!(
        read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(),
        2,
        "replayed siblings must be deduped"
    );
}

#[test]
fn replica_stale_head_does_not_overwrite_orphan_entry() {
    use calimero_context_client::local_governance::{
        NamespaceOp, SignedGroupOp, SignedNamespaceOp, SIGNED_GROUP_OP_SCHEMA_VERSION,
    };
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    let signer_sk = PrivateKey::random(&mut rng);
    let signer_pk = signer_sk.public_key();

    let namespace_id = [0xC5u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(signer_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &signer_pk, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5Bu8; 32];
    let key_id = GroupKeyring::new(&store, ns_gid)
        .store_key(&group_key)
        .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);

    let group_op_content_hash = |ns_op: &SignedNamespaceOp, inner: &GroupOp| -> [u8; 32] {
        SignedGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id: namespace_id,
            parent_op_hashes: ns_op.parent_op_hashes.clone(),
            state_hash: ns_op.state_hash,
            signer: ns_op.signer,
            nonce: ns_op.nonce,
            op: inner.clone(),
            signature: ns_op.signature,
        }
        .content_hash()
        .unwrap()
    };

    let make_policy_op = |mrtd: &str| GroupOp::TeeAdmissionPolicySet {
        allowed_mrtd: vec![mrtd.to_owned()],
        allowed_rtmr0: vec![],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec!["ok".to_owned()],
        accept_mock: true,
    };

    // ---- Op A (nonce 1): entry + head land at seq 1. ----
    let inner_a = make_policy_op("mA");
    let op_a = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &inner_a).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    let hash_a = group_op_content_hash(&op_a, &inner_a);
    gov.apply_signed_op(&op_a).expect("apply op A");
    assert_eq!(read_op_log_after(&store, &ns_gid, 0, 10).unwrap().len(), 1);
    assert_eq!(get_op_head(&store, &ns_gid).unwrap().unwrap().sequence, 1);

    // ---- Crash simulation: the orphan condition. Entry at seq 1 survives,
    // but the head is rewound to seq 0 as if the head `put` never committed. ----
    set_op_head(&store, &ns_gid, 0, vec![]).unwrap();

    // ---- Op B (nonce 2), different content. With a stale-head-derived
    // sequence it would reuse seq 1 and clobber A. ----
    let inner_b = make_policy_op("mB");
    let op_b = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: namespace_id,
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &inner_b).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    let hash_b = group_op_content_hash(&op_b, &inner_b);
    gov.apply_signed_op(&op_b).expect("apply op B");

    // The decisive assertions: A's orphan entry is preserved (still at seq 1),
    // B appended at seq 2, and both content hashes are present.
    let entries = read_op_log_after(&store, &ns_gid, 0, 10).unwrap();
    assert_eq!(
        entries.len(),
        2,
        "op B must NOT overwrite the orphan entry left by the simulated crash"
    );
    assert_eq!(entries[0].0, 1, "op A stays at seq 1");
    assert_eq!(entries[1].0, 2, "op B appended at seq 2, above the orphan");
    assert!(
        super::super::local_state::op_log_contains_content_hash(&store, &ns_gid, &hash_a).unwrap(),
        "op A's content must survive"
    );
    assert!(
        super::super::local_state::op_log_contains_content_hash(&store, &ns_gid, &hash_b).unwrap(),
        "op B's content must be logged"
    );
}

#[test]
fn recursive_remove_cascades_to_all_descendants() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let grandchild = ContextGroupId::from([0xE2; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    // Build hierarchy
    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();

    // Add admin + member to all groups
    for gid in [&root, &child, &grandchild] {
        MetaRepository::new(&store).save(gid, &test_meta()).unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &admin, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &member, GroupMemberRole::Member)
            .unwrap();
    }

    // Verify member exists everywhere
    assert!(MembershipRepository::new(&store)
        .is_member(&root, &member)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &member)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&grandchild, &member)
        .unwrap());

    // Remove from root — should cascade to child and grandchild
    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&root, &member)
        .unwrap();
    assert_eq!(removed_from.len(), 3, "should be removed from all 3 groups");

    assert!(!MembershipRepository::new(&store)
        .is_member(&root, &member)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_member(&child, &member)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_member(&grandchild, &member)
        .unwrap());

    // Admin should be unaffected
    assert!(MembershipRepository::new(&store)
        .is_member(&root, &admin)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &admin)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&grandchild, &admin)
        .unwrap());
}

#[test]
fn recursive_remove_from_child_does_not_affect_parent() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let grandchild = ContextGroupId::from([0xE2; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();

    for gid in [&root, &child, &grandchild] {
        MetaRepository::new(&store).save(gid, &test_meta()).unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &admin, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &member, GroupMemberRole::Member)
            .unwrap();
    }

    // Remove from child only — should cascade to grandchild but NOT root
    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&child, &member)
        .unwrap();
    assert_eq!(removed_from.len(), 2, "removed from child + grandchild");

    // Root membership should be unaffected
    assert!(
        MembershipRepository::new(&store)
            .is_member(&root, &member)
            .unwrap(),
        "root membership must survive child removal"
    );
    assert!(!MembershipRepository::new(&store)
        .is_member(&child, &member)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_member(&grandchild, &member)
        .unwrap());
}

#[test]
fn recursive_remove_member_not_in_some_descendants() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();

    for gid in [&root, &child] {
        MetaRepository::new(&store).save(gid, &test_meta()).unwrap();
        MembershipRepository::new(&store)
            .add_member(gid, &admin, GroupMemberRole::Admin)
            .unwrap();
    }
    // Member only in root, not in child
    MembershipRepository::new(&store)
        .add_member(&root, &member, GroupMemberRole::Member)
        .unwrap();

    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&root, &member)
        .unwrap();
    assert_eq!(
        removed_from.len(),
        1,
        "only removed from root where member existed"
    );
    assert!(!MembershipRepository::new(&store)
        .is_member(&root, &member)
        .unwrap());
}

#[test]
fn recursive_remove_skips_inherited_only_members() {
    // Regression for cursor[bot] comment on PR #2261: before the fix,
    // `recursive_remove_member` used `check_group_membership` which now
    // returns true for inherited members of `Open` subgroups. Calling
    // `remove_group_member` on such a group would be a no-op (no direct
    // row to delete) but the group would be added to the `removed_from`
    // list anyway -- the admin would believe they revoked access while
    // the user kept their inherited membership.
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let open_child = ContextGroupId::from([0xF1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    NamespaceRepository::new(&store)
        .nest(&root, &open_child)
        .unwrap();
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&open_child, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&root, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&open_child, &admin, GroupMemberRole::Admin)
        .unwrap();

    // Direct member of `root` only; inherited into `open_child` via the
    // CAN_JOIN_OPEN_SUBGROUPS cap + Open visibility.
    MembershipRepository::new(&store)
        .add_member(&root, &member, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&root, &member, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&open_child, VisibilityMode::Open)
        .unwrap();

    // Sanity: inherited path works pre-removal.
    assert!(MembershipRepository::new(&store)
        .is_member(&open_child, &member)
        .unwrap());

    // Recursive remove anchored at `open_child` must NOT report it as
    // removed-from -- the member has no direct row there.
    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&open_child, &member)
        .unwrap();
    assert!(
        removed_from.is_empty(),
        "inherited-only member should not be reported as removed (got {removed_from:?})"
    );

    // The member is still inherited because root membership + cap + Open
    // child are all unchanged.
    assert!(MembershipRepository::new(&store)
        .is_member(&open_child, &member)
        .unwrap());

    // To actually revoke, the admin removes them from the anchor (root).
    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&root, &member)
        .unwrap();
    assert_eq!(removed_from, vec![root]);
    assert!(!MembershipRepository::new(&store)
        .is_member(&open_child, &member)
        .unwrap());
}

#[test]
fn recursive_remove_nonexistent_member_returns_empty() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let stranger = PublicKey::from([0x99; 32]);

    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&root, &admin, GroupMemberRole::Admin)
        .unwrap();

    let removed_from = NamespaceRepository::new(&store)
        .recursive_remove_member(&root, &stranger)
        .unwrap();
    assert!(removed_from.is_empty(), "nothing to remove");
}

#[test]
fn collect_visible_descendant_groups_walls_at_restricted_subgroups_inviter_not_in() {
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let ns = ContextGroupId::from([0x10; 32]); // namespace root
    let open_sub = ContextGroupId::from([0x11; 32]); // Open child of ns
    let owner_priv = ContextGroupId::from([0x12; 32]); // a member's Restricted DM, inviter NOT in
    let behind_wall = ContextGroupId::from([0x13; 32]); // Open, but under owner_priv -> unreachable
    let inviter_priv = ContextGroupId::from([0x14; 32]); // Restricted, inviter IS a direct member

    NamespaceRepository::new(&store)
        .nest(&ns, &open_sub)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns, &owner_priv)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&owner_priv, &behind_wall)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns, &inviter_priv)
        .unwrap();
    for gid in [&ns, &open_sub, &owner_priv, &behind_wall, &inviter_priv] {
        MetaRepository::new(&store).save(gid, &test_meta()).unwrap();
    }

    // The recursive inviter is an admin of the namespace root.
    let inviter_sk = PrivateKey::random(&mut OsRng);
    let inviter_pk = inviter_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&ns, &inviter_pk, GroupMemberRole::Admin)
        .unwrap();

    // open_sub is Open -> the namespace admin inherits in.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&open_sub, VisibilityMode::Open)
        .unwrap();

    // owner_priv is a different member's private DM: Restricted, inviter never added.
    let owner_pk = PrivateKey::random(&mut OsRng).public_key();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&owner_priv, VisibilityMode::Restricted)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&owner_priv, &owner_pk, GroupMemberRole::Admin)
        .unwrap();
    // ...even though there is an Open subgroup *under* it: the wall hides the whole subtree.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&behind_wall, VisibilityMode::Open)
        .unwrap();

    // inviter_priv is Restricted, but the inviter has a direct member row -> visible.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&inviter_priv, VisibilityMode::Restricted)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&inviter_priv, &inviter_pk, GroupMemberRole::Member)
        .unwrap();

    // Sanity on the membership facts the walk depends on.
    assert!(MembershipRepository::new(&store)
        .is_member(&open_sub, &inviter_pk)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_member(&owner_priv, &inviter_pk)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&inviter_priv, &inviter_pk)
        .unwrap());

    let visible = NamespaceRepository::new(&store)
        .collect_visible_descendants(&ns, &inviter_pk)
        .unwrap();
    assert!(visible.contains(&open_sub));
    assert!(visible.contains(&inviter_priv));
    assert!(
        !visible.contains(&owner_priv),
        "a Restricted subgroup the inviter isn't in must be walled (got {visible:?})"
    );
    assert!(
        !visible.contains(&behind_wall),
        "the subtree behind a wall is unreachable too (got {visible:?})"
    );
    assert_eq!(
        visible.len(),
        2,
        "exactly open_sub + inviter_priv should be visible, got {visible:?}"
    );

    // The unfiltered walk still sees everything — cascade-delete / recursive-remove
    // rely on `collect_descendant_groups` keeping that whole-subtree behavior.
    let all = NamespaceRepository::new(&store)
        .collect_descendants(&ns)
        .unwrap();
    for gid in [&open_sub, &owner_priv, &behind_wall, &inviter_priv] {
        assert!(
            all.contains(gid),
            "{gid:?} missing from unfiltered descendants"
        );
    }
}

#[test]
fn create_recursive_invitations_omits_private_subgroups_inviter_not_in() {
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let ns = ContextGroupId::from([0x20; 32]);
    let open_sub = ContextGroupId::from([0x21; 32]);
    let owner_priv = ContextGroupId::from([0x22; 32]);

    NamespaceRepository::new(&store)
        .nest(&ns, &open_sub)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns, &owner_priv)
        .unwrap();
    for gid in [&ns, &open_sub, &owner_priv] {
        MetaRepository::new(&store).save(gid, &test_meta()).unwrap();
    }

    let inviter_sk = PrivateKey::random(&mut OsRng);
    let inviter_pk = inviter_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&ns, &inviter_pk, GroupMemberRole::Admin)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&open_sub, VisibilityMode::Open)
        .unwrap();

    let owner_pk = PrivateKey::random(&mut OsRng).public_key();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&owner_priv, VisibilityMode::Restricted)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&owner_priv, &owner_pk, GroupMemberRole::Admin)
        .unwrap();

    let invitations = NamespaceRepository::new(&store)
        .create_recursive_invitations(&ns, &inviter_sk, 3600, 1)
        .unwrap();
    let invited: Vec<ContextGroupId> = invitations.iter().map(|(gid, _)| *gid).collect();

    assert!(
        invited.contains(&ns),
        "the namespace itself is always invited"
    );
    assert!(
        invited.contains(&open_sub),
        "Open subgroups stay in the recursive set"
    );
    assert!(
        !invited.contains(&owner_priv),
        "a Restricted subgroup the inviter was never added to must not be invited into (got {invited:?})"
    );
    assert_eq!(
        invitations.len(),
        2,
        "exactly the namespace + open_sub should be invited, got {invited:?}"
    );

    // Each emitted invitation targets exactly the group it is keyed under.
    for (gid, signed) in &invitations {
        assert_eq!(signed.invitation.group_id, *gid);
    }
}

#[test]
fn governance_group_reparented_via_signed_op() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let mid_id = [0xA1u8; 32];
    let mid_gid = ContextGroupId::from(mid_id);
    let new_parent_id = [0xA2u8; 32];
    let new_parent_gid = ContextGroupId::from(new_parent_id);
    let leaf_id = [0xA3u8; 32];
    let leaf_gid = ContextGroupId::from(leaf_id);

    // Bootstrap namespace: meta + admin + namespace identity
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);

    // Create three subgroups via GroupCreated ops (atomic create+nest):
    // namespace → mid, namespace → new_parent, mid → leaf.
    for (i, (gid, parent)) in [(mid_id, ns_id), (new_parent_id, ns_id), (leaf_id, mid_id)]
        .iter()
        .enumerate()
    {
        let op = SignedNamespaceOp::sign(
            &admin_sk,
            ns_id,
            vec![],
            [0u8; 32],
            (i + 1) as u64,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id: *gid,
                parent_id: *parent,
                restricted: true,
            }),
        )
        .expect("sign create op");
        gov.apply_signed_op(&op).expect("apply create op");
    }

    assert_eq!(
        NamespaceRepository::new(&store).parent(&leaf_gid).unwrap(),
        Some(mid_gid)
    );

    // Reparent leaf from mid to new_parent.
    let reparent_op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        4,
        NamespaceOp::Root(RootOp::GroupReparented {
            child_group_id: leaf_id,
            new_parent_id,
        }),
    )
    .expect("sign reparent op");
    gov.apply_signed_op(&reparent_op)
        .expect("apply reparent op");

    assert_eq!(
        NamespaceRepository::new(&store).parent(&leaf_gid).unwrap(),
        Some(new_parent_gid)
    );
    let mid_children = NamespaceRepository::new(&store)
        .list_children(&mid_gid)
        .unwrap();
    assert!(!mid_children.contains(&leaf_gid), "leaf detached from mid");
    let new_children = NamespaceRepository::new(&store)
        .list_children(&new_parent_gid)
        .unwrap();
    assert!(
        new_children.contains(&leaf_gid),
        "leaf attached to new_parent"
    );
}

#[test]
fn governance_apply_signed_op_is_idempotent_on_replay() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xC0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: [0xC1; 32],
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign create op");
    let delta_id = op.content_hash().expect("content_hash");

    gov.apply_signed_op(&op).expect("first apply");
    assert_eq!(
        raw_namespace_dag_heads(&store, ns_id),
        vec![delta_id],
        "head set after first apply"
    );

    // Replay the exact same op — should be a no-op, not a duplicate head.
    let replay = gov.apply_signed_op(&op).expect("replay apply");
    assert!(replay.key_unwrap_failures.is_empty());
    assert_eq!(
        raw_namespace_dag_heads(&store, ns_id),
        vec![delta_id],
        "head set must stay duplicate-free after replay"
    );
}

#[test]
fn governance_rejects_non_admin_signer() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();
    let intruder_sk = PrivateKey::random(&mut rng);

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);

    // Bootstrap namespace with admin
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);

    // Non-admin tries to create a group
    let op = SignedNamespaceOp::sign(
        &intruder_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: [0xBB; 32],
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op");

    let result = gov.apply_signed_op(&op);
    assert!(result.is_err(), "non-admin signer should be rejected");
}

#[test]
fn governance_group_created_is_idempotent() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let new_group_id = [0xCC; 32];

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);

    let op1 = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op1");

    gov.apply_signed_op(&op1)
        .expect("first apply should succeed");

    // Apply same op again (different nonce but same group_id)
    let op2 = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op2");

    // Should not error — idempotent
    gov.apply_signed_op(&op2)
        .expect("duplicate GroupCreated should be idempotent");
}

/// #2771: `GroupCreated { restricted: false }` must write an Open visibility
/// key at apply time (born-Open atomic create), and `restricted: true` must
/// leave the subgroup Restricted. This is the store-level guard that the live
/// op carries visibility and that `tee_subgroup_admit` (which reads this key
/// via `is_open_chain_to_namespace`) will see Open immediately.
#[test]
fn governance_group_created_writes_birth_visibility() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;
    use crate::CapabilitiesRepository;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA1u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let open_group_id = [0xE0u8; 32];
    let restricted_group_id = [0xE1u8; 32];

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);
    let caps = CapabilitiesRepository::new(&store);

    // Born-Open: restricted = false ⇒ visibility key written as Open.
    let open_op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: open_group_id,
            parent_id: ns_id,
            restricted: false,
        }),
    )
    .expect("sign born-open op");
    gov.apply_signed_op(&open_op)
        .expect("born-open GroupCreated should apply");
    assert_eq!(
        caps.subgroup_visibility(&ContextGroupId::from(open_group_id))
            .expect("read open vis"),
        VisibilityMode::Open,
        "GroupCreated {{ restricted: false }} must write an Open visibility key"
    );

    // Born-Restricted: restricted = true ⇒ Restricted.
    let restricted_op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: restricted_group_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign born-restricted op");
    gov.apply_signed_op(&restricted_op)
        .expect("born-restricted GroupCreated should apply");
    assert_eq!(
        caps.subgroup_visibility(&ContextGroupId::from(restricted_group_id))
            .expect("read restricted vis"),
        VisibilityMode::Restricted,
        "GroupCreated {{ restricted: true }} must remain Restricted"
    );
}

/// Regression for a Cursor finding on PR #2855: birth visibility is an
/// INITIAL condition, not idempotent state. A duplicate `GroupCreated`
/// (replay — different nonce, same `group_id`) must NOT re-assert the birth
/// visibility, or it would clobber a `SubgroupVisibilitySet` flip applied in
/// the meantime. Create born-Open, flip to Restricted (the same store
/// mutation `SubgroupVisibilitySet` apply performs), then replay the SAME
/// `GroupCreated` op and assert the flip survives.
#[test]
fn governance_group_created_replay_does_not_reset_visibility() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;
    use crate::CapabilitiesRepository;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xB2u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let group_id = [0xE2u8; 32];
    let gid = ContextGroupId::from(group_id);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);
    let caps = CapabilitiesRepository::new(&store);

    // 1. Create the subgroup born-Open.
    let create_op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id,
            parent_id: ns_id,
            restricted: false,
        }),
    )
    .expect("sign create op");
    gov.apply_signed_op(&create_op)
        .expect("born-open GroupCreated should apply");
    assert_eq!(
        caps.subgroup_visibility(&gid)
            .expect("read vis after create"),
        VisibilityMode::Open,
        "precondition: subgroup is born Open"
    );

    // 2. Flip it Restricted — the same store mutation `SubgroupVisibilitySet`
    //    apply performs (see ops/group/subgroup_visibility_set.rs).
    caps.set_subgroup_visibility(&gid, VisibilityMode::Restricted)
        .expect("flip to Restricted");
    assert_eq!(
        caps.subgroup_visibility(&gid).expect("read vis after flip"),
        VisibilityMode::Restricted,
        "precondition: flip applied"
    );

    // 3. Replay the SAME GroupCreated op (different nonce, same group_id).
    let replay_op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id,
            parent_id: ns_id,
            restricted: false,
        }),
    )
    .expect("sign replay op");
    gov.apply_signed_op(&replay_op)
        .expect("GroupCreated replay should be idempotent");

    // 4. The flip must survive — the replay must NOT reset visibility to Open.
    assert_eq!(
        caps.subgroup_visibility(&gid)
            .expect("read vis after replay"),
        VisibilityMode::Restricted,
        "GroupCreated replay must not clobber a later SubgroupVisibilitySet flip"
    );
}

#[test]
fn governance_group_created_writes_parent_edge_even_when_meta_pre_populated() {
    // Regression test for Cursor Bugbot finding on PR #2200:
    // The create_group handler pre-populates GroupMeta BEFORE publishing
    // the GroupCreated op. A naive idempotency check that returns early on
    // "meta exists" would skip GroupParentRef/GroupChildIndex writes on the
    // originating node — leaving it with no parent edge while remote peers
    // correctly populate the edges. This test simulates the originator flow
    // and asserts the parent edge IS written even when meta pre-exists.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let new_group_id = [0xCCu8; 32];
    let new_gid = ContextGroupId::from(new_group_id);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    // Simulate the create_group HANDLER pre-populating meta before publishing:
    // this is the originator's flow.
    MetaRepository::new(&store)
        .save(&new_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();

    // Now apply the GroupCreated op — idempotency must NOT skip the edges.
    let gov = NamespaceGovernance::new(&store, ns_id);
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op");
    gov.apply_signed_op(&op)
        .expect("apply GroupCreated on originator");

    // Parent edge must exist (the bug was that it wouldn't).
    assert_eq!(
        NamespaceRepository::new(&store).parent(&new_gid).unwrap(),
        Some(ns_gid),
        "originator must have parent edge after GroupCreated even though meta was pre-populated"
    );
    // Child index on namespace must include the new group.
    let children = NamespaceRepository::new(&store)
        .list_children(&ns_gid)
        .unwrap();
    assert!(
        children.contains(&new_gid),
        "namespace's child index must include new group"
    );
}

#[test]
fn execute_group_created_rejects_self_parent() {
    // Regression test for the E2E regression where create_group.rs defaulted
    // parent_id to group_id for namespace-root creation, producing a
    // self-parent edge that made resolve_namespace cycle. The op handler
    // now rejects self-parent explicitly; the create_group handler skips
    // emitting GroupCreated entirely for root creation.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    // Attempt to emit GroupCreated with group_id == parent_id (the bug).
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: ns_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op");

    let gov = NamespaceGovernance::new(&store, ns_id);
    let err = gov.apply_signed_op(&op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<NamespaceError>(),
            Some(NamespaceError::SelfParentEdge)
        ),
        "expected self-parent rejection, got: {err}"
    );
}

#[test]
fn execute_group_created_inherits_app_key_and_application_from_parent() {
    // Regression guard: a freshly-applied `GroupCreated` op must seed the
    // subgroup's `GroupMetaValue` with the parent's `app_key` (not zero).
    // The cascade predicate is `from_app_key == descendant.app_key`, so a
    // zero-init here would make every cascade walk silently skip
    // remote-created subgroups even though the originator's local copy
    // had the right key (originator pre-populates meta with the derived
    // blob-id-based key; peers' copies come from this apply handler).
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xE0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);

    // `sample_meta_with_admin` pins app_key = [0xBB; 32] and
    // target_application_id = [0xCC; 32].
    let parent_meta = sample_meta_with_admin(admin_pk);
    MetaRepository::new(&store)
        .save(&ns_gid, &parent_meta)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let sub_id = [0xE1u8; 32];
    let sub_gid = ContextGroupId::from(sub_id);

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sub_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .expect("sign op");

    let gov = NamespaceGovernance::new(&store, ns_id);
    gov.apply_signed_op(&op).expect("apply group_created");

    let sub_meta = MetaRepository::new(&store)
        .load(&sub_gid)
        .expect("load sub meta")
        .expect("sub meta written");

    assert_eq!(
        sub_meta.app_key, parent_meta.app_key,
        "subgroup must inherit parent's app_key so cascade predicate matches"
    );
    assert_eq!(
        sub_meta.target_application_id, parent_meta.target_application_id,
        "subgroup must inherit parent's target_application_id"
    );
}

#[test]
fn execute_group_deleted_subset_check_allows_partial_retry() {
    // Regression test for meroreviewer bugbot finding #3124131096 on PR #2200:
    // If a previous apply of GroupDeleted crashes mid-cascade, the local
    // subtree is a partial-delete state — smaller than the payload. An
    // exact-equality determinism check would permanently reject the retry,
    // stalling the namespace DAG. The subset check lets the retry resume.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    // Build: namespace → A → B (two-level subtree).
    let a_id = [0xAAu8; 32];
    let b_id = [0xBBu8; 32];
    let a_gid = ContextGroupId::from(a_id);
    let b_gid = ContextGroupId::from(b_id);
    MetaRepository::new(&store)
        .save(&a_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MetaRepository::new(&store)
        .save(&b_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &a_gid)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&a_gid, &b_gid)
        .unwrap();

    // Pre-compute the ORIGINAL payload (the "full" cascade).
    let original_payload = NamespaceRepository::new(&store)
        .collect_subtree_for_cascade(&a_gid)
        .unwrap();
    let cascade_group_ids: Vec<[u8; 32]> = original_payload
        .descendant_groups
        .iter()
        .map(|g| g.to_bytes())
        .collect();
    assert_eq!(cascade_group_ids.len(), 1, "B is the only descendant of A");

    // Simulate a partial-delete crash by deleting B's meta + parent edge
    // (i.e., B is "already gone" from a hypothetical first apply attempt).
    MetaRepository::new(&store).delete(&b_gid).unwrap();
    {
        use calimero_store::key::{GroupChildIndex, GroupParentRef};
        let mut h = store.handle();
        h.delete(&GroupParentRef::new(b_id)).unwrap();
        h.delete(&GroupChildIndex::new(a_id, b_id)).unwrap();
    }

    // Now the retry: cascade op has payload [B], but local subtree of A is
    // empty (B already gone). Subset check: local {} ⊆ payload {B} ✓ → apply
    // proceeds. Exact-match check would have rejected here — that's the bug.
    let gov = NamespaceGovernance::new(&store, ns_id);
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupDeleted {
            root_group_id: a_id,
            cascade_group_ids,
            cascade_context_ids: vec![],
        }),
    )
    .expect("sign op");
    gov.apply_signed_op(&op)
        .expect("retry after partial-delete should succeed — not stall the DAG");

    // A must now be gone (retry completed the deletion).
    assert!(
        MetaRepository::new(&store).load(&a_gid).unwrap().is_none(),
        "cascade retry must complete the root deletion"
    );
}

#[test]
fn min_acks_after_local_mutation_uses_publish_time_subscribers() {
    let min_acks = super::governance::min_acks_after_local_mutation(1, 0);

    assert_eq!(
        min_acks, 0,
        "subscriber departure after the readiness gate must use min_acks=0 to avoid NoAckReceived after local DAG mutation"
    );
}

#[test]
fn is_descendant_of_direct_child() {
    let store = test_store();
    let parent = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    MetaRepository::new(&store)
        .save(&parent, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&parent, &child)
        .unwrap();

    assert!(NamespaceRepository::new(&store)
        .is_descendant_of(&child, &parent)
        .unwrap());
    assert!(!NamespaceRepository::new(&store)
        .is_descendant_of(&parent, &child)
        .unwrap());
}

#[test]
fn is_descendant_of_grandchild() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let mid = ContextGroupId::from([0xD1; 32]);
    let leaf = ContextGroupId::from([0xD2; 32]);
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&mid, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&leaf, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store).nest(&root, &mid).unwrap();
    NamespaceRepository::new(&store).nest(&mid, &leaf).unwrap();

    assert!(NamespaceRepository::new(&store)
        .is_descendant_of(&leaf, &root)
        .unwrap());
    assert!(NamespaceRepository::new(&store)
        .is_descendant_of(&leaf, &mid)
        .unwrap());
    assert!(!NamespaceRepository::new(&store)
        .is_descendant_of(&root, &leaf)
        .unwrap());
}

#[test]
fn is_descendant_of_unrelated() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    let b = ContextGroupId::from([0xD1; 32]);
    assert!(!NamespaceRepository::new(&store)
        .is_descendant_of(&a, &b)
        .unwrap());
    assert!(!NamespaceRepository::new(&store)
        .is_descendant_of(&b, &a)
        .unwrap());
}

#[test]
fn is_descendant_of_self_is_false() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    assert!(!NamespaceRepository::new(&store)
        .is_descendant_of(&a, &a)
        .unwrap());
}

#[test]
fn reparent_group_swaps_parent_edge() {
    let store = test_store();
    let old_parent = ContextGroupId::from([0xE0; 32]);
    let new_parent = ContextGroupId::from([0xE1; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    MetaRepository::new(&store)
        .save(&old_parent, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&new_parent, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&old_parent, &child)
        .unwrap();

    NamespaceRepository::new(&store)
        .reparent(&child, &new_parent)
        .unwrap();

    assert_eq!(
        NamespaceRepository::new(&store).parent(&child).unwrap(),
        Some(new_parent)
    );
    let old_children = NamespaceRepository::new(&store)
        .list_children(&old_parent)
        .unwrap();
    assert!(!old_children.contains(&child));
    let new_children = NamespaceRepository::new(&store)
        .list_children(&new_parent)
        .unwrap();
    assert!(new_children.contains(&child));
}

#[test]
fn reparent_group_idempotent_on_same_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    MetaRepository::new(&store)
        .save(&parent, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&parent, &child)
        .unwrap();

    NamespaceRepository::new(&store)
        .reparent(&child, &parent)
        .unwrap();
    assert_eq!(
        NamespaceRepository::new(&store).parent(&child).unwrap(),
        Some(parent)
    );
    assert_eq!(
        NamespaceRepository::new(&store)
            .list_children(&parent)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reparent_group_rejects_cycle() {
    let store = test_store();
    let a = ContextGroupId::from([0xE0; 32]);
    let b = ContextGroupId::from([0xE1; 32]);
    MetaRepository::new(&store).save(&a, &test_meta()).unwrap();
    MetaRepository::new(&store).save(&b, &test_meta()).unwrap();
    NamespaceRepository::new(&store).nest(&a, &b).unwrap();

    let err = NamespaceRepository::new(&store)
        .reparent(&a, &b)
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<NamespaceError>(),
            Some(NamespaceError::ReparentCycle { .. } | NamespaceError::RootHasNoParent(_))
        ),
        "expected cycle or root error, got: {err}"
    );
}

#[test]
fn reparent_group_rejects_root() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let other = ContextGroupId::from([0xE1; 32]);
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&other, &test_meta())
        .unwrap();

    let err = NamespaceRepository::new(&store)
        .reparent(&root, &other)
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<NamespaceError>(),
            Some(NamespaceError::RootHasNoParent(_))
        ),
        "expected root rejection, got: {err}"
    );
}

#[test]
fn reparent_group_rejects_nonexistent_new_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    let phantom = ContextGroupId::from([0xFF; 32]);
    MetaRepository::new(&store)
        .save(&parent, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&parent, &child)
        .unwrap();

    let err = NamespaceRepository::new(&store)
        .reparent(&child, &phantom)
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<NamespaceError>(),
            Some(NamespaceError::ReparentTargetMissing(_))
        ),
        "expected new-parent-not-found, got: {err}"
    );
}

#[test]
fn collect_subtree_for_cascade_empty_subtree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();

    let payload = NamespaceRepository::new(&store)
        .collect_subtree_for_cascade(&root)
        .unwrap();
    assert!(payload.descendant_groups.is_empty());
    assert!(payload.contexts.is_empty());
}

#[test]
fn collect_subtree_for_cascade_two_level_tree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let mid = ContextGroupId::from([0xF1; 32]);
    let leaf = ContextGroupId::from([0xF2; 32]);
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&mid, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&leaf, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store).nest(&root, &mid).unwrap();
    NamespaceRepository::new(&store).nest(&mid, &leaf).unwrap();

    let payload = NamespaceRepository::new(&store)
        .collect_subtree_for_cascade(&root)
        .unwrap();
    assert_eq!(payload.descendant_groups.len(), 2);
    let leaf_pos = payload
        .descendant_groups
        .iter()
        .position(|g| g == &leaf)
        .unwrap();
    let mid_pos = payload
        .descendant_groups
        .iter()
        .position(|g| g == &mid)
        .unwrap();
    assert!(
        leaf_pos < mid_pos,
        "expected children-first; leaf={leaf_pos} mid={mid_pos}"
    );
}

#[test]
fn collect_subtree_for_cascade_includes_contexts_from_all_groups() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();

    let ctx_root = ContextId::from([0x10; 32]);
    let ctx_child = ContextId::from([0x11; 32]);
    register_context_in_group(&store, &root, &ctx_root).unwrap();
    register_context_in_group(&store, &child, &ctx_child).unwrap();

    let payload = NamespaceRepository::new(&store)
        .collect_subtree_for_cascade(&root)
        .unwrap();
    assert!(payload.contexts.contains(&ctx_root));
    assert!(payload.contexts.contains(&ctx_child));
    assert_eq!(payload.contexts.len(), 2);
}

#[test]
fn governance_group_created_honors_can_create_subgroup_at_root_only() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    // Never added to the namespace — not a member, no capability row.
    let stranger_sk = PrivateKey::random(&mut rng);

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &member_pk, GroupMemberRole::Member)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);
    // `nonce` is informational only — `apply_signed_op` advances the DAG head
    // from `read_head_record().next_nonce`, not from `op.nonce`, and a rejected
    // op never advances the head or gets stored. Distinct `group_id`s already
    // give each op a distinct content hash; we pass increasing values for
    // readability. (Same as the `governance_group_deleted_*` test.)
    let create = |sk: &PrivateKey, group_id: [u8; 32], parent_id: [u8; 32], nonce: u64| {
        SignedNamespaceOp::sign(
            sk,
            ns_id,
            vec![],
            [0u8; 32],
            nonce,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id,
                parent_id,
                restricted: true,
            }),
        )
        .unwrap()
    };

    let chan = [0xB1u8; 32];

    // A total stranger (not a namespace member, no capability row) cannot
    // create a subgroup — rejected by the apply-side authorization check.
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&ns_gid, &stranger_sk.public_key())
            .unwrap(),
        "precondition: the stranger must not be enrolled in the namespace"
    );
    let err = gov
        .apply_signed_op(&create(&stranger_sk, chan, ns_id, 1))
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::GroupCreatedRejected { .. })
        ) || matches!(
            err.downcast_ref::<NamespaceError>(),
            Some(NamespaceError::SelfParentEdge)
        ),
        "stranger should be rejected by the authorization check, got: {err}"
    );
    assert!(MetaRepository::new(&store)
        .load(&ContextGroupId::from(chan))
        .unwrap()
        .is_none());

    // Member without the cap cannot create a subgroup, even under the root.
    assert!(gov
        .apply_signed_op(&create(&member_sk, chan, ns_id, 2))
        .is_err());
    assert!(MetaRepository::new(&store)
        .load(&ContextGroupId::from(chan))
        .unwrap()
        .is_none());

    // Granting CAN_CREATE_SUBGROUP at the namespace root lets them create one
    // directly under the root, and they become its owner.
    CapabilitiesRepository::new(&store)
        .set_member_capability(&ns_gid, &member_pk, MemberCapabilities::CAN_CREATE_SUBGROUP)
        .unwrap();
    gov.apply_signed_op(&create(&member_sk, chan, ns_id, 3))
        .expect("member with CAN_CREATE_SUBGROUP creates a subgroup under the root");
    assert_eq!(
        MetaRepository::new(&store)
            .load(&ContextGroupId::from(chan))
            .unwrap()
            .unwrap()
            .owner_identity,
        member_pk,
        "creator owns the new subgroup"
    );
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&ContextGroupId::from(chan), &member_pk)
            .unwrap(),
        "creator is added as an admin of the new subgroup"
    );

    // But the capability is scoped to root-level subgroups: the member cannot
    // create a nested subgroup under another subgroup.
    let nested_parent = [0xB2u8; 32];
    gov.apply_signed_op(&create(&admin_sk, nested_parent, ns_id, 4))
        .expect("admin creates an intermediate subgroup");
    let grandchild = [0xB3u8; 32];
    assert!(
        gov.apply_signed_op(&create(&member_sk, grandchild, nested_parent, 5))
            .is_err(),
        "CAN_CREATE_SUBGROUP is honored only directly under the namespace root"
    );

    // A namespace admin is still allowed at any depth.
    gov.apply_signed_op(&create(&admin_sk, grandchild, nested_parent, 6))
        .expect("namespace admin may create nested subgroups");
}

#[test]
fn governance_group_deleted_owner_admin_or_cap_only() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();
    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    let stranger_sk = PrivateKey::random(&mut rng);
    // A namespace member who is neither the subgroup owner, a namespace admin,
    // nor a CAN_DELETE_SUBGROUP holder — a distinct case from a total stranger.
    let plain_member_sk = PrivateKey::random(&mut rng);
    let plain_member_pk = plain_member_sk.public_key();
    let janitor_sk = PrivateKey::random(&mut rng);
    let janitor_pk = janitor_sk.public_key();

    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    // `owner_pk` is enrolled as an ordinary namespace member — that mirrors the
    // real model (a subgroup owner got there by being a namespace member and
    // creating it; `leave_namespace` refuses an owner via `MustTransferOwnership`,
    // so an owner is always a current member). It holds no caps and no admin
    // role at the namespace level, so it can only delete via the owner path.
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &owner_pk, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &plain_member_pk, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &janitor_pk, GroupMemberRole::Member)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    // Three leaf subgroups under the root, all owned by `owner_pk`.
    let mk_subgroup = |tag: u8| {
        let id = [tag; 32];
        let gid = ContextGroupId::from(id);
        MetaRepository::new(&store)
            .save(&gid, &sample_meta_with_admin(owner_pk))
            .unwrap();
        NamespaceRepository::new(&store)
            .nest(&ns_gid, &gid)
            .unwrap();
        (id, gid)
    };
    let (s1, s1_gid) = mk_subgroup(0xC1);
    let (s2, s2_gid) = mk_subgroup(0xC2);
    let (s3, s3_gid) = mk_subgroup(0xC3);

    let gov = NamespaceGovernance::new(&store, ns_id);
    // `nonce` here is informational only — `apply_signed_op` advances the DAG
    // head from `read_head_record().next_nonce`, not from `op.nonce`; distinct
    // `root_group_id`s already give each op a distinct content hash. We still
    // pass monotonically increasing values for readability.
    let del = |sk: &PrivateKey, root_group_id: [u8; 32], nonce: u64| {
        SignedNamespaceOp::sign(
            sk,
            ns_id,
            vec![],
            [0u8; 32],
            nonce,
            NamespaceOp::Root(RootOp::GroupDeleted {
                root_group_id,
                cascade_group_ids: vec![],
                cascade_context_ids: vec![],
            }),
        )
        .unwrap()
    };

    // A total stranger (not even a namespace member) is rejected — and we pin
    // that it's the *authorization* check rejecting it (not some other error
    // path): signature verification passes for any valid key, so the op
    // reaches `execute_group_deleted` and fails the owner/admin/cap gate.
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&ns_gid, &stranger_sk.public_key())
            .unwrap(),
        "precondition: the stranger must not be enrolled in the namespace"
    );
    let err = gov.apply_signed_op(&del(&stranger_sk, s1, 1)).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::GroupDeletedRejected(
                GroupDeletedRejection::Unauthorized { .. }
            ))
        ),
        "stranger should be rejected by the authorization check, got: {err}"
    );
    assert!(MetaRepository::new(&store).load(&s1_gid).unwrap().is_some());

    // A plain namespace member (no CAN_DELETE_SUBGROUP, not the owner, not an
    // admin) is also rejected — the distinct "member but unauthorized" case,
    // again by the authorization check.
    let err = gov
        .apply_signed_op(&del(&plain_member_sk, s1, 2))
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::GroupDeletedRejected(
                GroupDeletedRejection::Unauthorized { .. }
            ))
        ),
        "plain member should be rejected by the authorization check, got: {err}"
    );
    assert!(MetaRepository::new(&store).load(&s1_gid).unwrap().is_some());

    // The subgroup's owner can cascade-delete it.
    gov.apply_signed_op(&del(&owner_sk, s1, 3))
        .expect("subgroup owner can delete it");
    assert!(MetaRepository::new(&store).load(&s1_gid).unwrap().is_none());

    // Re-applying the same GroupDeleted after the root meta is gone (the
    // crash-recovery shape: cascade finished, DAG head not yet advanced) must
    // be an idempotent no-op, even though the signer here (`owner_pk`) is not
    // a namespace admin and holds no CAN_DELETE_SUBGROUP — the auth check is
    // skipped when the root meta is absent.
    gov.apply_signed_op(&del(&owner_sk, s1, 6))
        .expect("re-apply of GroupDeleted after the root meta is gone is an idempotent no-op");
    assert!(MetaRepository::new(&store).load(&s1_gid).unwrap().is_none());

    // A namespace admin can delete a subgroup they don't own (moderation).
    gov.apply_signed_op(&del(&admin_sk, s2, 4))
        .expect("namespace admin can delete any subgroup");
    assert!(MetaRepository::new(&store).load(&s2_gid).unwrap().is_none());

    // A namespace member holding CAN_DELETE_SUBGROUP can delete a subgroup.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &ns_gid,
            &janitor_pk,
            MemberCapabilities::CAN_DELETE_SUBGROUP,
        )
        .unwrap();
    gov.apply_signed_op(&del(&janitor_sk, s3, 5))
        .expect("CAN_DELETE_SUBGROUP holder can delete a subgroup");
    assert!(MetaRepository::new(&store).load(&s3_gid).unwrap().is_none());
}

/// Shared setup for the `state_hash` apply-path tests below. Builds a
/// namespace with a signer admin and a wrapped subgroup that has its own
/// meta + members + group key, ready for `NamespaceOp::Group` ops.
fn setup_state_hash_test_fixture() -> (
    Store,
    PrivateKey,
    [u8; 32],
    ContextGroupId,
    [u8; 32],
    [u8; 32],
) {
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;

    let signer_sk = PrivateKey::random(&mut rng);
    let signer_pk = signer_sk.public_key();

    let namespace_id = [0xE0u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(signer_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &signer_pk, GroupMemberRole::Admin)
        .unwrap();

    let group_id_arr = [0xE1u8; 32];
    let group_gid = ContextGroupId::from(group_id_arr);
    MetaRepository::new(&store)
        .save(&group_gid, &sample_meta_with_admin(signer_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&group_gid, &signer_pk, GroupMemberRole::Admin)
        .unwrap();

    let group_key = [0x6Au8; 32];
    let key_id = GroupKeyring::new(&store, group_gid)
        .store_key(&group_key)
        .unwrap();

    (store, signer_sk, namespace_id, group_gid, group_key, key_id)
}

#[test]
fn namespace_group_op_zero_state_hash_bypasses_check() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, group_gid, group_key, key_id) =
        setup_state_hash_test_fixture();

    // Mutate the wrapped group's state AFTER picking up the (zero) state_hash
    // so the recomputed hash would differ from any non-zero claim. The zero
    // bypass must still apply cleanly — this is the documented backwards-
    // compat path for pre-fix on-disk ops.
    let other_pk = PrivateKey::random(&mut rand::rngs::OsRng).public_key();
    MembershipRepository::new(&store)
        .add_member(&group_gid, &other_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: group_gid.to_bytes(),
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op)
        .expect("zero state_hash must bypass the staleness check");
}

#[test]
fn namespace_group_op_with_current_state_hash_applies() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, group_gid, group_key, key_id) =
        setup_state_hash_test_fixture();

    let current = MetaRepository::new(&store)
        .compute_state_hash(&group_gid)
        .expect("compute state hash");

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        current,
        1,
        NamespaceOp::Group {
            group_id: group_gid.to_bytes(),
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op)
        .expect("op signed against current state must apply");
}

#[test]
fn namespace_group_op_with_stale_state_hash_applies_with_warning() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, group_gid, group_key, key_id) =
        setup_state_hash_test_fixture();

    // Snapshot the state hash, then mutate the group (a real concurrent op
    // would do this between sign and apply). The pre-mutation hash is now
    // stale relative to post-mutation state — but the namespace path
    // applies anyway and only logs a warning. Hard-rejecting would
    // over-reject the multi-node concurrent-op case; see the apply-path
    // comment in `apply_group_op_inner` and the PR #2500 caveat.
    let stale = MetaRepository::new(&store)
        .compute_state_hash(&group_gid)
        .expect("compute state hash");

    let other_pk = PrivateKey::random(&mut rand::rngs::OsRng).public_key();
    MembershipRepository::new(&store)
        .add_member(&group_gid, &other_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        stale,
        1,
        NamespaceOp::Group {
            group_id: group_gid.to_bytes(),
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op)
        .expect("stale state_hash must apply with a warning, not reject");
}

/// #2848 Part B: a group op carrying a *non-zero* `state_hash` whose target
/// subgroup has no meta row yet (e.g. an encrypted ContextRegistered buffered
/// before its GroupCreated lands and now re-driven) must BYPASS the staleness
/// check rather than blow up on `GroupNotFoundForHash`. Before the fix
/// `compute_state_hash` was called unconditionally and this returned `Err`,
/// stranding the op forever.
#[test]
fn group_op_with_stale_hash_and_absent_meta_applies() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, group_gid, group_key, key_id) =
        setup_state_hash_test_fixture();

    // Drop the subgroup meta so it is absent on apply — but keep the group key
    // so the op still decrypts and reaches the staleness check. A non-zero
    // state_hash with absent meta is exactly the buffered-before-GroupCreated
    // case.
    MetaRepository::new(&store)
        .delete(&group_gid)
        .expect("delete subgroup meta");
    assert!(
        MetaRepository::new(&store)
            .load(&group_gid)
            .unwrap()
            .is_none(),
        "precondition: subgroup meta is absent"
    );

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0x11u8; 32], // non-zero, would mismatch any recomputed hash
        1,
        NamespaceOp::Group {
            group_id: group_gid.to_bytes(),
            key_id,
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op).expect(
        "non-zero state_hash with absent subgroup meta must bypass the staleness check, \
         not error on GroupNotFoundForHash (#2848 Part B)",
    );
}

/// #2848 Part A gate: a `GroupCreated` for a subgroup whose key the local node
/// does NOT hold must be a cheap no-op on the retry path — the key-presence
/// gate short-circuits before the full op-log scan. There are no buffered ops
/// to re-drive, so apply must succeed cleanly and surface no divergence.
#[test]
fn group_created_with_no_key_skips_retry() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xF0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // Brand-new subgroup id: no GroupKeyring entry exists for it, so the
    // key-presence gate in the GroupCreated arm must skip the retry entirely.
    let new_group_id = [0xF1u8; 32];
    let new_group_gid = ContextGroupId::from(new_group_id);
    assert!(
        GroupKeyring::new(&store, new_group_gid)
            .load_current_key()
            .unwrap()
            .is_none(),
        "precondition: no key held for the new subgroup"
    );

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: ns_id,
            restricted: true,
        }),
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, ns_id);
    let result = gov
        .apply_signed_op(&op)
        .expect("GroupCreated with no held key must apply cleanly (cheap no-op retry gate)");
    assert!(
        result.divergence.is_none(),
        "no buffered ops to re-drive, so no divergence should surface"
    );
}

#[test]
fn namespace_root_op_with_current_state_hash_applies() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, _group_gid, _group_key, _key_id) =
        setup_state_hash_test_fixture();
    let ns_gid = ContextGroupId::from(namespace_id);

    let current = MetaRepository::new(&store)
        .compute_state_hash(&ns_gid)
        .expect("compute namespace state hash");

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        current,
        1,
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![1, 2, 3],
        }),
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op)
        .expect("root op signed against current namespace state must apply");
}

#[test]
fn namespace_root_op_with_stale_state_hash_applies_with_warning() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    use super::NamespaceGovernance;

    let (store, signer_sk, namespace_id, _group_gid, _group_key, _key_id) =
        setup_state_hash_test_fixture();
    let ns_gid = ContextGroupId::from(namespace_id);

    let stale = MetaRepository::new(&store)
        .compute_state_hash(&ns_gid)
        .expect("compute namespace state hash");

    // Move namespace state forward (admin adds a new namespace member),
    // simulating a concurrent op landing between sign and apply. The
    // root-op staleness check warns but does not reject — same shape as
    // the group-op branch, same convergence-under-contention rationale.
    let new_member_pk = PrivateKey::random(&mut rand::rngs::OsRng).public_key();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &new_member_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        stale,
        1,
        NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![9, 9, 9],
        }),
    )
    .unwrap();

    let gov = NamespaceGovernance::new(&store, namespace_id);
    gov.apply_signed_op(&op)
        .expect("stale root state_hash must apply with a warning, not reject");
}

// ---------------------------------------------------------------------------
// Direct (pull-based) group-key delivery (#2613)
// ---------------------------------------------------------------------------

#[test]
fn apply_received_group_key_stores_key_for_recipient() {
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;

    let namespace_id = [0xD0u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    // A non-root subgroup, so the bootstrap-admin seed path is not exercised.
    let group_id = [0xD1u8; 32];
    let group_gid = ContextGroupId::from(group_id);

    // The local node's namespace identity = the ECDH recipient.
    let recipient_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let recipient_sk = PrivateKey::from(recipient_sk_bytes);
    NamespaceRepository::new(&store)
        .store_identity(
            &ns_gid,
            &recipient_sk.public_key(),
            &recipient_sk_bytes,
            &[0u8; 32],
        )
        .unwrap();

    // A remote key-holder wraps the group key for us.
    let sender_sk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng));
    let group_key = [0x6Au8; 32];
    let envelope =
        GroupKeyring::wrap_for_member(&sender_sk, &recipient_sk.public_key(), &group_key).unwrap();
    let envelope_bytes = borsh::to_vec(&envelope).unwrap();

    // Precondition: we hold no key yet.
    assert!(GroupKeyring::new(&store, group_gid)
        .load_current_key()
        .unwrap()
        .is_none());

    apply_received_group_key(
        &store,
        namespace_id,
        group_id,
        &envelope_bytes,
        sender_sk.public_key(),
    )
    .unwrap();

    let stored = GroupKeyring::new(&store, group_gid)
        .load_current_key()
        .unwrap();
    assert_eq!(stored.map(|(_, k)| k), Some(group_key));
}

#[test]
fn apply_received_group_key_ignores_envelope_for_other_recipient() {
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;

    let namespace_id = [0xD2u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let group_id = [0xD3u8; 32];
    let group_gid = ContextGroupId::from(group_id);

    let recipient_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let recipient_sk = PrivateKey::from(recipient_sk_bytes);
    NamespaceRepository::new(&store)
        .store_identity(
            &ns_gid,
            &recipient_sk.public_key(),
            &recipient_sk_bytes,
            &[0u8; 32],
        )
        .unwrap();

    // Envelope wrapped for somebody else entirely.
    let sender_sk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng));
    let other_pk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng)).public_key();
    let group_key = [0x6Bu8; 32];
    let envelope = GroupKeyring::wrap_for_member(&sender_sk, &other_pk, &group_key).unwrap();
    let envelope_bytes = borsh::to_vec(&envelope).unwrap();

    // Not addressed to us: benign no-op, no key stored.
    let divergence = apply_received_group_key(
        &store,
        namespace_id,
        group_id,
        &envelope_bytes,
        sender_sk.public_key(),
    )
    .unwrap();
    assert!(divergence.is_none());
    assert!(GroupKeyring::new(&store, group_gid)
        .load_current_key()
        .unwrap()
        .is_none());
}

#[test]
fn groups_awaiting_key_reports_then_clears() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;
    let signer_sk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng));

    let namespace_id = [0xD4u8; 32];
    let group_id = [0xD5u8; 32];
    let group_gid = ContextGroupId::from(group_id);

    // Buffer an encrypted group op for `group_id` whose key we don't hold.
    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id,
            // Content-addressed id of the key the op is encrypted under, so
            // storing that exact key later resolves the op (awaiting clears).
            key_id: GroupKeyring::key_id_for(&[0xAA; 32]),
            encrypted: GroupKeyring::encrypt_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    NamespaceOpLogService::new(&store, namespace_id)
        .store_signed_operation(&op)
        .unwrap();

    // Keyless: the group is reported as awaiting a key.
    let awaiting = namespace_groups_awaiting_key(&store, namespace_id).unwrap();
    assert_eq!(awaiting, vec![group_id]);

    // Once the op's key is stored locally, the group drops out of the set.
    GroupKeyring::new(&store, group_gid)
        .store_key(&[0xAA; 32])
        .unwrap();
    let awaiting = namespace_groups_awaiting_key(&store, namespace_id).unwrap();
    assert!(awaiting.is_empty());
}

#[test]
fn restricted_subgroup_awaits_key_despite_holding_namespace_key() {
    // Regression for the whole group-* e2e suite going red: a joiner gets
    // the namespace (root) key with its join, then is added to a RESTRICTED
    // subgroup whose ops are encrypted under the subgroup's OWN key. Holding
    // the namespace key must NOT mask that the subgroup is still awaiting its
    // own key — otherwise the pull never requests it and the subgroup's
    // `ContextRegistered` op never decrypts ("context does not belong to any
    // group" on join_context).
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;
    let signer_sk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng));

    let namespace_id = [0xD6u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let subgroup_id = [0xD7u8; 32];

    let namespace_key = [0x11u8; 32];
    let subgroup_key = [0x22u8; 32];

    // The node holds the namespace key (delivered with its join)...
    GroupKeyring::new(&store, ns_gid)
        .store_key(&namespace_key)
        .unwrap();

    // ...but the buffered subgroup op is encrypted under the subgroup's own
    // key, which the node does NOT hold.
    let op = SignedNamespaceOp::sign(
        &signer_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: subgroup_id,
            key_id: GroupKeyring::key_id_for(&subgroup_key),
            encrypted: GroupKeyring::encrypt_op(&subgroup_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    NamespaceOpLogService::new(&store, namespace_id)
        .store_signed_operation(&op)
        .unwrap();

    // The subgroup must still be reported as awaiting its key.
    assert_eq!(
        namespace_groups_awaiting_key(&store, namespace_id).unwrap(),
        vec![subgroup_id],
        "holding the namespace key must not mask a Restricted subgroup awaiting its own key"
    );
}

#[test]
fn responder_delivery_round_trips_key_to_joiner_cross_store() {
    // The cross-node exchange minus the libp2p transport: a key-holding
    // responder store and a keyless joiner store with DISTINCT identities.
    // Proves the responder authz + ECDH wrap (`build_group_key_delivery`)
    // interoperates with the joiner unwrap + buffered-op replay
    // (`apply_received_group_key`) — i.e. the exact bytes a `GroupKeyResponse`
    // would carry on the wire actually unlock the joiner's group.
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    let mut rng = OsRng;

    let namespace_id = [0xF0u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    // A non-root subgroup so the bootstrap-admin seed path is not exercised.
    let subgroup_id = [0xF1u8; 32];
    let subgroup_gid = ContextGroupId::from(subgroup_id);
    let group_key = [0x6Cu8; 32];

    // Joiner identity: the ECDH recipient and the member the key is for.
    let joiner_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let joiner_sk = PrivateKey::from(joiner_sk_bytes);
    let joiner_pk = joiner_sk.public_key();

    // Responder identity: the namespace identity that holds and wraps the key.
    let responder_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let responder_sk = PrivateKey::from(responder_sk_bytes);
    let responder_pk = responder_sk.public_key();

    // ---- Responder store: holds the key, knows the joiner is a member. ----
    let responder_store = test_store();
    NamespaceRepository::new(&responder_store)
        .store_identity(&ns_gid, &responder_pk, &responder_sk_bytes, &[0u8; 32])
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&ns_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&subgroup_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    NamespaceRepository::new(&responder_store)
        .nest(&ns_gid, &subgroup_gid)
        .unwrap();
    MembershipRepository::new(&responder_store)
        .add_member(&subgroup_gid, &joiner_pk, GroupMemberRole::Member)
        .unwrap();
    GroupKeyring::new(&responder_store, subgroup_gid)
        .store_key(&group_key)
        .unwrap();

    // Responder builds the delivery for the joiner (the `GroupKeyResponse`).
    let (envelope_bytes, responder_identity) =
        build_group_key_delivery(&responder_store, namespace_id, subgroup_id, joiner_pk).unwrap();
    assert!(
        !envelope_bytes.is_empty(),
        "responder holding the key must deliver it to a member"
    );
    assert_eq!(responder_identity, responder_pk);

    // ---- Joiner store: keyless, with a buffered encrypted op for the group. -
    let joiner_store = test_store();
    NamespaceRepository::new(&joiner_store)
        .store_identity(&ns_gid, &joiner_pk, &joiner_sk_bytes, &[0u8; 32])
        .unwrap();
    let buffered = SignedNamespaceOp::sign(
        &responder_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: subgroup_id,
            // Content-addressed id of `group_key`, so the retry path finds
            // the delivered key by id once it is stored.
            key_id: GroupKeyring::key_id_for(&group_key),
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    NamespaceOpLogService::new(&joiner_store, namespace_id)
        .store_signed_operation(&buffered)
        .unwrap();

    // Precondition: joiner awaits the key and holds none.
    assert_eq!(
        namespace_groups_awaiting_key(&joiner_store, namespace_id).unwrap(),
        vec![subgroup_id]
    );

    // Joiner applies the responder's delivery (the wire payload).
    apply_received_group_key(
        &joiner_store,
        namespace_id,
        subgroup_id,
        &envelope_bytes,
        responder_identity,
    )
    .unwrap();

    // Postcondition: the ECDH-wrapped key round-tripped between the two
    // distinct identities, is stored under the joiner's keyring, and the
    // group no longer awaits a key (its buffered ops are now decryptable).
    assert_eq!(
        GroupKeyring::new(&joiner_store, subgroup_gid)
            .load_current_key()
            .unwrap()
            .map(|(_, k)| k),
        Some(group_key)
    );
    assert!(namespace_groups_awaiting_key(&joiner_store, namespace_id)
        .unwrap()
        .is_empty());
}

#[test]
fn responder_delivery_round_trips_key_to_read_only_tee_joiner() {
    // Same cross-store key-recovery pull as
    // `responder_delivery_round_trips_key_to_joiner_cross_store`, but the
    // joiner is a `ReadOnlyTee` member rather than a plain `Member`. The
    // responder authz is `is_member` (role-agnostic), so the per-subgroup key
    // for a Restricted subgroup must round-trip to a TEE joiner exactly as it
    // does for a regular member — this closes the runtime-unverified gap for
    // the `ReadOnlyTee` role at the crypto/authz layer.
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
    use rand::rngs::OsRng;

    let mut rng = OsRng;

    let namespace_id = [0xF2u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    // A non-root subgroup so the bootstrap-admin seed path is not exercised.
    let subgroup_id = [0xF3u8; 32];
    let subgroup_gid = ContextGroupId::from(subgroup_id);
    let group_key = [0x6Du8; 32];

    // Joiner identity: the ECDH recipient and the member the key is for.
    let joiner_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let joiner_sk = PrivateKey::from(joiner_sk_bytes);
    let joiner_pk = joiner_sk.public_key();

    // Responder identity: the namespace identity that holds and wraps the key.
    let responder_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let responder_sk = PrivateKey::from(responder_sk_bytes);
    let responder_pk = responder_sk.public_key();

    // ---- Responder store: holds the key, knows the joiner is a TEE member. ----
    let responder_store = test_store();
    NamespaceRepository::new(&responder_store)
        .store_identity(&ns_gid, &responder_pk, &responder_sk_bytes, &[0u8; 32])
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&ns_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    MetaRepository::new(&responder_store)
        .save(&subgroup_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    NamespaceRepository::new(&responder_store)
        .nest(&ns_gid, &subgroup_gid)
        .unwrap();
    MembershipRepository::new(&responder_store)
        .add_member(&subgroup_gid, &joiner_pk, GroupMemberRole::ReadOnlyTee)
        .unwrap();
    GroupKeyring::new(&responder_store, subgroup_gid)
        .store_key(&group_key)
        .unwrap();

    // Responder builds the delivery for the joiner (the `GroupKeyResponse`).
    let (envelope_bytes, responder_identity) =
        build_group_key_delivery(&responder_store, namespace_id, subgroup_id, joiner_pk).unwrap();
    assert!(
        !envelope_bytes.is_empty(),
        "responder holding the key must deliver it to a ReadOnlyTee member"
    );
    assert_eq!(responder_identity, responder_pk);

    // ---- Joiner store: keyless, with a buffered encrypted op for the group. -
    let joiner_store = test_store();
    NamespaceRepository::new(&joiner_store)
        .store_identity(&ns_gid, &joiner_pk, &joiner_sk_bytes, &[0u8; 32])
        .unwrap();
    let buffered = SignedNamespaceOp::sign(
        &responder_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: subgroup_id,
            // Content-addressed id of `group_key`, so the retry path finds
            // the delivered key by id once it is stored.
            key_id: GroupKeyring::key_id_for(&group_key),
            encrypted: GroupKeyring::encrypt_op(&group_key, &GroupOp::Noop).unwrap(),
            key_rotation: None,
        },
    )
    .unwrap();
    NamespaceOpLogService::new(&joiner_store, namespace_id)
        .store_signed_operation(&buffered)
        .unwrap();

    // Precondition: joiner awaits the key and holds none.
    assert_eq!(
        namespace_groups_awaiting_key(&joiner_store, namespace_id).unwrap(),
        vec![subgroup_id]
    );

    // Joiner applies the responder's delivery (the wire payload).
    apply_received_group_key(
        &joiner_store,
        namespace_id,
        subgroup_id,
        &envelope_bytes,
        responder_identity,
    )
    .unwrap();

    // Postcondition: the ECDH-wrapped key round-tripped to the TEE joiner, is
    // stored under the joiner's keyring, and the group no longer awaits a key.
    assert_eq!(
        GroupKeyring::new(&joiner_store, subgroup_gid)
            .load_current_key()
            .unwrap()
            .map(|(_, k)| k),
        Some(group_key)
    );
    assert!(namespace_groups_awaiting_key(&joiner_store, namespace_id)
        .unwrap()
        .is_empty());
}

#[test]
fn responder_refuses_delivery_to_non_member() {
    use rand::rngs::OsRng;

    let mut rng = OsRng;

    let namespace_id = [0xF2u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);
    let subgroup_id = [0xF3u8; 32];
    let subgroup_gid = ContextGroupId::from(subgroup_id);

    let responder_sk_bytes: [u8; 32] = rand::Rng::gen(&mut rng);
    let responder_sk = PrivateKey::from(responder_sk_bytes);
    let responder_pk = responder_sk.public_key();
    let stranger_pk = PrivateKey::from(rand::Rng::gen::<[u8; 32]>(&mut rng)).public_key();

    let store = test_store();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &responder_pk, &responder_sk_bytes, &[0u8; 32])
        .unwrap();
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup_gid, &sample_meta_with_admin(responder_pk))
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup_gid)
        .unwrap();
    GroupKeyring::new(&store, subgroup_gid)
        .store_key(&[0x6Du8; 32])
        .unwrap();

    // The requester is NOT a member of the subgroup → empty envelope: no key
    // wrapped, and no membership oracle leaked (same reply as "key not held").
    let (envelope_bytes, _responder_identity) =
        build_group_key_delivery(&store, namespace_id, subgroup_id, stranger_pk).unwrap();
    assert!(
        envelope_bytes.is_empty(),
        "responder must not wrap a key for a non-member"
    );
}

/// #2848 Part C — curative startup sweep, gov-store level.
///
/// Reconstructs the ALREADY-stranded state a node lands in when the live
/// re-drive (Parts A/B) was never available: a buffered, effect-skipped
/// `ContextRegistered` for a subgroup whose key is now HELD and whose meta is
/// now PRESENT — but with NO pending trigger (no GroupCreated/KeyDelivery
/// apply re-drove it). We construct this by buffering the op while key+meta are
/// absent, then writing the key and meta DIRECTLY (bypassing the apply path, so
/// the Part A GroupCreated re-drive never fires).
///
/// Asserts the enumerator
/// ([`namespace_groups_with_held_key_buffered_ops`]) returns exactly the
/// stranded subgroup, and that re-driving it
/// ([`redrive_buffered_ops_for_group`]) applies the buffered op (the context
/// becomes registered to the subgroup) and reports a non-`None` divergence.
///
/// Also asserts the two no-op shapes: a held-key group with nothing buffered,
/// and a no-key (deleted/never-keyed) group, are NOT enumerated.
#[test]
fn curative_sweep_redrives_stranded_context() {
    use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
    use calimero_primitives::application::ApplicationId;
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;

    // ---- Namespace root + receiver identity --------------------------------
    let ns_gid = ContextGroupId::from([0xD8u8; 32]);
    let namespace_id = ns_gid.to_bytes();

    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &owner_pk, GroupMemberRole::Admin)
        .unwrap();
    // This receiver node's namespace identity — makes the namespace a "known"
    // one for `iter_identities`/`known_namespace_identities`.
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk, &[0u8; 32])
        .unwrap();

    // ---- The stranded subgroup: pick id + mint its key ---------------------
    let sub_gid = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());
    let subgroup_key: [u8; 32] = {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        k
    };
    let key_id = GroupKeyring::key_id_for(&subgroup_key);
    let context_id = ContextId::from([0xC8u8; 32]);

    // ---- Step 1: buffer the encrypted ContextRegistered (effect-skipped) ---
    // `state_hash == [0u8; 32]` skips the staleness check, so the re-drive
    // applies cleanly once key+meta are present (the staleness path is already
    // covered by the node-level R1 test).
    let inner_op = GroupOp::ContextRegistered {
        context_id,
        application_id: ApplicationId::from([0xCCu8; 32]),
        blob_id: calimero_primitives::blobs::BlobId::from([0xDDu8; 32]),
        source: "calimero://stub-app".to_owned(),
        service_name: None,
    };
    let encrypted = GroupKeyring::encrypt_op(&subgroup_key, &inner_op).unwrap();
    let ctx_registered_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Group {
            group_id: sub_gid.to_bytes(),
            key_id,
            encrypted,
            key_rotation: None,
        },
    )
    .unwrap();
    apply_signed_namespace_op(&store, &ctx_registered_op)
        .expect("apply buffered ContextRegistered (effect-skipped: no key, no meta)");

    // Effect skipped: context not registered, no enumeration yet (no key held).
    assert_eq!(
        get_group_for_context(&store, &context_id).unwrap(),
        None,
        "buffered ContextRegistered must be effect-skipped before the key arrives"
    );
    assert!(
        namespace_groups_with_held_key_buffered_ops(&store, namespace_id)
            .unwrap()
            .is_empty(),
        "no group should be enumerated while the key is still absent (awaiting-key state)"
    );

    // ---- Step 2: make the node stranded WITHOUT a trigger ------------------
    // Write the subgroup key and meta DIRECTLY. This skips the apply path, so
    // the Part A GroupCreated re-drive never fires — exactly the pre-fix
    // stranded residue: key held, meta present, op still buffered, no trigger.
    let stored_key_id = GroupKeyring::new(&store, sub_gid)
        .store_key(&subgroup_key)
        .unwrap();
    assert_eq!(
        stored_key_id, key_id,
        "stored key id must match the op's key_id"
    );
    MetaRepository::new(&store)
        .save(&sub_gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    // Nest the subgroup so it resolves under the namespace (for completeness).
    nest_for_test(&store, &ns_gid, &sub_gid);

    // ---- Enumerator: exactly the stranded subgroup is returned -------------
    let enumerated = namespace_groups_with_held_key_buffered_ops(&store, namespace_id).unwrap();
    assert_eq!(
        enumerated,
        vec![sub_gid.to_bytes()],
        "the held-key subgroup with a buffered op must be enumerated for the curative sweep"
    );
    // The op is still buffered (not yet re-driven).
    assert_eq!(
        get_group_for_context(&store, &context_id).unwrap(),
        None,
        "the buffered op must still be stranded before the sweep re-drives it"
    );

    // ---- Re-drive: the buffered op applies ---------------------------------
    let applied = redrive_buffered_ops_for_group(&store, namespace_id, sub_gid.to_bytes())
        .expect("re-drive must not error");
    assert_eq!(
        applied, 1,
        "re-driving the stranded subgroup must apply exactly the one buffered op"
    );
    assert_eq!(
        get_group_for_context(&store, &context_id).unwrap(),
        Some(sub_gid),
        "#2848 Part C: the curative re-drive must register the previously-stranded context"
    );

    // ---- Idempotency: a second re-drive applies nothing new ----------------
    // (The namespace op-log is append-only, so the group stays ENUMERATED, but
    // a re-drive of an already-applied op is a nonce-deduped no-op — this is
    // the sweep's convergence signal: applied-count drops to zero.)
    let applied_again = redrive_buffered_ops_for_group(&store, namespace_id, sub_gid.to_bytes())
        .expect("second re-drive must not error");
    assert_eq!(
        applied_again, 0,
        "re-driving an already-applied group must apply nothing (idempotent no-op)"
    );

    // ---- No-op shape: a no-key (deleted/never-keyed) group -----------------
    // Buffer an op for a DIFFERENT subgroup whose key we never store: it is
    // awaiting-key, not held-key, so the curative enumerator must skip it (the
    // held-key filter is also the deleted-group exit).
    let nokey_gid = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());
    let nokey_key: [u8; 32] = {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        k
    };
    let nokey_inner = GroupOp::ContextRegistered {
        context_id: ContextId::from([0xE9u8; 32]),
        application_id: ApplicationId::from([0xCCu8; 32]),
        blob_id: calimero_primitives::blobs::BlobId::from([0xDDu8; 32]),
        source: "calimero://stub-app".to_owned(),
        service_name: None,
    };
    let nokey_encrypted = GroupKeyring::encrypt_op(&nokey_key, &nokey_inner).unwrap();
    let nokey_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Group {
            group_id: nokey_gid.to_bytes(),
            key_id: GroupKeyring::key_id_for(&nokey_key),
            encrypted: nokey_encrypted,
            key_rotation: None,
        },
    )
    .unwrap();
    apply_signed_namespace_op(&store, &nokey_op).expect("buffer no-key op (effect-skipped)");

    let enumerated_after =
        namespace_groups_with_held_key_buffered_ops(&store, namespace_id).unwrap();
    assert!(
        !enumerated_after.contains(&nokey_gid.to_bytes()),
        "a no-key (deleted/never-keyed) group must NOT be enumerated by the curative sweep"
    );
    // The already-drained held-key subgroup STAYS enumerated (the namespace
    // op-log is append-only, so the logged op persists and the key is still
    // held) — re-driving it is a cheap idempotent no-op (asserted above).
    assert_eq!(
        enumerated_after,
        vec![sub_gid.to_bytes()],
        "the held-key subgroup remains the only curative-set entry; the no-key group is excluded"
    );

    // Sanity: the no-key group IS in the awaiting-key set (the strict inverse).
    // The held-key subgroup is NOT (its key is held).
    let awaiting = namespace_groups_awaiting_key(&store, namespace_id).unwrap();
    assert_eq!(
        awaiting,
        vec![nokey_gid.to_bytes()],
        "the no-key group must be in the awaiting-key set (the inverse of the curative set)"
    );

    // ---- known_namespace_identities returns this namespace -----------------
    assert_eq!(
        known_namespace_identities(&store).unwrap(),
        vec![namespace_id],
        "the node's known-namespace enumeration must include the joined namespace"
    );
}
