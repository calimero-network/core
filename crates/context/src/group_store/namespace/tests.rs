//! Tests for `group_store::namespace::*`. Extracted from the monolithic
//! `group_store/tests.rs` as part of issue #2480 (epic #2300).
//!
//! Helpers shared with non-namespace tests (`test_store`, `test_group_id`,
//! `test_meta`, `dummy_member_removed_op`, `nest_for_test`,
//! `sample_meta_with_admin`) are imported from the parent
//! `group_store::test_fixtures` module. Namespace-only inline helpers
//! (`raw_namespace_dag_heads`) came along with the move.

use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
use calimero_store::Store;

use super::super::test_fixtures::{
    dummy_member_removed_op, nest_for_test, sample_meta_with_admin, test_group_id, test_meta,
    test_store,
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
            encrypted: encrypt_group_op(&[0xA1; 32], &GroupOp::Noop).unwrap(),
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
            encrypted: encrypt_group_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
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
            encrypted: encrypt_group_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
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
            encrypted: encrypt_group_op(&[0xBB; 32], &GroupOp::Noop).unwrap(),
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
    let key_id = store_group_key(&store, &group_a, &group_key).unwrap();

    let encrypted_a = encrypt_group_op(&group_key, &GroupOp::Noop).unwrap();
    let encrypted_b = encrypt_group_op(&group_key, &GroupOp::Noop).unwrap();

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
    let key_id = store_group_key(&store, &group, &group_key).unwrap();

    // Search the random-key space for a signer whose 4 signed ops
    // produce a content-hash iteration order DIFFERENT from nonce
    // order. P(success per attempt) ≥ 23/24 ≈ 96 % (any non-identity
    // permutation works), so 64 attempts have a silent-pass
    // probability of (1/24)^64 ≈ 10^-88. The explicit `found` flag
    // and the post-loop `assert!(found, …)` make that path loud
    // rather than silently succeeding with in-order ops.
    let max_attempts = 64;
    let mut found = false;
    let mut raw_nonces: Vec<u64> = Vec::new();
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
                        encrypted: encrypt_group_op(&group_key, &GroupOp::Noop).unwrap(),
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

    nest_group(&store, &parent, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();
    assert!(nest_group(&store, &grandchild, &parent).is_err());

    let children = list_child_groups(&store, &parent).unwrap();
    assert_eq!(children, vec![child]);
    let descendants = collect_descendant_groups(&store, &parent).unwrap();
    assert!(descendants.contains(&child));
    assert!(descendants.contains(&grandchild));

    assert_eq!(resolve_namespace(&store, &grandchild).unwrap(), parent);
    assert_eq!(resolve_namespace(&store, &outsider).unwrap(), outsider);

    register_context_in_group(&store, &child, &context).unwrap();
    add_group_member(&store, &child, &ro_member, GroupMemberRole::ReadOnly).unwrap();
    add_group_member(&store, &child, &rw_member, GroupMemberRole::Member).unwrap();
    assert!(is_read_only_for_context(&store, &context, &ro_member).unwrap());
    assert!(!is_read_only_for_context(&store, &context, &rw_member).unwrap());
}

#[test]
fn authorized_for_state_op_admits_admin_and_member_only() {
    use super::is_authorized_for_context_state_op;

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
    save_group_meta(&store, &gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &ro, GroupMemberRole::ReadOnly).unwrap();
    add_group_member(&store, &gid, &ro_tee, GroupMemberRole::ReadOnlyTee).unwrap();

    assert!(
        is_authorized_for_context_state_op(&store, &context, &admin).unwrap(),
        "Admin must be authorized to author state ops"
    );
    assert!(
        is_authorized_for_context_state_op(&store, &context, &member).unwrap(),
        "Member must be authorized to author state ops"
    );
    assert!(
        !is_authorized_for_context_state_op(&store, &context, &ro).unwrap(),
        "ReadOnly must NOT be authorized to author state ops"
    );
    assert!(
        !is_authorized_for_context_state_op(&store, &context, &ro_tee).unwrap(),
        "ReadOnlyTee must NOT be authorized to author state ops"
    );
    assert!(
        !is_authorized_for_context_state_op(&store, &context, &outsider).unwrap(),
        "Non-member must NOT be authorized to author state ops"
    );
}

#[test]
fn authorized_for_state_op_rejects_removed_member() {
    use super::is_authorized_for_context_state_op;

    let store = test_store();
    let gid = ContextGroupId::from([0xD0; 32]);
    let context = ContextId::from([0xD1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xDD; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    save_group_meta(&store, &gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &target, GroupMemberRole::Member).unwrap();

    // Member is authorized while in the group.
    assert!(is_authorized_for_context_state_op(&store, &context, &target).unwrap());

    // After removal: the GroupMember row is gone (apply path deletes it),
    // and the deny-list flags the identity as denied — both ways the
    // check must return `false`. The B3 receive path rejects deltas
    // from this identity at the cut; this check rejects local state ops
    // by the same identity at the WASM-execute path.
    remove_group_member(&store, &gid, &target).unwrap();

    assert!(
        !is_authorized_for_context_state_op(&store, &context, &target).unwrap(),
        "Removed member must NOT be authorized to author state ops locally"
    );
}

#[test]
fn authorized_for_state_op_recognises_namespace_creator() {
    use super::is_authorized_for_context_state_op;

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
    save_group_meta(&store, &gid, &meta).unwrap();
    register_context_in_group(&store, &gid, &context).unwrap();
    // No `GroupMember` row for the creator — relies on the
    // `is_group_admin` carve-out.

    assert!(
        is_authorized_for_context_state_op(&store, &context, &creator).unwrap(),
        "Namespace creator must be authorized via the admin-identity carve-out"
    );
}

#[test]
fn authorized_for_state_op_allows_non_group_context() {
    use super::is_authorized_for_context_state_op;

    // A context that isn't registered under any group has no
    // group-membership concept to enforce. The check returns `true`
    // (no enforcement) so legacy / non-group contexts keep working.
    let store = test_store();
    let context = ContextId::from([0xF1; 32]);
    let executor = PublicKey::from([0xF2; 32]);

    assert!(
        is_authorized_for_context_state_op(&store, &context, &executor).unwrap(),
        "Non-group context must allow any executor (nothing to enforce)"
    );
}

#[test]
fn authorized_for_state_op_admits_inherited_members_via_open_subgroup() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    use super::is_authorized_for_context_state_op;

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
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    save_group_meta(&store, &ns, &meta).unwrap();
    save_group_meta(&store, &child, &meta).unwrap();

    set_default_capabilities(&store, &ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS).unwrap();
    add_group_member(&store, &ns, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &ns, &inherited, GroupMemberRole::Member).unwrap();

    // Register the context under the (Open) child — `inherited` has
    // no row in `child`, only in `ns`.
    register_context_in_group(&store, &child, &context).unwrap();

    assert!(
        is_authorized_for_context_state_op(&store, &context, &inherited).unwrap(),
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(verifier_pk)).unwrap();
    add_group_member(&store, &ns_gid, &verifier_pk, GroupMemberRole::Admin).unwrap();
    let group_key = [0x97u8; 32];
    let key_id = store_group_key(&store, &ns_gid, &group_key).unwrap();

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
    let policy_op = encrypt_group_op(
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
    let join_op = encrypt_group_op(
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
        get_group_member_role(&store, &ns_gid, &tee_member).unwrap(),
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
        count_group_members(&store, &ns_gid).unwrap(),
        2,
        "verifier admin + newly admitted ReadOnlyTee member"
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(signer_pk)).unwrap();
    add_group_member(&store, &ns_gid, &signer_pk, GroupMemberRole::Admin).unwrap();
    let group_key = [0x5Au8; 32];
    let key_id = store_group_key(&store, &ns_gid, &group_key).unwrap();

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
            encrypted: encrypt_group_op(&group_key, &inner_a).unwrap(),
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
            encrypted: encrypt_group_op(&group_key, &inner_b).unwrap(),
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

    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(signer_pk)).unwrap();
    add_group_member(&store, &ns_gid, &signer_pk, GroupMemberRole::Admin).unwrap();
    let group_key = [0x5Bu8; 32];
    let key_id = store_group_key(&store, &ns_gid, &group_key).unwrap();

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
            encrypted: encrypt_group_op(&group_key, &inner_a).unwrap(),
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
            encrypted: encrypt_group_op(&group_key, &inner_b).unwrap(),
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
    nest_group(&store, &root, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();

    // Add admin + member to all groups
    for gid in [&root, &child, &grandchild] {
        save_group_meta(&store, gid, &test_meta()).unwrap();
        add_group_member(&store, gid, &admin, GroupMemberRole::Admin).unwrap();
        add_group_member(&store, gid, &member, GroupMemberRole::Member).unwrap();
    }

    // Verify member exists everywhere
    assert!(check_group_membership(&store, &root, &member).unwrap());
    assert!(check_group_membership(&store, &child, &member).unwrap());
    assert!(check_group_membership(&store, &grandchild, &member).unwrap());

    // Remove from root — should cascade to child and grandchild
    let removed_from = recursive_remove_member(&store, &root, &member).unwrap();
    assert_eq!(removed_from.len(), 3, "should be removed from all 3 groups");

    assert!(!check_group_membership(&store, &root, &member).unwrap());
    assert!(!check_group_membership(&store, &child, &member).unwrap());
    assert!(!check_group_membership(&store, &grandchild, &member).unwrap());

    // Admin should be unaffected
    assert!(check_group_membership(&store, &root, &admin).unwrap());
    assert!(check_group_membership(&store, &child, &admin).unwrap());
    assert!(check_group_membership(&store, &grandchild, &admin).unwrap());
}

#[test]
fn recursive_remove_from_child_does_not_affect_parent() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let grandchild = ContextGroupId::from([0xE2; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    nest_group(&store, &root, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();

    for gid in [&root, &child, &grandchild] {
        save_group_meta(&store, gid, &test_meta()).unwrap();
        add_group_member(&store, gid, &admin, GroupMemberRole::Admin).unwrap();
        add_group_member(&store, gid, &member, GroupMemberRole::Member).unwrap();
    }

    // Remove from child only — should cascade to grandchild but NOT root
    let removed_from = recursive_remove_member(&store, &child, &member).unwrap();
    assert_eq!(removed_from.len(), 2, "removed from child + grandchild");

    // Root membership should be unaffected
    assert!(
        check_group_membership(&store, &root, &member).unwrap(),
        "root membership must survive child removal"
    );
    assert!(!check_group_membership(&store, &child, &member).unwrap());
    assert!(!check_group_membership(&store, &grandchild, &member).unwrap());
}

#[test]
fn recursive_remove_member_not_in_some_descendants() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    nest_group(&store, &root, &child).unwrap();

    for gid in [&root, &child] {
        save_group_meta(&store, gid, &test_meta()).unwrap();
        add_group_member(&store, gid, &admin, GroupMemberRole::Admin).unwrap();
    }
    // Member only in root, not in child
    add_group_member(&store, &root, &member, GroupMemberRole::Member).unwrap();

    let removed_from = recursive_remove_member(&store, &root, &member).unwrap();
    assert_eq!(
        removed_from.len(),
        1,
        "only removed from root where member existed"
    );
    assert!(!check_group_membership(&store, &root, &member).unwrap());
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

    nest_group(&store, &root, &open_child).unwrap();
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &open_child, &test_meta()).unwrap();
    add_group_member(&store, &root, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &open_child, &admin, GroupMemberRole::Admin).unwrap();

    // Direct member of `root` only; inherited into `open_child` via the
    // CAN_JOIN_OPEN_SUBGROUPS cap + Open visibility.
    add_group_member(&store, &root, &member, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &root,
        &member,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &open_child, VisibilityMode::Open).unwrap();

    // Sanity: inherited path works pre-removal.
    assert!(check_group_membership(&store, &open_child, &member).unwrap());

    // Recursive remove anchored at `open_child` must NOT report it as
    // removed-from -- the member has no direct row there.
    let removed_from = recursive_remove_member(&store, &open_child, &member).unwrap();
    assert!(
        removed_from.is_empty(),
        "inherited-only member should not be reported as removed (got {removed_from:?})"
    );

    // The member is still inherited because root membership + cap + Open
    // child are all unchanged.
    assert!(check_group_membership(&store, &open_child, &member).unwrap());

    // To actually revoke, the admin removes them from the anchor (root).
    let removed_from = recursive_remove_member(&store, &root, &member).unwrap();
    assert_eq!(removed_from, vec![root]);
    assert!(!check_group_membership(&store, &open_child, &member).unwrap());
}

#[test]
fn recursive_remove_nonexistent_member_returns_empty() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let stranger = PublicKey::from([0x99; 32]);

    save_group_meta(&store, &root, &test_meta()).unwrap();
    add_group_member(&store, &root, &admin, GroupMemberRole::Admin).unwrap();

    let removed_from = recursive_remove_member(&store, &root, &stranger).unwrap();
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

    nest_group(&store, &ns, &open_sub).unwrap();
    nest_group(&store, &ns, &owner_priv).unwrap();
    nest_group(&store, &owner_priv, &behind_wall).unwrap();
    nest_group(&store, &ns, &inviter_priv).unwrap();
    for gid in [&ns, &open_sub, &owner_priv, &behind_wall, &inviter_priv] {
        save_group_meta(&store, gid, &test_meta()).unwrap();
    }

    // The recursive inviter is an admin of the namespace root.
    let inviter_sk = PrivateKey::random(&mut OsRng);
    let inviter_pk = inviter_sk.public_key();
    add_group_member(&store, &ns, &inviter_pk, GroupMemberRole::Admin).unwrap();

    // open_sub is Open -> the namespace admin inherits in.
    set_subgroup_visibility(&store, &open_sub, VisibilityMode::Open).unwrap();

    // owner_priv is a different member's private DM: Restricted, inviter never added.
    let owner_pk = PrivateKey::random(&mut OsRng).public_key();
    set_subgroup_visibility(&store, &owner_priv, VisibilityMode::Restricted).unwrap();
    add_group_member(&store, &owner_priv, &owner_pk, GroupMemberRole::Admin).unwrap();
    // ...even though there is an Open subgroup *under* it: the wall hides the whole subtree.
    set_subgroup_visibility(&store, &behind_wall, VisibilityMode::Open).unwrap();

    // inviter_priv is Restricted, but the inviter has a direct member row -> visible.
    set_subgroup_visibility(&store, &inviter_priv, VisibilityMode::Restricted).unwrap();
    add_group_member(&store, &inviter_priv, &inviter_pk, GroupMemberRole::Member).unwrap();

    // Sanity on the membership facts the walk depends on.
    assert!(check_group_membership(&store, &open_sub, &inviter_pk).unwrap());
    assert!(!check_group_membership(&store, &owner_priv, &inviter_pk).unwrap());
    assert!(check_group_membership(&store, &inviter_priv, &inviter_pk).unwrap());

    let visible = collect_visible_descendant_groups(&store, &ns, &inviter_pk).unwrap();
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
    let all = collect_descendant_groups(&store, &ns).unwrap();
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

    nest_group(&store, &ns, &open_sub).unwrap();
    nest_group(&store, &ns, &owner_priv).unwrap();
    for gid in [&ns, &open_sub, &owner_priv] {
        save_group_meta(&store, gid, &test_meta()).unwrap();
    }

    let inviter_sk = PrivateKey::random(&mut OsRng);
    let inviter_pk = inviter_sk.public_key();
    add_group_member(&store, &ns, &inviter_pk, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &open_sub, VisibilityMode::Open).unwrap();

    let owner_pk = PrivateKey::random(&mut OsRng).public_key();
    set_subgroup_visibility(&store, &owner_priv, VisibilityMode::Restricted).unwrap();
    add_group_member(&store, &owner_priv, &owner_pk, GroupMemberRole::Admin).unwrap();

    let invitations = create_recursive_invitations(&store, &ns, &inviter_sk, 3600, 1).unwrap();
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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
            }),
        )
        .expect("sign create op");
        gov.apply_signed_op(&op).expect("apply create op");
    }

    assert_eq!(get_parent_group(&store, &leaf_gid).unwrap(), Some(mid_gid));

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
        get_parent_group(&store, &leaf_gid).unwrap(),
        Some(new_parent_gid)
    );
    let mid_children = list_child_groups(&store, &mid_gid).unwrap();
    assert!(!mid_children.contains(&leaf_gid), "leaf detached from mid");
    let new_children = list_child_groups(&store, &new_parent_gid).unwrap();
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

    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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
    assert!(replay.pending_deliveries.is_empty());
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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

    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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
        }),
    )
    .expect("sign op2");

    // Should not error — idempotent
    gov.apply_signed_op(&op2)
        .expect("duplicate GroupCreated should be idempotent");
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

    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

    // Simulate the create_group HANDLER pre-populating meta before publishing:
    // this is the originator's flow.
    save_group_meta(&store, &new_gid, &sample_meta_with_admin(admin_pk)).unwrap();

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
        }),
    )
    .expect("sign op");
    gov.apply_signed_op(&op)
        .expect("apply GroupCreated on originator");

    // Parent edge must exist (the bug was that it wouldn't).
    assert_eq!(
        get_parent_group(&store, &new_gid).unwrap(),
        Some(ns_gid),
        "originator must have parent edge after GroupCreated even though meta was pre-populated"
    );
    // Child index on namespace must include the new group.
    let children = list_child_groups(&store, &ns_gid).unwrap();
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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
        }),
    )
    .expect("sign op");

    let gov = NamespaceGovernance::new(&store, ns_id);
    let err = gov.apply_signed_op(&op).unwrap_err();
    assert!(
        format!("{err}").contains("self-parent"),
        "expected self-parent rejection, got: {err}"
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

    // Build: namespace → A → B (two-level subtree).
    let a_id = [0xAAu8; 32];
    let b_id = [0xBBu8; 32];
    let a_gid = ContextGroupId::from(a_id);
    let b_gid = ContextGroupId::from(b_id);
    save_group_meta(&store, &a_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    save_group_meta(&store, &b_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    nest_group(&store, &ns_gid, &a_gid).unwrap();
    nest_group(&store, &a_gid, &b_gid).unwrap();

    // Pre-compute the ORIGINAL payload (the "full" cascade).
    let original_payload = collect_subtree_for_cascade(&store, &a_gid).unwrap();
    let cascade_group_ids: Vec<[u8; 32]> = original_payload
        .descendant_groups
        .iter()
        .map(|g| g.to_bytes())
        .collect();
    assert_eq!(cascade_group_ids.len(), 1, "B is the only descendant of A");

    // Simulate a partial-delete crash by deleting B's meta + parent edge
    // (i.e., B is "already gone" from a hypothetical first apply attempt).
    delete_group_meta(&store, &b_gid).unwrap();
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
        load_group_meta(&store, &a_gid).unwrap().is_none(),
        "cascade retry must complete the root deletion"
    );
}

#[test]
fn min_acks_after_local_mutation_uses_publish_time_subscribers() {
    let min_acks = super::min_acks_after_local_mutation(1, 0);

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
    save_group_meta(&store, &parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_group(&store, &parent, &child).unwrap();

    assert!(is_descendant_of(&store, &child, &parent).unwrap());
    assert!(!is_descendant_of(&store, &parent, &child).unwrap());
}

#[test]
fn is_descendant_of_grandchild() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let mid = ContextGroupId::from([0xD1; 32]);
    let leaf = ContextGroupId::from([0xD2; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &mid, &test_meta()).unwrap();
    save_group_meta(&store, &leaf, &test_meta()).unwrap();
    nest_group(&store, &root, &mid).unwrap();
    nest_group(&store, &mid, &leaf).unwrap();

    assert!(is_descendant_of(&store, &leaf, &root).unwrap());
    assert!(is_descendant_of(&store, &leaf, &mid).unwrap());
    assert!(!is_descendant_of(&store, &root, &leaf).unwrap());
}

#[test]
fn is_descendant_of_unrelated() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    let b = ContextGroupId::from([0xD1; 32]);
    assert!(!is_descendant_of(&store, &a, &b).unwrap());
    assert!(!is_descendant_of(&store, &b, &a).unwrap());
}

#[test]
fn is_descendant_of_self_is_false() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    assert!(!is_descendant_of(&store, &a, &a).unwrap());
}

