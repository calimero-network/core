//! End-to-end apply-handler test for the cascade engine
//! (`GroupOp::CascadeUpgrade`).
//!
//! This exercises the store-level apply path
//! [`apply_local_signed_group_op`] — i.e. what a peer receiving the
//! cascade op via gossip executes locally. The actor-side dispatch
//! (`handlers/upgrade_group.rs::dispatch_cascade`) is not exercised
//! here: it sits one layer above and requires a full actor context.
//! The cascade engine's *behaviour* lives in `cascade::walk_for_predicate`
//! + the apply arm in `calimero_governance_store::apply_group_op_mutations`, and that
//!   is exactly what `apply_local_signed_group_op` drives.

use calimero_governance_store::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::sync::Arc;

use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::apply_local_signed_group_op;
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

fn app_id_1() -> ApplicationId {
    ApplicationId::from([0xAA; 32])
}
fn app_id_2() -> ApplicationId {
    ApplicationId::from([0xBB; 32])
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
    }
}

/// Create a group at `gid` with `admin` as direct admin (so the
/// cascade arm's per-descendant `MANAGE_APPLICATION` pre-scan passes
/// on every node in the walk) on `bytecode_id`+`target_application_id`.
/// `admin` is a signing KEY; it is enrolled here so the account the governance
/// rows name is the one that key resolves to. Deriving an account from the key
/// instead would compile and key the rows to a principal nothing resolves to.
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

#[test]
fn cascade_upgrade_updates_all_matching_descendants_and_skips_sibling_namespace() {
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let store = empty_store();

    // Namespace R: root + R/A + R/B + R/B/B1, all on BYTECODE_ID_1 / APP_ID_1.
    let r = ContextGroupId::from([0x70; 32]);
    let r_a = ContextGroupId::from([0xA1; 32]);
    let r_b = ContextGroupId::from([0xB1; 32]);
    let r_b_b1 = ContextGroupId::from([0xB2; 32]);

    create_group(&store, &r, admin_pk, BYTECODE_ID_1, app_id_1());
    create_group(&store, &r_a, admin_pk, BYTECODE_ID_1, app_id_1());
    create_group(&store, &r_b, admin_pk, BYTECODE_ID_1, app_id_1());
    create_group(&store, &r_b_b1, admin_pk, BYTECODE_ID_1, app_id_1());

    NamespaceRepository::new(&store).nest(&r, &r_a).unwrap();
    NamespaceRepository::new(&store).nest(&r, &r_b).unwrap();
    NamespaceRepository::new(&store)
        .nest(&r_b, &r_b_b1)
        .unwrap();

    // Sibling namespace S: completely separate root with one child.
    // Same BYTECODE_ID_1 as R so we prove the cascade's tree-walk is what
    // contains the blast radius (not a global bytecode_id sweep).
    let s = ContextGroupId::from([0x50; 32]);
    let s_x = ContextGroupId::from([0x51; 32]);

    create_group(&store, &s, admin_pk, BYTECODE_ID_1, app_id_1());
    create_group(&store, &s_x, admin_pk, BYTECODE_ID_1, app_id_1());
    NamespaceRepository::new(&store).nest(&s, &s_x).unwrap();

    // Sanity: every group starts on (BYTECODE_ID_1, APP_ID_1).
    for gid in [&r, &r_a, &r_b, &r_b_b1, &s, &s_x] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("meta");
        assert_eq!(m.bytecode_id, BYTECODE_ID_1);
        assert_eq!(m.target_application_id, app_id_1());
    }

    // Cascade op signed on R, targeting from_bytecode_id=K1, new bytecode_id=K2 + new target=APP_ID_2.
    let cascade_op = SignedGroupOp::sign(
        &admin_sk,
        r.to_bytes().into(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: BYTECODE_ID_1.into(),
            bytecode_id: BYTECODE_ID_2.into(),
            target_application_id: app_id_2(),
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
        },
    )
    .expect("sign CascadeUpgrade");

    // Apply must succeed (i.e. `apply_group_op_mutations` returns
    // `Ok((true, _))`). A `false` (variant-not-handled) return would
    // make `apply_local_signed_group_op` bail with "unsupported group
    // op variant for local apply" — failing this assertion.
    apply_local_signed_group_op(&store, &cascade_op).expect("cascade op applies cleanly");

    // Every group under R must now be on (BYTECODE_ID_2, APP_ID_2).
    for gid in [&r, &r_a, &r_b, &r_b_b1] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("meta after");
        assert_eq!(
            m.bytecode_id,
            BYTECODE_ID_2,
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
    // descendants of R only, not "every group with bytecode_id == K1".
    for gid in [&s, &s_x] {
        let m = MetaRepository::new(&store)
            .load(gid)
            .unwrap()
            .expect("sibling meta");
        assert_eq!(
            m.bytecode_id,
            BYTECODE_ID_1,
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
