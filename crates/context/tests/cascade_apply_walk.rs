//! End-to-end apply-handler test for the cascade engine
//! (`GroupOp::CascadeTargetApplicationSet`).
//!
//! This exercises the store-level apply path
//! [`apply_local_signed_group_op`] — i.e. what a peer receiving the
//! cascade op via gossip executes locally. The actor-side dispatch
//! (`handlers/upgrade_group.rs::dispatch_cascade`) is not exercised
//! here: it sits one layer above and requires a full actor context.
//! The cascade engine's *behaviour* lives in `cascade::walk_for_predicate`
//! + the apply arm in `group_store::apply_group_op_mutations`, and that
//!   is exactly what `apply_local_signed_group_op` drives.

use calimero_context::group_store::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::sync::Arc;

use calimero_context::group_store::apply_local_signed_group_op;
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
fn cascade_target_application_set_updates_all_matching_descendants_and_skips_sibling_namespace() {
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let store = empty_store();

    // Namespace R: root + R/A + R/B + R/B/B1, all on APP_KEY_1 / APP_ID_1.
    let r = ContextGroupId::from([0x70; 32]);
    let r_a = ContextGroupId::from([0xA1; 32]);
    let r_b = ContextGroupId::from([0xB1; 32]);
    let r_b_b1 = ContextGroupId::from([0xB2; 32]);

    create_group(&store, &r, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_a, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &r_b_b1, admin_pk, APP_KEY_1, app_id_1());

    NamespaceRepository::new(&store).nest(&r, &r_a).unwrap();
    NamespaceRepository::new(&store).nest(&r, &r_b).unwrap();
    NamespaceRepository::new(&store)
        .nest(&r_b, &r_b_b1)
        .unwrap();

    // Sibling namespace S: completely separate root with one child.
    // Same APP_KEY_1 as R so we prove the cascade's tree-walk is what
    // contains the blast radius (not a global app_key sweep).
    let s = ContextGroupId::from([0x50; 32]);
    let s_x = ContextGroupId::from([0x51; 32]);

    create_group(&store, &s, admin_pk, APP_KEY_1, app_id_1());
    create_group(&store, &s_x, admin_pk, APP_KEY_1, app_id_1());
    NamespaceRepository::new(&store).nest(&s, &s_x).unwrap();

    // Sanity: every group starts on (APP_KEY_1, APP_ID_1).
    for gid in [&r, &r_a, &r_b, &r_b_b1, &s, &s_x] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("meta");
        assert_eq!(m.app_key, APP_KEY_1);
        assert_eq!(m.target_application_id, app_id_1());
    }

    // Cascade op signed on R, targeting from_app_key=K1, new app_key=K2 + new target=APP_ID_2.
    let cascade_op = SignedGroupOp::sign(
        &admin_sk,
        r.to_bytes(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: APP_KEY_1,
            app_key: APP_KEY_2,
            target_application_id: app_id_2(),
        },
    )
    .expect("sign CascadeTargetApplicationSet");

    // Apply must succeed (i.e. `apply_group_op_mutations` returns
    // `Ok((true, _))`). A `false` (variant-not-handled) return would
    // make `apply_local_signed_group_op` bail with "unsupported group
    // op variant for local apply" — failing this assertion.
    apply_local_signed_group_op(&store, &cascade_op).expect("cascade op applies cleanly");

    // Every group under R must now be on (APP_KEY_2, APP_ID_2).
    for gid in [&r, &r_a, &r_b, &r_b_b1] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("meta after");
        assert_eq!(
            m.app_key,
            APP_KEY_2,
            "group {} in cascaded subtree must be on K2",
            hex::encode(gid.to_bytes())
        );
        assert_eq!(
            m.target_application_id,
            app_id_2(),
            "group {} in cascaded subtree must point at APP_ID_2",
            hex::encode(gid.to_bytes())
        );
    }

    // Sibling namespace S must be untouched — the cascade walked
    // descendants of R only, not "every group with app_key == K1".
    for gid in [&s, &s_x] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("sibling meta");
        assert_eq!(
            m.app_key,
            APP_KEY_1,
            "sibling-namespace group {} must NOT be touched by R's cascade",
            hex::encode(gid.to_bytes())
        );
        assert_eq!(
            m.target_application_id,
            app_id_1(),
            "sibling-namespace group {} must keep its original target",
            hex::encode(gid.to_bytes())
        );
    }
}