#[test]
fn reparent_group_swaps_parent_edge() {
    let store = test_store();
    let old_parent = ContextGroupId::from([0xE0; 32]);
    let new_parent = ContextGroupId::from([0xE1; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    save_group_meta(&store, &old_parent, &test_meta()).unwrap();
    save_group_meta(&store, &new_parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_group(&store, &old_parent, &child).unwrap();

    reparent_group(&store, &child, &new_parent).unwrap();

    assert_eq!(get_parent_group(&store, &child).unwrap(), Some(new_parent));
    let old_children = list_child_groups(&store, &old_parent).unwrap();
    assert!(!old_children.contains(&child));
    let new_children = list_child_groups(&store, &new_parent).unwrap();
    assert!(new_children.contains(&child));
}

#[test]
fn reparent_group_idempotent_on_same_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    save_group_meta(&store, &parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_group(&store, &parent, &child).unwrap();

    reparent_group(&store, &child, &parent).unwrap();
    assert_eq!(get_parent_group(&store, &child).unwrap(), Some(parent));
    assert_eq!(list_child_groups(&store, &parent).unwrap().len(), 1);
}

#[test]
fn reparent_group_rejects_cycle() {
    let store = test_store();
    let a = ContextGroupId::from([0xE0; 32]);
    let b = ContextGroupId::from([0xE1; 32]);
    save_group_meta(&store, &a, &test_meta()).unwrap();
    save_group_meta(&store, &b, &test_meta()).unwrap();
    nest_group(&store, &a, &b).unwrap();

    let err = reparent_group(&store, &a, &b).unwrap_err();
    assert!(
        format!("{err}").contains("cycle") || format!("{err}").contains("namespace root"),
        "expected cycle or root error, got: {err}"
    );
}

#[test]
fn reparent_group_rejects_root() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let other = ContextGroupId::from([0xE1; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &other, &test_meta()).unwrap();

    let err = reparent_group(&store, &root, &other).unwrap_err();
    assert!(
        format!("{err}").contains("namespace root") || format!("{err}").contains("no parent"),
        "expected root rejection, got: {err}"
    );
}

#[test]
fn reparent_group_rejects_nonexistent_new_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    let phantom = ContextGroupId::from([0xFF; 32]);
    save_group_meta(&store, &parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_group(&store, &parent, &child).unwrap();

    let err = reparent_group(&store, &child, &phantom).unwrap_err();
    assert!(
        format!("{err}").contains("not found") || format!("{err}").contains("does not exist"),
        "expected new-parent-not-found, got: {err}"
    );
}

#[test]
fn collect_subtree_for_cascade_empty_subtree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
    assert!(payload.descendant_groups.is_empty());
    assert!(payload.contexts.is_empty());
}

#[test]
fn collect_subtree_for_cascade_two_level_tree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let mid = ContextGroupId::from([0xF1; 32]);
    let leaf = ContextGroupId::from([0xF2; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &mid, &test_meta()).unwrap();
    save_group_meta(&store, &leaf, &test_meta()).unwrap();
    nest_group(&store, &root, &mid).unwrap();
    nest_group(&store, &mid, &leaf).unwrap();

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
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
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_group(&store, &root, &child).unwrap();

    let ctx_root = ContextId::from([0x10; 32]);
    let ctx_child = ContextId::from([0x11; 32]);
    register_context_in_group(&store, &root, &ctx_root).unwrap();
    register_context_in_group(&store, &child, &ctx_child).unwrap();

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &ns_gid, &member_pk, GroupMemberRole::Member).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

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
            }),
        )
        .unwrap()
    };

    let chan = [0xB1u8; 32];

    // A total stranger (not a namespace member, no capability row) cannot
    // create a subgroup — rejected by the apply-side authorization check.
    assert!(
        !check_group_membership(&store, &ns_gid, &stranger_sk.public_key()).unwrap(),
        "precondition: the stranger must not be enrolled in the namespace"
    );
    let err = gov
        .apply_signed_op(&create(&stranger_sk, chan, ns_id, 1))
        .unwrap_err();
    assert!(
        format!("{err}").contains("GroupCreated rejected"),
        "stranger should be rejected by the authorization check, got: {err}"
    );
    assert!(load_group_meta(&store, &ContextGroupId::from(chan))
        .unwrap()
        .is_none());

    // Member without the cap cannot create a subgroup, even under the root.
    assert!(gov
        .apply_signed_op(&create(&member_sk, chan, ns_id, 2))
        .is_err());
    assert!(load_group_meta(&store, &ContextGroupId::from(chan))
        .unwrap()
        .is_none());

    // Granting CAN_CREATE_SUBGROUP at the namespace root lets them create one
    // directly under the root, and they become its owner.
    set_member_capability(
        &store,
        &ns_gid,
        &member_pk,
        MemberCapabilities::CAN_CREATE_SUBGROUP,
    )
    .unwrap();
    gov.apply_signed_op(&create(&member_sk, chan, ns_id, 3))
        .expect("member with CAN_CREATE_SUBGROUP creates a subgroup under the root");
    assert_eq!(
        load_group_meta(&store, &ContextGroupId::from(chan))
            .unwrap()
            .unwrap()
            .owner_identity,
        member_pk,
        "creator owns the new subgroup"
    );
    assert!(
        is_group_admin(&store, &ContextGroupId::from(chan), &member_pk).unwrap(),
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
    save_group_meta(&store, &ns_gid, &sample_meta_with_admin(admin_pk)).unwrap();
    add_group_member(&store, &ns_gid, &admin_pk, GroupMemberRole::Admin).unwrap();
    // `owner_pk` is enrolled as an ordinary namespace member — that mirrors the
    // real model (a subgroup owner got there by being a namespace member and
    // creating it; `leave_namespace` refuses an owner via `MustTransferOwnership`,
    // so an owner is always a current member). It holds no caps and no admin
    // role at the namespace level, so it can only delete via the owner path.
    add_group_member(&store, &ns_gid, &owner_pk, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &ns_gid, &plain_member_pk, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &ns_gid, &janitor_pk, GroupMemberRole::Member).unwrap();
    store_namespace_identity(&store, &ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32]).unwrap();

    // Three leaf subgroups under the root, all owned by `owner_pk`.
    let mk_subgroup = |tag: u8| {
        let id = [tag; 32];
        let gid = ContextGroupId::from(id);
        save_group_meta(&store, &gid, &sample_meta_with_admin(owner_pk)).unwrap();
        nest_group(&store, &ns_gid, &gid).unwrap();
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
        !check_group_membership(&store, &ns_gid, &stranger_sk.public_key()).unwrap(),
        "precondition: the stranger must not be enrolled in the namespace"
    );
    let err = gov.apply_signed_op(&del(&stranger_sk, s1, 1)).unwrap_err();
    assert!(
        format!("{err}").contains("GroupDeleted rejected"),
        "stranger should be rejected by the authorization check, got: {err}"
    );
    assert!(load_group_meta(&store, &s1_gid).unwrap().is_some());

    // A plain namespace member (no CAN_DELETE_SUBGROUP, not the owner, not an
    // admin) is also rejected — the distinct "member but unauthorized" case,
    // again by the authorization check.
    let err = gov
        .apply_signed_op(&del(&plain_member_sk, s1, 2))
        .unwrap_err();
    assert!(
        format!("{err}").contains("GroupDeleted rejected"),
        "plain member should be rejected by the authorization check, got: {err}"
    );
    assert!(load_group_meta(&store, &s1_gid).unwrap().is_some());

    // The subgroup's owner can cascade-delete it.
    gov.apply_signed_op(&del(&owner_sk, s1, 3))
        .expect("subgroup owner can delete it");
    assert!(load_group_meta(&store, &s1_gid).unwrap().is_none());

    // Re-applying the same GroupDeleted after the root meta is gone (the
    // crash-recovery shape: cascade finished, DAG head not yet advanced) must
    // be an idempotent no-op, even though the signer here (`owner_pk`) is not
    // a namespace admin and holds no CAN_DELETE_SUBGROUP — the auth check is
    // skipped when the root meta is absent.
    gov.apply_signed_op(&del(&owner_sk, s1, 6))
        .expect("re-apply of GroupDeleted after the root meta is gone is an idempotent no-op");
    assert!(load_group_meta(&store, &s1_gid).unwrap().is_none());

    // A namespace admin can delete a subgroup they don't own (moderation).
    gov.apply_signed_op(&del(&admin_sk, s2, 4))
        .expect("namespace admin can delete any subgroup");
    assert!(load_group_meta(&store, &s2_gid).unwrap().is_none());

    // A namespace member holding CAN_DELETE_SUBGROUP can delete a subgroup.
    set_member_capability(
        &store,
        &ns_gid,
        &janitor_pk,
        MemberCapabilities::CAN_DELETE_SUBGROUP,
    )
    .unwrap();
    gov.apply_signed_op(&del(&janitor_sk, s3, 5))
        .expect("CAN_DELETE_SUBGROUP holder can delete a subgroup");
    assert!(load_group_meta(&store, &s3_gid).unwrap().is_none());
}
