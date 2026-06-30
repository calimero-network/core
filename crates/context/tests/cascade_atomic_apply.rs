//! Apply-handler tests for the atomic `GroupOp::CascadeUpgrade` op (PR-3).
//!
//! The atomic op sets `target_application_id`, `app_key`, AND `migration`
//! in a SINGLE descendant walk per matched group, plus stamps a sticky
//! `cascade_hlc` fence onto the per-group upgrade record. This eliminates
//! the receiver apply-order bug that the legacy two-op path
//! (`CascadeTargetApplicationSet` then `CascadeGroupMigrationSet`) suffers
//! from — see the characterization test at the bottom of this file.
//!
//! Harness helpers (`empty_store`/`meta`/`create_group`/consts) are copied
//! verbatim from `cascade_apply_walk.rs`.

use std::sync::Arc;

use calimero_context::group_store::{MembershipRepository, MetaRepository, NamespaceRepository};

use calimero_context::group_store::{
    apply_local_signed_group_op, UpgradeLadderRepository, UpgradesRepository,
};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use rand::rngs::OsRng;

const APP_KEY_1: [u8; 32] = [0x11; 32];
const APP_KEY_2: [u8; 32] = [0x22; 32];

fn app_id_1() -> ApplicationId {
    ApplicationId::from([0xAA; 32])
}
fn app_id_2() -> ApplicationId {
    ApplicationId::from([0xBB; 32])
}

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn meta(admin: PublicKey, app_key: [u8; 32], target: ApplicationId) -> GroupMetaValue {
    GroupMetaValue {
        app_key,
        target_application_id: target,
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// Create a group at `gid` with `admin` as direct admin (so the
/// cascade arm's per-descendant `MANAGE_APPLICATION` pre-scan passes
/// on every node in the walk) on `app_key`+`target_application_id`.
fn create_group(
    store: &Store,
    gid: &ContextGroupId,
    admin: PublicKey,
    app_key: [u8; 32],
    target: ApplicationId,
) {
    MetaRepository::new(store)
        .save(gid, &meta(admin, app_key, target))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(gid, &admin, GroupMemberRole::Admin)
        .unwrap();
}

#[test]
fn cascade_upgrade_atomic_op_sets_target_app_key_and_migration_and_records_cascade_hlc() {
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let store = empty_store();

    let r = ContextGroupId::from([0x70; 32]);
    let r_b = ContextGroupId::from([0xB1; 32]);
    let r_b_b1 = ContextGroupId::from([0xB2; 32]);
    create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b_b1, admin_pk, APP_KEY_1, app_id_1());
    NamespaceRepository::new(&store).nest(&r, &r_b).unwrap();
    NamespaceRepository::new(&store)
        .nest(&r_b, &r_b_b1)
        .unwrap();

    let fence = calimero_storage::logical_clock::HybridTimestamp::zero();
    let op = SignedGroupOp::sign(
        &admin_sk,
        r.to_bytes(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_app_key: APP_KEY_1.into(),
            app_key: APP_KEY_2.into(),
            target_application_id: app_id_2(),
            migration: Some(b"migrate_v2".to_vec()),
            cascade_hlc: fence,
        },
    )
    .expect("sign CascadeUpgrade");

    apply_local_signed_group_op(&store, &op).expect("atomic cascade applies");

    for gid in [&r, &r_b, &r_b_b1] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("meta");
        assert_eq!(m.app_key, APP_KEY_2);
        assert_eq!(m.target_application_id, app_id_2());
        assert_eq!(m.migration, Some(b"migrate_v2".to_vec()));
        let up = UpgradesRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("upgrade record");
        assert_eq!(up.cascade_hlc, Some(fence));
        // The op also appends an upgrade-ladder rung per matched descendant
        // — the sequence a behind context replays to catch up.
        let rungs = UpgradeLadderRepository::new(&store).load(gid).unwrap();
        assert_eq!(rungs.len(), 1);
        assert_eq!(rungs[0].app_key, APP_KEY_2);
        assert_eq!(rungs[0].application_id, app_id_2());
    }
}

