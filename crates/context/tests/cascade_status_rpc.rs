//! Integration test for `collect_cascade_status` (Task 6, PR-3).
//!
//! Verifies that the pure core fn walks the namespace subtree and returns one
//! entry per group that has an upgrade record, with the correct `cascade_hlc`
//! stamped by the atomic `CascadeUpgrade` op.

use std::sync::Arc;

use calimero_context::group_store::{
    apply_local_signed_group_op, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_context::handlers::get_cascade_status::collect_cascade_status;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::HybridTimestamp;
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
fn collect_cascade_status_returns_entries_for_all_three_groups() {
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

    let fence = HybridTimestamp::zero();
    let op = SignedGroupOp::sign(
        &admin_sk,
        r.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::CascadeUpgrade {
            from_app_key: APP_KEY_1,
            app_key: APP_KEY_2,
            target_application_id: app_id_2(),
            migration: Some(b"migrate_v2".to_vec()),
            cascade_hlc: fence,
        },
    )
    .expect("sign CascadeUpgrade");

    apply_local_signed_group_op(&store, &op).expect("atomic cascade applies");

    let entries = collect_cascade_status(&store, &r).unwrap();
    assert_eq!(
        entries.len(),
        3,
        "expected one entry per group in namespace"
    );

    for e in &entries {
        assert_eq!(
            e.cascade_hlc,
            Some(fence),
            "each group should have cascade_hlc == fence"
        );
    }

    // All group ids should be present (order not guaranteed).
    let ids: std::collections::HashSet<ContextGroupId> =
        entries.iter().map(|e| e.group_id).collect();
    assert!(ids.contains(&r), "root group should be present");
    assert!(ids.contains(&r_b), "r_b group should be present");
    assert!(ids.contains(&r_b_b1), "r_b_b1 group should be present");
}

#[test]
fn collect_cascade_status_empty_when_no_upgrade_records() {
    let store = empty_store();
    let r = ContextGroupId::from([0x70; 32]);
    // No upgrade records at all → empty result.
    let entries = collect_cascade_status(&store, &r).unwrap();
    assert!(
        entries.is_empty(),
        "no upgrade records → no cascade status entries"
    );
}
