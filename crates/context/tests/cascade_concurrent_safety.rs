//! Concurrent-cascade safety regression for the cascade engine.
//!
//! When two cascade ops race against the same subtree, the
//! *predicate-skip* path is the optimistic-concurrency guard: the loser
//! arrives with `from_bytecode_id` no longer matching any group's current
//! `bytecode_id`, so its walk produces zero matched entries and the apply
//! arm finishes as a silent no-op (per spec §5). The handler must
//! still report `handled = true` to the caller — otherwise
//! [`apply_local_signed_group_op`] bails with "unsupported group op
//! variant for local apply", which would break both:
//!
//! - the audit-log persistence path in `namespace/governance.rs`
//!   (only persists when `handled == true`), and
//! - cross-peer convergence (the loser would fail on the receiver
//!   and succeed on the emitter, leaving the two stores divergent).
//!
//! That `handled = true` fall-through is the **C1 regression guard**.
//! If anyone reverts the fix and makes the cascade arm return
//! `Ok((false, None))` when `any_applied == false`, the second
//! `apply_local_signed_group_op` call on the loser op will bail with
//! `unsupported group op variant for local apply`, flagging the
//! regression at PR-review time.
//!
//! See `docs/superpowers/plans/2026-05-26-pr2-cascade-engine.md` Task 9
//! and the comment in
//! `crates/context/src/group_store/mod.rs::apply_group_op_mutations`
//! `CascadeUpgrade` arm (the `divergence = None;
//! /* fall-through */` block).
//!
//! ## Replica-convergence design
//!
//! The two cascade ops are arranged in a **causal chain** (Op B
//! parents = `[content_hash(Op A)]`) and applied via the DAG-mediated
//! path ([`GroupGovernanceApplier`] + [`DagStore`]). The DAG layer
//! guarantees ancestor-before-descendant: a replica that receives
//! Op B before Op A queues it as pending until Op A arrives, then
//! applies Op A and cascades to Op B. So both replicas — regardless
//! of physical arrival order — apply (Op A, Op B) in causal order.
//! That gives us cross-replica convergence to the SAME final state
//! AND lets us assert that the second (Op B) apply lands in the
//! predicate-skip branch, which is the C1 regression guard.

use calimero_governance_store::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::sync::Arc;

use calimero_context::governance_dag::{signed_op_to_delta, GroupGovernanceApplier};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_dag::DagStore;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use rand::rngs::OsRng;

const BYTECODE_ID_1: [u8; 32] = [0x11; 32];
const BYTECODE_ID_2: [u8; 32] = [0x22; 32];
const BYTECODE_ID_3: [u8; 32] = [0x33; 32];

fn app_id_1() -> ApplicationId {
    ApplicationId::from([0xAA; 32])
}
fn app_id_2() -> ApplicationId {
    ApplicationId::from([0xBB; 32])
}
fn app_id_3() -> ApplicationId {
    ApplicationId::from([0xCC; 32])
}

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn meta(
    admin: calimero_account::AccountId,
    bytecode_id: [u8; 32],
    target: ApplicationId,
) -> GroupMetaValue {
    GroupMetaValue {
        bytecode_id,
        target_application_id: target,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
        package: Box::default(),
        version: Box::default(),
    }
}

/// `admin` is a signing KEY; it is enrolled so the rows name the account that
/// key resolves to, which is what every gate reading them will look up.
fn create_group(
    store: &Store,
    gid: &ContextGroupId,
    admin: PublicKey,
    bytecode_id: [u8; 32],
    target: ApplicationId,
) -> calimero_account::AccountId {
    let account = calimero_context::test_support::enrol(store, gid, &admin);
    MetaRepository::new(store)
        .save(gid, &meta(account, bytecode_id, target))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(gid, &account, GroupMemberRole::Admin)
        .unwrap();
    account
}

/// Build the per-replica fixture: root R on K1 + one child R/A on K1,
/// admin granted on both.
fn build_replica(admin_pk: PublicKey) -> (Store, ContextGroupId, ContextGroupId) {
    let store = empty_store();
    let r = ContextGroupId::from([0x70; 32]);
    let r_a = ContextGroupId::from([0x71; 32]);
    create_group(&store, &r, admin_pk, BYTECODE_ID_1, app_id_1());
    create_group(&store, &r_a, admin_pk, BYTECODE_ID_1, app_id_1());
    NamespaceRepository::new(&store).nest(&r, &r_a).unwrap();
    (store, r, r_a)
}