/// Reverse-delivery convergence guard for the ATOMIC op.
///
/// Mirrors `cascade_concurrent_safety.rs`'s DagStore+applier harness, but
/// proves the positive counterpart to the characterization below: because
/// the cascade is a SINGLE `CascadeUpgrade` op, no physical arrival order
/// of the surrounding governance ops can split target/app_key/migration
/// across two apply passes. A benign predecessor op (`GroupMetadataSet`,
/// genesis-parented) and the `CascadeUpgrade` op (parented on it) are
/// delivered to replica B in REVERSE order — the DAG queues the cascade
/// pending until its parent arrives, then applies both in causal order.
/// Both replicas converge to (K2, APP_ID_2, migrate_v2) with an identical
/// `cascade_hlc` on every descendant.
#[tokio::test]
async fn cascade_upgrade_reverse_delivery_converges_atomically() {
    use calimero_context::governance_dag::{signed_op_to_delta, GroupGovernanceApplier};
    use calimero_dag::DagStore;

    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    // Per-replica fixture builder: root R on K1 + child R/A on K1.
    let build = || {
        let store = empty_store();
        let r = ContextGroupId::from([0x70; 32]);
        let r_a = ContextGroupId::from([0x71; 32]);
        create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());
        create_group(&store, &r_a, admin_pk, APP_KEY_1, app_id_1());
        NamespaceRepository::new(&store).nest(&r, &r_a).unwrap();
        (store, r, r_a)
    };

    let (store_a, root, child) = build();
    let (store_b, root_b, child_b) = build();
    assert_eq!(root, root_b);
    assert_eq!(child, child_b);

    let fence = calimero_storage::logical_clock::HybridTimestamp::zero();

    // Op P: benign predecessor (genesis-parented). Causally first.
    let op_p = SignedGroupOp::sign(
        &admin_sk,
        root.to_bytes(),
        vec![[0u8; 32]],
        1,
        GroupOp::GroupMetadataSet {
            name: Some("label".to_owned()),
            data: std::collections::BTreeMap::from([("k".to_owned(), "pre-cascade".to_owned())]),
        },
    )
    .expect("sign op_p");
    let op_p_hash = op_p.content_hash().expect("op_p content_hash");

    // Op C: the atomic CascadeUpgrade, causally AFTER op_p.
    let op_c = SignedGroupOp::sign(
        &admin_sk,
        root.to_bytes(),
        vec![op_p_hash],
        2,
        GroupOp::CascadeUpgrade {
            from_app_key: APP_KEY_1.into(),
            app_key: APP_KEY_2.into(),
            target_application_id: app_id_2(),
            migration: Some(b"migrate_v2".to_vec()),
            cascade_hlc: fence,
        },
    )
    .expect("sign op_c");

    // Replica A: receive in causal order — op_p then op_c.
    {
        let applier = GroupGovernanceApplier::new(store_a.clone());
        let mut dag = DagStore::new([0u8; 32]);
        let p = dag
            .add_delta(signed_op_to_delta(&op_p).unwrap(), &applier)
            .await
            .expect("op_p applies on replica A");
        assert!(p, "op_p should apply immediately on replica A");
        let c = dag
            .add_delta(signed_op_to_delta(&op_c).unwrap(), &applier)
            .await
            .expect("op_c (CascadeUpgrade) applies on replica A");
        assert!(c, "op_c should apply immediately on replica A");
    }

    // Replica B: receive in REVERSE order — op_c first (queued pending
    // until op_p arrives), then op_p arrives and cascades into op_c.
    {
        let applier = GroupGovernanceApplier::new(store_b.clone());
        let mut dag = DagStore::new([0u8; 32]);
        let c_pending = dag
            .add_delta(signed_op_to_delta(&op_c).unwrap(), &applier)
            .await
            .expect("dag add op_c on replica B (should queue pending)");
        assert!(
            !c_pending,
            "op_c should be pending on replica B (op_p not yet arrived)"
        );
        // Still on K1 while op_c is pending.
        let pre = MetaRepository::new(&store_b).load(&root).unwrap().unwrap();
        assert_eq!(
            pre.app_key, APP_KEY_1,
            "replica B must still be on K1 while op_c is pending"
        );
        let p = dag
            .add_delta(signed_op_to_delta(&op_p).unwrap(), &applier)
            .await
            .expect("op_p + cascaded-pending op_c must apply on replica B");
        assert!(p, "op_p should apply and cascade pending op_c");
    }

    // Convergence: both replicas land on identical (K2, APP_ID_2,
    // migrate_v2) state on every descendant, with an identical fence.
    for (store, label) in [(&store_a, "A"), (&store_b, "B")] {
        for gid in [&root, &child] {
            let m = MetaRepository::new(store).load(gid).unwrap().expect("meta");
            assert_eq!(m.app_key, APP_KEY_2, "replica {label}: app_key == K2");
            assert_eq!(
                m.target_application_id,
                app_id_2(),
                "replica {label}: target == APP_ID_2"
            );
            assert_eq!(
                m.migration,
                Some(b"migrate_v2".to_vec()),
                "replica {label}: migration NOT dropped (atomic op)"
            );
            let up = UpgradesRepository::new(store)
                .load(gid)
                .unwrap()
                .expect("upgrade record");
            assert_eq!(
                up.cascade_hlc,
                Some(fence),
                "replica {label}: identical cascade_hlc fence"
            );
        }
    }
}

/// CHARACTERIZATION of the xilosada core#2507 review-item-#3 apply-order bug.
/// Delivering CascadeTargetApplicationSet BEFORE CascadeGroupMigrationSet
/// rewrites every descendant's app_key away from `from_app_key`, so the
/// migration-set predicate then matches nothing and `migration` is dropped.
/// The assertion below PASSES today (migration == None) — the green proves
/// the bug exists: the two-op path silently drops migration on reverse delivery.
/// Disposed of in Step 9 once the atomic op replaces the two-op path.
#[test]
#[ignore = "documents the pre-CascadeUpgrade two-op apply-order bug (xilosada core#2507 item #3); cascade no longer emits these ops — see cascade_upgrade.rs"]
#[allow(deprecated)]
fn two_op_reverse_delivery_drops_migration_characterization() {
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let store = empty_store();
    let r = ContextGroupId::from([0x70; 32]);
    create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());

    apply_local_signed_group_op(
        &store,
        &SignedGroupOp::sign(
            &admin_sk,
            r.to_bytes(),
            vec![],
            1,
            GroupOp::CascadeTargetApplicationSet {
                from_app_key: APP_KEY_1.into(),
                app_key: APP_KEY_2.into(),
                target_application_id: app_id_2(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    apply_local_signed_group_op(
        &store,
        &SignedGroupOp::sign(
            &admin_sk,
            r.to_bytes(),
            vec![],
            2,
            GroupOp::CascadeGroupMigrationSet {
                from_app_key: APP_KEY_1.into(),
                migration: Some(b"migrate_v2".to_vec()),
            },
        )
        .unwrap(),
    )
    .unwrap();

    let m = MetaRepository::new(&store).load(&r).unwrap().unwrap();
    assert_eq!(
        m.migration, None,
        "two-op reverse delivery drops migration (documented bug)"
    );
}