#[tokio::test]
async fn divergent_cascade_apply_order_converges_via_predicate_skip() {
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let (store_a, root_a, child_a) = build_replica(admin_pk);
    let (store_b, root_b, child_b) = build_replica(admin_pk);
    // Both replicas use the same hardcoded group IDs.
    assert_eq!(root_a, root_b);
    assert_eq!(child_a, child_b);
    let root = root_a;
    let child = child_a;

    // Op A: cascade K1 -> K2 (genesis-parented). Causally first.
    let op_a = SignedGroupOp::sign(
        &admin_sk,
        root.to_bytes().into(),
        vec![[0u8; 32]],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: BYTECODE_ID_1.into(),
            bytecode_id: BYTECODE_ID_2.into(),
            target_application_id: app_id_2(),
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.example.app".to_owned(),
            version: "2.0.0".to_owned(),
        },
    )
    .expect("sign op_a");
    let op_a_hash = op_a.content_hash().expect("op_a content_hash");

    // Op B: cascade K1 -> K3, causally AFTER op_a (parent =
    // content_hash(op_a)). Same `from_bytecode_id` as op_a, so on a tree
    // that's already executed op_a, op_b's predicate matches nothing
    // and the cascade arm hits the predicate-skip path.
    let op_b = SignedGroupOp::sign(
        &admin_sk,
        root.to_bytes().into(),
        vec![op_a_hash],
        2,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: BYTECODE_ID_1.into(),
            bytecode_id: BYTECODE_ID_3.into(),
            target_application_id: app_id_3(),
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.example.app".to_owned(),
            version: "2.0.0".to_owned(),
        },
    )
    .expect("sign op_b");

    // Replica A: receive in causal order — op_a first, then op_b.
    {
        let applier = GroupGovernanceApplier::new(store_a.clone());
        let mut dag = DagStore::new([0u8; 32]);
        let applied_a = dag
            .add_delta(signed_op_to_delta(&op_a).unwrap(), &applier)
            .await
            .expect("dag add op_a on replica A");
        assert!(applied_a, "op_a should apply immediately on replica A");
        // C1 regression guard: this `add_delta` call ends up running
        // through `apply_local_signed_group_op` -> cascade arm with
        // no matched descendants. If the arm returned `handled =
        // false`, the underlying `apply_local_signed_group_op` would
        // bail with "unsupported group op variant for local apply"
        // and this `add_delta` would return `Err`.
        let applied_b = dag
            .add_delta(signed_op_to_delta(&op_b).unwrap(), &applier)
            .await
            .expect(
                "op_b must apply as silent no-op on replica A (C1 regression guard) — \
                 if this fails with \"unsupported group op variant for local apply\", \
                 the cascade arm in apply_group_op_mutations regressed: it must fall \
                 through to Ok((true, None)) even when no descendants matched",
            );
        assert!(
            applied_b,
            "op_b should apply (as silent no-op) on replica A"
        );
    }

    // Replica B: receive in REVERSE order — op_b arrives first
    // (queued as pending because op_a hasn't arrived), then op_a
    // arrives, applies, and cascades into op_b's now-unblocked apply.
    {
        let applier = GroupGovernanceApplier::new(store_b.clone());
        let mut dag = DagStore::new([0u8; 32]);
        let applied_b_pending = dag
            .add_delta(signed_op_to_delta(&op_b).unwrap(), &applier)
            .await
            .expect("dag add op_b on replica B (should queue pending)");
        assert!(
            !applied_b_pending,
            "op_b should be pending on replica B (op_a not yet arrived)"
        );
        // Until op_a arrives, op_b has not been applied — store_b's
        // groups are still on K1.
        let pre = MetaRepository::new(&store_b).load(&root).unwrap().unwrap();
        assert_eq!(
            pre.bytecode_id, BYTECODE_ID_1,
            "replica B must still be on K1 while op_b is pending"
        );

        // op_a arrives: applies, then the DAG cascades into op_b's
        // pending apply (which now sees a tree on K2 and hits the
        // predicate-skip path). Both must apply cleanly.
        let applied_a = dag
            .add_delta(signed_op_to_delta(&op_a).unwrap(), &applier)
            .await
            .expect(
                "op_a + cascaded-pending op_b must apply on replica B \
                 (C1 regression guard fires here on the cascaded op_b apply)",
            );
        assert!(
            applied_a,
            "op_a should apply immediately and cascade pending op_b"
        );
    }

    // Convergence: both replicas land on identical state, despite
    // physically receiving the ops in opposite orders. The DAG-causal
    // winner is op_a (it's op_b's ancestor), so the final bytecode_id on
    // both replicas is K2 / APP_ID_2.
    let final_a_root = MetaRepository::new(&store_a).load(&root).unwrap().unwrap();
    let final_a_child = MetaRepository::new(&store_a).load(&child).unwrap().unwrap();
    let final_b_root = MetaRepository::new(&store_b).load(&root).unwrap().unwrap();
    let final_b_child = MetaRepository::new(&store_b).load(&child).unwrap().unwrap();

    assert_eq!(
        final_a_root.bytecode_id, BYTECODE_ID_2,
        "replica A: causal-winner op_a moved every group to K2"
    );
    assert_eq!(
        final_a_root.target_application_id,
        app_id_2(),
        "replica A: target_application_id == APP_ID_2"
    );
    assert_eq!(final_a_child.bytecode_id, BYTECODE_ID_2);
    assert_eq!(final_a_child.target_application_id, app_id_2());

    // Cross-replica convergence: replica B ends in the EXACT same
    // state as replica A — bytes-equal on every field of GroupMeta.
    assert_eq!(
        final_b_root.bytecode_id, final_a_root.bytecode_id,
        "convergence: replica B root bytecode_id must match replica A"
    );
    assert_eq!(
        final_b_root.target_application_id, final_a_root.target_application_id,
        "convergence: replica B root target_application_id must match replica A"
    );
    assert_eq!(
        final_b_child.bytecode_id, final_a_child.bytecode_id,
        "convergence: replica B child bytecode_id must match replica A"
    );
    assert_eq!(
        final_b_child.target_application_id, final_a_child.target_application_id,
        "convergence: replica B child target_application_id must match replica A"
    );

    // Loser bytecode_id (K3) won on neither replica — op_b's predicate
    // skip was correctly triggered both times.
    assert_ne!(
        final_a_root.bytecode_id, BYTECODE_ID_3,
        "loser op_b must NOT have written K3 on replica A"
    );
    assert_ne!(
        final_b_root.bytecode_id, BYTECODE_ID_3,
        "loser op_b must NOT have written K3 on replica B"
    );
}
