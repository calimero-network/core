use super::{
    CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository,
    MetadataRepository, NamespaceRepository, SigningKeysRepository, UpgradeLadderRepository,
    UpgradesRepository,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
use calimero_store::Store;

use super::test_fixtures::{
    dummy_member_removed_op, nest_for_test, nest_for_test_unchecked, sample_meta_with_admin,
    test_group_id, test_meta, test_store,
};
use super::*;

// -----------------------------------------------------------------------
// Group meta tests
// -----------------------------------------------------------------------

#[test]
fn save_load_delete_group_meta() {
    let store = test_store();
    let gid = test_group_id();
    let meta = test_meta();

    assert!(MetaRepository::new(&store).load(&gid).unwrap().is_none());

    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    let loaded = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
    assert_eq!(loaded.app_key, meta.app_key);
    assert_eq!(loaded.target_application_id, meta.target_application_id);

    MetaRepository::new(&store).delete(&gid).unwrap();
    assert!(MetaRepository::new(&store).load(&gid).unwrap().is_none());
}

// -----------------------------------------------------------------------
// Member tests
// -----------------------------------------------------------------------

#[test]
fn permission_checker_enforces_admin_and_capability_rules() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x10; 32]);
    let member = PublicKey::from([0x11; 32]);
    let outsider = PublicKey::from([0x12; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let checker = PermissionChecker::new(&store, gid);
    assert!(checker.require_admin(&admin).is_ok());
    assert!(checker.require_admin(&member).is_err());

    assert!(checker
        .require_manage_members(&admin, "manage members")
        .is_ok());
    assert!(checker
        .require_manage_members(&member, "manage members")
        .is_err());
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            calimero_context_config::MemberCapabilities::MANAGE_MEMBERS.bits(),
        )
        .unwrap();
    assert!(checker
        .require_manage_members(&member, "manage members")
        .is_ok());

    assert!(checker.require_can_create_context(&admin).is_ok());
    assert!(checker.require_can_create_context(&member).is_err());
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .unwrap();
    assert!(checker.require_can_create_context(&member).is_ok());

    assert!(checker.require_admin_or_self(&member, &member).is_ok());
    assert!(checker.require_admin_or_self(&member, &outsider).is_err());
    assert!(checker.require_admin_or_self(&admin, &outsider).is_ok());
}

#[test]
fn group_settings_service_enforces_permissions_and_persists_values() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x21; 32]);
    let member = PublicKey::from([0x22; 32]);
    let app_id = ApplicationId::from([0x23; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();

    let settings = GroupSettingsService::new(&store, gid);

    assert!(settings
        .set_subgroup_visibility(&member, calimero_context_config::VisibilityMode::Restricted)
        .is_err());
    settings
        .set_subgroup_visibility(&admin, calimero_context_config::VisibilityMode::Restricted)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        calimero_context_config::VisibilityMode::Restricted
    );

    settings.set_default_capabilities(&admin, 0b101).unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&gid)
            .unwrap(),
        Some(0b101)
    );

    assert!(settings
        .set_group_migration(&member, &Some(vec![1, 2, 3]))
        .is_err());
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            calimero_context_config::MemberCapabilities::MANAGE_APPLICATION.bits(),
        )
        .unwrap();
    settings
        .set_group_migration(&member, &Some(vec![1, 2, 3]))
        .unwrap();
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .migration,
        Some(vec![1, 2, 3])
    );

    settings
        .set_target_application(&member, &[0xAB; 32], &app_id)
        .unwrap();
    let meta = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
    assert_eq!(meta.app_key, [0xAB; 32]);
    assert_eq!(meta.target_application_id, app_id);

    MetadataRepository::new(&store)
        .set_group(
            &gid,
            &calimero_primitives::metadata::MetadataRecord {
                name: Some("group-main".to_owned()),
                data: Default::default(),
                updated_at: 0,
                updated_by: [1_u8; 32].into(),
            },
        )
        .unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .group_metadata(&gid)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("group-main")
    );
}

#[test]
fn set_target_application_appends_upgrade_ladder_rung() {
    // The ladder is captured at the single choke point every upgrade op
    // passes through, in the order targets were applied; re-applying the
    // same target (op replay) must not double a rung.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x21; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();

    let settings = GroupSettingsService::new(&store, gid);
    let app_v2 = ApplicationId::from([0xD2; 32]);
    let app_v3 = ApplicationId::from([0xD3; 32]);

    settings
        .set_target_application(&admin, &[0x02; 32], &app_v2)
        .unwrap();
    settings
        .set_target_application(&admin, &[0x02; 32], &app_v2)
        .unwrap();
    settings
        .set_target_application(&admin, &[0x03; 32], &app_v3)
        .unwrap();

    let rungs = UpgradeLadderRepository::new(&store).load(&gid).unwrap();
    assert_eq!(rungs.len(), 2);
    assert_eq!(rungs[0].app_key, [0x02; 32]);
    assert_eq!(rungs[0].application_id, app_v2);
    assert_eq!(rungs[1].app_key, [0x03; 32]);
    assert_eq!(rungs[1].application_id, app_v3);
}

#[test]
fn context_registration_service_applies_backfill_and_detach_rules() {
    let store = test_store();
    let gid = test_group_id();
    let other_gid = ContextGroupId::from([0x31; 32]);
    let admin = PublicKey::from([0x32; 32]);
    let creator = PublicKey::from([0x33; 32]);
    let context = ContextId::from([0x34; 32]);
    let app_id = ApplicationId::from([0x35; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &creator, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &creator,
            calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .unwrap();

    let mut meta = test_meta();
    meta.target_application_id = calimero_primitives::application::ZERO_APPLICATION_ID;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();

    // Pre-store context meta with zero app id to verify backfill path.
    let zero_app = calimero_primitives::application::ZERO_APPLICATION_ID;
    let ctx_meta_key = calimero_store::key::ContextMeta::new(context);
    let mut handle = store.handle();
    handle
        .put(
            &ctx_meta_key,
            &calimero_store::types::ContextMeta::new(
                calimero_store::key::ApplicationMeta::new(zero_app),
                [0x44; 32],
                vec![[0x45; 32]],
                None,
            ),
        )
        .unwrap();
    drop(handle);

    let service = ContextRegistrationService::new(&store, gid);
    let permissions = PermissionChecker::new(&store, gid);

    assert!(service
        .register(
            &permissions,
            &PublicKey::from([0x36; 32]),
            &context,
            &app_id
        )
        .is_err());
    service
        .register(&permissions, &creator, &context, &app_id)
        .unwrap();
    assert_eq!(get_group_for_context(&store, &context).unwrap(), Some(gid));
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .target_application_id,
        app_id
    );
    let handle = store.handle();
    let ctx_meta: calimero_store::types::ContextMeta = handle.get(&ctx_meta_key).unwrap().unwrap();
    assert_eq!(ctx_meta.application.application_id(), app_id);

    assert!(service.detach(&permissions, &creator, &context).is_err());
    service.detach(&permissions, &admin, &context).unwrap();
    assert_eq!(get_group_for_context(&store, &context).unwrap(), None);

    register_context_in_group(&store, &other_gid, &context).unwrap();
    assert!(service.detach(&permissions, &admin, &context).is_err());
}

#[test]
fn context_tree_service_register_move_detach_and_cascade_cleanup() {
    let store = test_store();
    let gid_a = ContextGroupId::from([0x31; 32]);
    let gid_b = ContextGroupId::from([0x32; 32]);
    let context = ContextId::from([0x33; 32]);
    let member = PublicKey::from([0x34; 32]);

    let tree_a = ContextTreeService::new(&store, gid_a);
    let tree_b = ContextTreeService::new(&store, gid_b);

    tree_a.register_context(&context).unwrap();
    assert_eq!(tree_a.group_for_context(&context).unwrap(), Some(gid_a));

    // Moving registration to another group should clean the old group index.
    tree_b.register_context(&context).unwrap();
    assert_eq!(tree_b.group_for_context(&context).unwrap(), Some(gid_b));
    assert!(tree_a.enumerate_contexts(0, usize::MAX).unwrap().is_empty());
    assert_eq!(
        tree_b.enumerate_contexts(0, usize::MAX).unwrap(),
        vec![context]
    );

    let mut handle = store.handle();
    handle
        .put(
            &calimero_store::key::ContextIdentity::new(context, member),
            &calimero_store::types::ContextIdentity {
                private_key: None,
                sender_key: Some([0u8; 32]),
            },
        )
        .unwrap();
    drop(handle);

    tree_b.cascade_remove_member(&member).unwrap();
    let handle = store.handle();
    let identity_key = calimero_store::key::ContextIdentity::new(context, member);
    assert!(!handle.has(&identity_key).unwrap());

    tree_b.unregister_context(&context).unwrap();
    assert_eq!(tree_b.group_for_context(&context).unwrap(), None);
}

#[test]
fn context_registration_service_keeps_existing_non_zero_context_meta_application() {
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x41; 32]);
    let context = ContextId::from([0x42; 32]);
    let existing_app_id = ApplicationId::from([0x43; 32]);
    let incoming_app_id = ApplicationId::from([0x44; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &creator, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &creator,
            calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .unwrap();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();

    let ctx_meta_key = calimero_store::key::ContextMeta::new(context);
    let mut handle = store.handle();
    handle
        .put(
            &ctx_meta_key,
            &calimero_store::types::ContextMeta::new(
                calimero_store::key::ApplicationMeta::new(existing_app_id),
                [0x55; 32],
                vec![[0x56; 32]],
                None,
            ),
        )
        .unwrap();
    drop(handle);

    let service = ContextRegistrationService::new(&store, gid);
    let permissions = PermissionChecker::new(&store, gid);
    service
        .register(&permissions, &creator, &context, &incoming_app_id)
        .unwrap();

    let handle = store.handle();
    let ctx_meta: calimero_store::types::ContextMeta = handle.get(&ctx_meta_key).unwrap().unwrap();
    assert_eq!(ctx_meta.application.application_id(), existing_app_id);
}

/// Re-applying the same op (e.g. a node's own published op coming back via
/// sync backfill, which the in-memory `DagStore` dedup set doesn't cover)
/// must not append `delta_id` to the head set a second time — a head set
/// with duplicates makes `compute_governance_position` refuse to embed a
/// position and every peer then rejects the node's deltas (#2327).
/// A head set that is *already* corrupted with duplicates (older build, or a
/// not-yet-found path) must self-heal: the next `advance_dag_head` collapses
/// the duplicates, and `read_head_record` de-dups defensively on read.
#[test]
fn apply_local_signed_group_op_nonce_and_admin() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member_pk = PrivateKey::random(&mut rng).public_key();

    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op1).unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &member_pk)
        .unwrap());

    let op_dup_nonce =
        SignedGroupOp::sign(&admin_sk, gid_bytes.into(), vec![], 1, GroupOp::Noop).unwrap();
    assert!(
        apply_local_signed_group_op(&store, &op_dup_nonce).is_ok(),
        "duplicate nonce should be silently accepted (idempotent)"
    );

    let op2 = SignedGroupOp::sign(&admin_sk, gid_bytes.into(), vec![], 2, GroupOp::Noop).unwrap();
    apply_local_signed_group_op(&store, &op2).unwrap();

    let non_admin_sk = PrivateKey::random(&mut rng);
    MembershipRepository::new(&store)
        .add_member(&gid, &non_admin_sk.public_key(), GroupMemberRole::Member)
        .unwrap();
    let op_bad = SignedGroupOp::sign(
        &non_admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: PrivateKey::random(&mut rng).public_key(),
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
}

/// Regression test for #2516: two concurrent same-signer governance ops are
/// DAG siblings with consecutive nonces, so causal delivery imposes no order
/// between them and they can land in either order. The old `nonce <= last`
/// high-water-mark guard advanced on whichever higher nonce arrived first and
/// then dropped the lower-nonce sibling forever (`lower <= last`). The
/// windowed guard parks the higher nonce above the contiguous floor and still
/// applies the lower one when it arrives — both ops land, and the floor closes
/// the gap.
#[test]
fn apply_local_signed_group_op_out_of_order_siblings_2516() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    // Group meta is needed for the author-mint assertion at the end
    // (`sign_apply_local_group_op_borsh` computes a state hash over it).
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member_high = PrivateKey::random(&mut rng).public_key();
    let member_low = PrivateKey::random(&mut rng).public_key();

    // The HIGHER-nonce sibling (nonce 2) is delivered first.
    let op_high = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        2,
        GroupOp::MemberAdded {
            member: member_high,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_high).unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &member_high)
        .unwrap());

    // Floor cannot advance past the missing nonce 1; the applied nonce sits
    // in the above-floor set.
    let window = load_nonce_window(&store, &gid, &admin_pk).unwrap();
    assert_eq!(window.floor(), 0, "floor stuck behind the missing nonce 1");
    assert!(window.contains(2), "nonce 2 recorded above the floor");

    // The LOWER-nonce sibling (nonce 1) is delivered second. The old guard
    // would have dropped it as `1 <= last(=2)`; the window applies it.
    let op_low = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: member_low,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_low).unwrap();
    assert!(
        MembershipRepository::new(&store)
            .is_member(&gid, &member_low)
            .unwrap(),
        "lower-nonce sibling must NOT be dropped (the #2516 bug)"
    );

    // Both ops are durably logged and the gap has closed: floor == 2.
    assert_eq!(read_op_log_after(&store, &gid, 0, 10).unwrap().len(), 2);
    assert_eq!(
        get_local_gov_nonce(&store, &gid, &admin_pk).unwrap(),
        Some(2)
    );

    // Replays of either sibling are deduped — no third log entry.
    apply_local_signed_group_op(&store, &op_high).unwrap();
    apply_local_signed_group_op(&store, &op_low).unwrap();
    assert_eq!(
        read_op_log_after(&store, &gid, 0, 10).unwrap().len(),
        2,
        "replayed siblings must be deduped"
    );

    // The next op this node authors mints `max_applied + 1` (== 3), never
    // reusing a nonce already inside the window.
    let out = sign_apply_local_group_op_borsh(&store, &gid, &admin_sk, GroupOp::Noop).unwrap();
    let next = borsh::from_slice::<SignedGroupOp>(&out.bytes)
        .unwrap()
        .nonce;
    assert_eq!(next, 3, "author mints above the highest applied nonce");
}

/// The local-apply path (`apply_local_signed_group_op`, run under
/// `governance_dag` on DAG-delta application) must dedup op-log appends by
/// persisted content hash, matching the namespace receive path. This covers
/// the narrow crash window in `store_nonce_window` where an applied
/// above-floor nonce is lost from the persisted window and the op is then
/// re-delivered via DAG replay: the nonce guard no longer short-circuits it,
/// but the content-hash dedup must still prevent a duplicate op-log entry.
///
/// Uses a real MUTATING op (`MemberAdded`), not `Noop`, so it also exercises
/// the replay-safety contract: `apply_group_op_mutations` re-runs on replay
/// (before the content-hash dedup fires) and must NOT error — `add_member` is
/// an idempotent upsert, so the re-applied op succeeds and the window is
/// re-persisted rather than leaving the node stuck on the nonce.
#[test]
fn apply_local_signed_group_op_replay_does_not_duplicate_log_entry() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member = PrivateKey::random(&mut rng).public_key();
    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();

    apply_local_signed_group_op(&store, &op).unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &member)
        .unwrap());
    assert_eq!(read_op_log_after(&store, &gid, 0, 10).unwrap().len(), 1);
    assert_eq!(
        get_local_gov_nonce(&store, &gid, &admin_pk).unwrap(),
        Some(1)
    );

    // Simulate the lost-window crash: roll the persisted floor back to 0 so
    // the nonce guard no longer dedups op 1 on re-delivery.
    set_local_gov_nonce(&store, &gid, &admin_pk, 0).unwrap();

    // Re-deliver the same op (DAG replay). It now passes the nonce guard and
    // re-runs the (idempotent) mutation, but the content-hash dedup must skip
    // the append — no error, no duplicate entry.
    apply_local_signed_group_op(&store, &op).unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &member)
        .unwrap());
    assert_eq!(
        read_op_log_after(&store, &gid, 0, 10).unwrap().len(),
        1,
        "replayed op must NOT append a duplicate op-log entry"
    );
    // The window is re-advanced, so a third delivery dedups at the guard.
    assert_eq!(
        get_local_gov_nonce(&store, &gid, &admin_pk).unwrap(),
        Some(1)
    );
}

/// The full window (floor + above-set) round-trips through the single-key
/// atomic store, including the out-of-order above-floor nonces.
#[test]
fn nonce_window_round_trips_through_single_key() {
    use crate::nonce_window::NonceWindow;

    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x7Bu8; 32]);

    let mut window = NonceWindow::new(4, []);
    assert!(window.record(6));
    assert!(window.record(8));
    store_nonce_window(&store, &gid, &signer, &window).unwrap();

    let reloaded = load_nonce_window(&store, &gid, &signer).unwrap();
    assert_eq!(reloaded, window, "full window survives store + load");
    assert_eq!(reloaded.floor(), 4);
    assert_eq!(reloaded.above().collect::<Vec<_>>(), vec![6, 8]);
    // get_local_gov_nonce reads the floor out of the same authoritative key.
    assert_eq!(get_local_gov_nonce(&store, &gid, &signer).unwrap(), Some(4));
}

/// Migration: a pre-window database has only the legacy `GroupLocalGovNonce`
/// floor. Both readers migrate from it, and the first `store_nonce_window`
/// makes the window key authoritative (the stale legacy floor is then ignored).
#[test]
fn nonce_window_migrates_from_legacy_floor_key() {
    use calimero_store::key::GroupLocalGovNonce;

    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x7Au8; 32]);

    // Simulate a pre-window DB: only the legacy single-`u64` high-water mark.
    {
        let mut handle = store.handle();
        handle
            .put(&GroupLocalGovNonce::new(gid.to_bytes(), signer), &7u64)
            .unwrap();
    }

    // Both readers migrate the legacy floor.
    assert_eq!(get_local_gov_nonce(&store, &gid, &signer).unwrap(), Some(7));
    let mut window = load_nonce_window(&store, &gid, &signer).unwrap();
    assert_eq!(window.floor(), 7);
    assert!(window.contains(7));
    assert!(!window.contains(8));

    // Recording an out-of-order nonce (gap at 8) persists the full window under
    // the new key; the next load reads it, not the stale legacy floor.
    assert!(window.record(9));
    store_nonce_window(&store, &gid, &signer, &window).unwrap();

    let reloaded = load_nonce_window(&store, &gid, &signer).unwrap();
    assert_eq!(reloaded.floor(), 7);
    assert!(
        reloaded.contains(9),
        "above-floor nonce survived via the authoritative window key"
    );
    assert_eq!(reloaded.max_applied(), 9);
}

#[test]
fn reject_read_only_tee_via_member_added() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: tee_pk,
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::ReadOnlyTeeViaAttestationOnly)
        ),
        "expected ReadOnlyTeeViaAttestationOnly, got: {err}"
    );
}

#[test]
fn reject_read_only_tee_via_member_role_set() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberRoleSet {
            member: member_pk,
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::ReadOnlyTeeViaAttestationOnly)
        ),
        "expected ReadOnlyTeeViaAttestationOnly, got: {err}"
    );
}

#[test]
fn apply_local_member_alias_member_signer_or_admin() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedGroupOp::sign(
        &member_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberMetadataSet {
            member: member_pk,
            name: Some("alice".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .member_metadata(&gid, &member_pk)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("alice")
    );

    let other_sk = PrivateKey::random(&mut rng);
    let op_bad = SignedGroupOp::sign(
        &other_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberMetadataSet {
            member: member_pk,
            name: Some("bob".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());

    let admin_op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberMetadataSet {
            member: member_pk,
            name: Some("carol".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &admin_op).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .member_metadata(&gid, &member_pk)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("carol")
    );
}

#[test]
fn apply_local_context_alias_admin_or_creator() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let creator_sk = PrivateKey::random(&mut rng);
    let creator_pk = creator_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &creator_pk, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &creator_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .unwrap();

    let context_id = ContextId::from([0x33; 32]);

    let op_reg = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextRegistered {
            context_id,
            application_id: calimero_primitives::application::ApplicationId::from([0u8; 32]),
            blob_id: calimero_primitives::blobs::BlobId::from([0u8; 32]),
            source: String::new(),
            service_name: None,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_reg).unwrap();

    let op_creator_alias = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes.into(),
        vec![],
        2,
        GroupOp::ContextMetadataSet {
            context_id,
            name: Some("from-creator".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    assert!(
        apply_local_signed_group_op(&store, &op_creator_alias).is_err(),
        "non-admin creator without CAN_MANAGE_METADATA should be rejected"
    );

    let op_admin = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextMetadataSet {
            context_id,
            name: Some("from-admin".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_admin).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .context_metadata(&gid, &context_id)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("from-admin")
    );
}

#[test]
fn apply_local_signed_group_op_capabilities_upgrade_policy_and_delete() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    // GroupDelete is now Owner-only; align the meta's owner_identity with
    // the signing key so the delete at the end passes the owner gate.
    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    meta.owner_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let member_m = PrivateKey::random(&mut rng).public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_m, GroupMemberRole::Member)
        .unwrap();

    let op_caps = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberCapabilitySet {
            member: member_m,
            capabilities: calimero_context_config::MemberCapabilities::from_bits_truncate(0x7),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_caps).unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .member_capability(&gid, &member_m)
            .unwrap()
            .unwrap(),
        0x7
    );

    let op_policy = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        2,
        GroupOp::UpgradePolicySet {
            policy: UpgradePolicy::Automatic,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_policy).unwrap();
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .upgrade_policy,
        UpgradePolicy::Automatic
    );

    let op_del =
        SignedGroupOp::sign(&admin_sk, gid_bytes.into(), vec![], 3, GroupOp::GroupDelete).unwrap();
    apply_local_signed_group_op(&store, &op_del).unwrap();
    assert!(MetaRepository::new(&store).load(&gid).unwrap().is_none());
}

#[test]
fn apply_local_signed_group_op_rejects_last_admin_removal() {
    use calimero_context_client::local_governance::SignedGroupOp;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    // Founder == sole admin (no other admin to count), owner kept distinct so
    // the last-admin guard fires, not owner-immunity.
    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let op_bad = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        dummy_member_removed_op(admin_pk),
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
}

// -----------------------------------------------------------------------
// Governance-op rejection paths
//
// These pin the authorization gates of the per-op apply handlers
// (`ops/group/transfer_ownership.rs`, `context_capability_{granted,
// revoked}.rs`). There is no top-level admin gate in
// `apply_local_signed_group_op` — every op carries its own check — so
// these tests drive the full signed-op path and assert both that the
// op is rejected AND that it is rejected for the intended reason.
// -----------------------------------------------------------------------

/// `TransferOwnership` is owner-only. An admin who is *not* the current
/// owner cannot transfer ownership, even though they otherwise pass
/// every member-management gate.
#[test]
fn transfer_ownership_rejects_non_owner_signer() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    // A second admin — privileged, but not the owner.
    let other_admin_sk = PrivateKey::random(&mut rng);
    let other_admin_pk = other_admin_sk.public_key();
    // A third admin standing by as a valid successor, so the op fails on
    // the owner check rather than the new-owner-role check.
    let successor_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &owner_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &other_admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &successor_pk, GroupMemberRole::Admin)
        .unwrap();

    let op = SignedGroupOp::sign(
        &other_admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::TransferOwnership {
            new_owner: successor_pk,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::OnlyOwnerCanTransfer(_))
        ),
        "expected OnlyOwnerCanTransfer, got: {err}"
    );
    // Ownership is unchanged.
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .owner_identity,
        owner_pk
    );
}

/// `TransferOwnership` requires the successor to already be an Admin.
/// Transferring to a plain Member is rejected (the handler refuses to
/// create an "owner with reduced capabilities" state).
#[test]
fn transfer_ownership_rejects_new_owner_not_admin() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    let plain_member_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &owner_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &plain_member_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedGroupOp::sign(
        &owner_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::TransferOwnership {
            new_owner: plain_member_pk,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::TransferTargetNotAdmin {
                role: GroupMemberRole::Member,
                ..
            })
        ),
        "expected TransferTargetNotAdmin(Member), got: {err}"
    );
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .owner_identity,
        owner_pk
    );
}

/// `TransferOwnership` rejects a successor who is not a member of the
/// group at all (would otherwise create an absentee owner).
#[test]
fn transfer_ownership_rejects_new_owner_not_member() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    let outsider_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &owner_pk, GroupMemberRole::Admin)
        .unwrap();

    let op = SignedGroupOp::sign(
        &owner_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::TransferOwnership {
            new_owner: outsider_pk,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::TransferTargetNotMember(_))
        ),
        "expected TransferTargetNotMember, got: {err}"
    );
    assert_eq!(
        MetaRepository::new(&store)
            .load(&gid)
            .unwrap()
            .unwrap()
            .owner_identity,
        owner_pk
    );
}

/// `TransferOwnership` must move the meta `admin_identity` pin to the
/// successor, not just `owner_identity`. `is_admin` honors
/// `meta.admin_identity` as an always-admin that no member-row change can
/// revoke, so leaving it on the old owner would grant the former owner
/// permanent, unrevokable admin after handover. This pins the fix: after a
/// transfer the former owner's admin authority is only as durable as their
/// member row, so removing that row revokes their admin entirely.
#[test]
fn transfer_ownership_moves_admin_identity_to_new_owner() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    // Genesis shape: creator is owner_identity == admin_identity, with an
    // explicit Admin member row (mirrors `GroupCreated`/namespace genesis).
    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();
    let successor_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(owner_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &owner_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &successor_pk, GroupMemberRole::Admin)
        .unwrap();

    let op = SignedGroupOp::sign(
        &owner_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::TransferOwnership {
            new_owner: successor_pk,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();

    // Both pins moved to the successor.
    let meta = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
    assert_eq!(meta.owner_identity, successor_pk, "owner moved");
    assert_eq!(meta.admin_identity, successor_pk, "admin pin moved");

    // The former owner's admin is now backed solely by their (revokable)
    // member row — removing it strips their admin entirely. Before the fix
    // the lingering `admin_identity` pin would keep them admin forever.
    MembershipRepository::new(&store)
        .remove_member(&gid, &owner_pk)
        .unwrap();
    assert!(
        !MembershipRepository::new(&store)
            .is_admin(&gid, &owner_pk)
            .unwrap(),
        "former owner must lose admin once their member row is removed"
    );
    // The successor remains admin (member row + meta pin).
    assert!(
        MembershipRepository::new(&store)
            .is_admin(&gid, &successor_pk)
            .unwrap(),
        "successor must still be admin after the transfer"
    );
}

/// `ContextCapabilityGranted` is gated by `require_manage_members`. A
/// plain member without the `MANAGE_MEMBERS` capability (and not an
/// admin) cannot grant a context capability — and the grant must not be
/// written.
#[test]
fn context_capability_granted_rejects_unauthorized_signer() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let context_id = ContextId::from([0x44; 32]);
    let target_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedGroupOp::sign(
        &member_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextCapabilityGranted {
            context_id,
            member: target_pk,
            capability: calimero_governance_types::ContextCapabilityBits::new(0b1)
                .expect("capability bitmask is non-zero"),
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<CapabilitiesError>(),
            Some(CapabilitiesError::Unauthorized { operation, .. })
                if operation == "grant context capability"
        ),
        "expected Unauthorized(grant context capability), got: {err}"
    );
    // Nothing was granted.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_id, &target_pk)
            .unwrap(),
        None
    );
}

/// `ContextCapabilityRevoked` shares the same `require_manage_members`
/// gate. A plain member cannot revoke a context capability, and an
/// existing grant must survive the rejected op untouched.
#[test]
fn context_capability_revoked_rejects_unauthorized_signer() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let context_id = ContextId::from([0x55; 32]);
    let target_pk = PrivateKey::random(&mut rng).public_key();

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();
    // Pre-existing grant that the unauthorized revoke must not disturb.
    CapabilitiesRepository::new(&store)
        .set_context_member(&gid, &context_id, &target_pk, 0b11)
        .unwrap();

    let op = SignedGroupOp::sign(
        &member_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member: target_pk,
            capability: calimero_governance_types::ContextCapabilityBits::new(0b1)
                .expect("capability bitmask is non-zero"),
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<CapabilitiesError>(),
            Some(CapabilitiesError::Unauthorized { operation, .. })
                if operation == "revoke context capability"
        ),
        "expected Unauthorized(revoke context capability), got: {err}"
    );
    // The grant is untouched — the rejected op wrote nothing.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_id, &target_pk)
            .unwrap(),
        Some(0b11)
    );
}

/// `ContextCapabilityGranted` must refuse to write a per-context capability
/// row for a context that is not registered in this group. An authorized
/// signer (admin) clears the `require_manage_members` gate but still hits the
/// context↔group guard — the same orphan-row hazard the grant handler guards
/// against. Nothing may be written.
#[test]
fn context_capability_granted_rejects_context_not_in_group() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    // Grantee is a member, so the op fails on the context guard rather than
    // the grantee-membership guard.
    let target_sk = PrivateKey::random(&mut rng);
    let target_pk = target_sk.public_key();
    // Never registered in `gid` (nor any group).
    let context_id = ContextId::from([0x66; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target_pk, GroupMemberRole::Member)
        .unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextCapabilityGranted {
            context_id,
            member: target_pk,
            capability: calimero_governance_types::ContextCapabilityBits::new(0b1)
                .expect("capability bitmask is non-zero"),
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ContextRegistrationError>(),
            Some(ContextRegistrationError::NotInGroup { .. })
        ),
        "expected NotInGroup, got: {err}"
    );
    // No per-context row was written.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_id, &target_pk)
            .unwrap(),
        None
    );
}

/// `ContextCapabilityGranted` requires the grantee to be a DIRECT member of
/// the group. An authorized signer granting to a non-member (even for a
/// context correctly registered in the group) is rejected — without this a
/// `manage_members` signer could write an orphan capability row for an
/// arbitrary identity the enumeration/authorization paths never reconcile.
#[test]
fn context_capability_granted_rejects_non_member_grantee() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    // Never added as a member of `gid`.
    let outsider_pk = PrivateKey::random(&mut rng).public_key();
    let context_id = ContextId::from([0x77; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    // Context is properly registered in this group, so the op reaches the
    // grantee-membership guard.
    register_context_in_group(&store, &gid, &context_id).unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextCapabilityGranted {
            context_id,
            member: outsider_pk,
            capability: calimero_governance_types::ContextCapabilityBits::new(0b1)
                .expect("capability bitmask is non-zero"),
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::NotMember { .. })
        ),
        "expected NotMember, got: {err}"
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_id, &outsider_pk)
            .unwrap(),
        None
    );
}

/// `ContextCapabilityRevoked` mirrors the grant path's context↔group guard:
/// an authorized signer cannot touch a per-context row for a context that is
/// not registered in this group, even though revoke is otherwise lenient
/// about grantee membership.
#[test]
fn context_capability_revoked_rejects_context_not_in_group() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let target_pk = PrivateKey::random(&mut rng).public_key();
    // Never registered in `gid`.
    let context_id = ContextId::from([0x88; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member: target_pk,
            capability: calimero_governance_types::ContextCapabilityBits::new(0b1)
                .expect("capability bitmask is non-zero"),
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ContextRegistrationError>(),
            Some(ContextRegistrationError::NotInGroup { .. })
        ),
        "expected NotInGroup, got: {err}"
    );
}

// -----------------------------------------------------------------------
// Cross-node determinism: same ops applied in different orders on two
// independent stores must converge to the same group state hash. The
// existing `compute_state_hash_is_deterministic` (meta.rs) only proves a
// single store's hash is stable across repeated calls; this proves the
// applied STATE is order-independent, which is the property two nodes
// replaying the same governance DAG in different topological orders rely
// on for convergence.
// -----------------------------------------------------------------------

/// Apply a commuting set of `MemberAdded` ops — each from a distinct admin
/// signer, so they carry no cross-signer nonce ordering constraint — in one
/// order on store A and the reverse order on store B. Both nodes must end at
/// the identical group state hash, and that hash must differ from the
/// pre-apply baseline (guards against a vacuously-equal assertion where no
/// op mutated state).
#[test]
fn cross_node_state_hash_is_order_independent() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    // Three admins, each authorized to add members. Distinct signers means
    // each op is nonce 1 for its own signer, so reordering them across nodes
    // never trips the per-signer nonce window.
    let admin0_sk = PrivateKey::random(&mut rng);
    let admin0_pk = admin0_sk.public_key();
    let admin1_sk = PrivateKey::random(&mut rng);
    let admin1_pk = admin1_sk.public_key();
    let admin2_sk = PrivateKey::random(&mut rng);
    let admin2_pk = admin2_sk.public_key();

    // Three members each op will add. Distinct identities → the ops commute
    // (the member set is the same regardless of insertion order).
    let member_x = PrivateKey::random(&mut rng).public_key();
    let member_y = PrivateKey::random(&mut rng).public_key();
    let member_z = PrivateKey::random(&mut rng).public_key();

    // Identical genesis for both nodes: admin0 is the meta owner/admin, and all
    // three admins hold `Admin` member rows. `MemberAdded` is gated by
    // `require_manage_members`, whose admin check (`is_admin`) passes for
    // anyone with an `Admin` member row — NOT only `meta.admin_identity`. So
    // admin1 and admin2 can each sign a `MemberAdded` op even though only
    // admin0 is the meta admin; the membership assertions after apply confirm
    // all three ops were actually accepted (not silently rejected).
    let bootstrap = |store: &Store| {
        MetaRepository::new(store)
            .save(&gid, &sample_meta_with_admin(admin0_pk))
            .unwrap();
        for admin in [&admin0_pk, &admin1_pk, &admin2_pk] {
            MembershipRepository::new(store)
                .add_member(&gid, admin, GroupMemberRole::Admin)
                .unwrap();
        }
    };
    let store_a = test_store();
    let store_b = test_store();
    bootstrap(&store_a);
    bootstrap(&store_b);

    // Baseline (admins only) — both nodes agree before any op is applied.
    let baseline = MetaRepository::new(&store_a)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        baseline,
        MetaRepository::new(&store_b)
            .compute_state_hash(&gid)
            .unwrap(),
        "identical genesis must yield identical baseline hashes"
    );

    let sign_add = |sk: &PrivateKey, member| {
        SignedGroupOp::sign(
            sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap()
    };
    let op0 = sign_add(&admin0_sk, member_x);
    let op1 = sign_add(&admin1_sk, member_y);
    let op2 = sign_add(&admin2_sk, member_z);

    // Node A applies in one order...
    for op in [&op0, &op1, &op2] {
        apply_local_signed_group_op(&store_a, op).unwrap();
    }
    // ...node B in the reverse order.
    for op in [&op2, &op1, &op0] {
        apply_local_signed_group_op(&store_b, op).unwrap();
    }

    // Every op actually landed on both nodes: assert the concrete member set
    // rather than trusting hash equality alone, so a hash that ignored some
    // members (or an op that silently no-op'd) can't make the test pass
    // vacuously.
    for store in [&store_a, &store_b] {
        let members = MembershipRepository::new(store);
        for member in [member_x, member_y, member_z] {
            assert!(
                members.is_member(&gid, &member).unwrap(),
                "each added member must be present on both nodes after convergence"
            );
        }
    }

    let hash_a = MetaRepository::new(&store_a)
        .compute_state_hash(&gid)
        .unwrap();
    let hash_b = MetaRepository::new(&store_b)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        hash_a, hash_b,
        "same ops in different orders must converge to the same state hash"
    );
    assert_ne!(
        hash_a, baseline,
        "applying MemberAdded ops must actually change the state hash"
    );
}

// -----------------------------------------------------------------------
// Signing key tests
// -----------------------------------------------------------------------

#[test]
fn store_and_get_signing_key() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    assert!(SigningKeysRepository::new(&store)
        .get_key(&gid, &pk)
        .unwrap()
        .is_none());

    SigningKeysRepository::new(&store)
        .store_key(&gid, &pk, &sk)
        .unwrap();
    let loaded = SigningKeysRepository::new(&store)
        .get_key(&gid, &pk)
        .unwrap()
        .unwrap();
    assert_eq!(loaded, sk);
}

#[test]
fn delete_signing_key() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    SigningKeysRepository::new(&store)
        .store_key(&gid, &pk, &sk)
        .unwrap();
    SigningKeysRepository::new(&store)
        .delete_key(&gid, &pk)
        .unwrap();
    assert!(SigningKeysRepository::new(&store)
        .get_key(&gid, &pk)
        .unwrap()
        .is_none());
}

#[test]
fn require_signing_key_fails_when_missing() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);

    assert!(SigningKeysRepository::new(&store)
        .require_key(&gid, &pk)
        .is_err());
}

#[test]
fn delete_all_group_signing_keys_removes_all() {
    let store = test_store();
    let gid = test_group_id();
    let pk1 = PublicKey::from([0x10; 32]);
    let pk2 = PublicKey::from([0x11; 32]);

    SigningKeysRepository::new(&store)
        .store_key(&gid, &pk1, &[0xAA; 32])
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&gid, &pk2, &[0xBB; 32])
        .unwrap();

    SigningKeysRepository::new(&store)
        .delete_all_for_group(&gid)
        .unwrap();

    assert!(SigningKeysRepository::new(&store)
        .get_key(&gid, &pk1)
        .unwrap()
        .is_none());
    assert!(SigningKeysRepository::new(&store)
        .get_key(&gid, &pk2)
        .unwrap()
        .is_none());
}

// -----------------------------------------------------------------------
// Context-group index tests
// -----------------------------------------------------------------------

#[test]
fn register_and_unregister_context() {
    let store = test_store();
    let gid = test_group_id();
    let cid = ContextId::from([0x11; 32]);

    assert!(get_group_for_context(&store, &cid).unwrap().is_none());

    register_context_in_group(&store, &gid, &cid).unwrap();
    assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid);

    unregister_context_from_group(&store, &gid, &cid).unwrap();
    assert!(get_group_for_context(&store, &cid).unwrap().is_none());
}

/// The `join_group` handler registers every context listed in the
/// received `JoinBundle` by calling `register_context_in_group`
/// directly, rather than relying on the bundle's governance-op stream
/// to apply a `ContextRegistered` op. This test pins the invariant:
/// after that direct-register call, `get_group_for_context` resolves
/// the mapping with no governance op applied. Removing the
/// direct-register call from the handler would leave the mapping empty
/// and break every downstream caller that resolves namespace from
/// context (e.g. the unknown-member catch-up on the sync path).
#[test]
fn join_bundle_registration_writes_context_group_ref_without_governance_op() {
    let store = test_store();
    let gid = test_group_id();

    let context_ids = [
        ContextId::from([0x11; 32]),
        ContextId::from([0x22; 32]),
        ContextId::from([0x33; 32]),
    ];

    for cid in &context_ids {
        assert!(
            get_group_for_context(&store, cid).unwrap().is_none(),
            "precondition: no mapping before register",
        );
    }

    // Same call the join handler makes for each context in the bundle.
    for cid in &context_ids {
        register_context_in_group(&store, &gid, cid).unwrap();
    }

    for cid in &context_ids {
        assert_eq!(
            get_group_for_context(&store, cid).unwrap(),
            Some(gid),
            "every bundled context must have its group mapping after join \
             registration, independent of governance-op application",
        );
    }
}

#[test]
fn re_register_context_cleans_old_group() {
    let store = test_store();
    let gid1 = ContextGroupId::from([0x01; 32]);
    let gid2 = ContextGroupId::from([0x02; 32]);
    let cid = ContextId::from([0x11; 32]);

    register_context_in_group(&store, &gid1, &cid).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .count_contexts(&gid1)
            .unwrap(),
        1
    );

    register_context_in_group(&store, &gid2, &cid).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .count_contexts(&gid1)
            .unwrap(),
        0
    );
    assert_eq!(
        MetadataRepository::new(&store)
            .count_contexts(&gid2)
            .unwrap(),
        1
    );
    assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid2);
}

#[test]
fn enumerate_and_count_contexts() {
    let store = test_store();
    let gid = test_group_id();

    for i in 0u8..4 {
        let mut cid_bytes = [0u8; 32];
        cid_bytes[0] = i;
        register_context_in_group(&store, &gid, &ContextId::from(cid_bytes)).unwrap();
    }

    assert_eq!(
        MetadataRepository::new(&store)
            .count_contexts(&gid)
            .unwrap(),
        4
    );

    let page = enumerate_group_contexts(&store, &gid, 1, 2).unwrap();
    assert_eq!(page.len(), 2);
}

// -----------------------------------------------------------------------
// Upgrade tests
// -----------------------------------------------------------------------

#[test]
fn save_load_delete_upgrade() {
    let store = test_store();
    let gid = test_group_id();

    assert!(UpgradesRepository::new(&store)
        .load(&gid)
        .unwrap()
        .is_none());

    let upgrade = GroupUpgradeValue {
        from_version: "1.0.0".to_owned(),
        to_version: "2.0.0".to_owned(),
        migration: None,
        initiated_at: 1_700_000_000,
        initiated_by: PublicKey::from([0x01; 32]),
        status: GroupUpgradeStatus::InProgress {
            total: 5,
            completed: 0,
            failed: 0,
        },
        cascade_hlc: None,
        cascade_seq: None,
    };

    UpgradesRepository::new(&store)
        .save(&gid, &upgrade)
        .unwrap();
    let loaded = UpgradesRepository::new(&store).load(&gid).unwrap().unwrap();
    assert_eq!(loaded.from_version, "1.0.0");
    assert_eq!(loaded.to_version, "2.0.0");

    UpgradesRepository::new(&store).delete(&gid).unwrap();
    assert!(UpgradesRepository::new(&store)
        .load(&gid)
        .unwrap()
        .is_none());
}

#[test]
fn enumerate_in_progress_upgrades_filters_completed() {
    let store = test_store();
    let gid_in_progress = ContextGroupId::from([0x01; 32]);
    let gid_completed = ContextGroupId::from([0x02; 32]);

    UpgradesRepository::new(&store)
        .save(
            &gid_in_progress,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::InProgress {
                    total: 5,
                    completed: 2,
                    failed: 0,
                },
                cascade_hlc: None,
                cascade_seq: None,
            },
        )
        .unwrap();

    UpgradesRepository::new(&store)
        .save(
            &gid_completed,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::Completed {
                    completed_at: Some(1_700_001_000),
                },
                cascade_hlc: None,
                cascade_seq: None,
            },
        )
        .unwrap();

    let results = UpgradesRepository::new(&store)
        .enumerate_in_progress()
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, gid_in_progress);
}

// -----------------------------------------------------------------------
// enumerate_all_groups — prefix guard regression test
// -----------------------------------------------------------------------

/// Regression test: `enumerate_all_groups` must stop at GroupMeta keys and
/// not spill into adjacent GroupMember keys (prefix 0x21).  Before the fix,
/// the function would attempt to deserialise a `GroupMemberRole` value as
/// `GroupMetaValue`, panicking with "failed to fill whole buffer".
#[test]
fn enumerate_all_groups_stops_before_member_keys() {
    let store = test_store();
    let gid = test_group_id();
    let meta = test_meta();
    let member = PublicKey::from([0x10; 32]);

    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    // Add a group member — this writes a GroupMember key (prefix 0x21)
    // into the same column, right after GroupMeta keys (prefix 0x20).
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Admin)
        .unwrap();

    // Must return exactly one group without panicking.
    let groups = MetaRepository::new(&store).enumerate_all(0, 100).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].0, gid.to_bytes());
}

#[test]
fn enumerate_all_groups_multiple_groups_with_members() {
    let store = test_store();
    let gid1 = ContextGroupId::from([0x01; 32]);
    let gid2 = ContextGroupId::from([0x02; 32]);
    let meta = test_meta();

    MetaRepository::new(&store).save(&gid1, &meta).unwrap();
    MetaRepository::new(&store).save(&gid2, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid1, &PublicKey::from([0xAA; 32]), GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid2, &PublicKey::from([0xBB; 32]), GroupMemberRole::Member)
        .unwrap();

    let groups = MetaRepository::new(&store).enumerate_all(0, 100).unwrap();
    assert_eq!(groups.len(), 2);

    // Pagination
    let page = MetaRepository::new(&store).enumerate_all(1, 1).unwrap();
    assert_eq!(page.len(), 1);
}

// -----------------------------------------------------------------------
// extract_application_id — base58 decoding regression test
// -----------------------------------------------------------------------

/// Regression test: `extract_application_id` must decode the `id` field
/// using base58 (via `Repr<ApplicationId>`), not hex.  Before the fix,
/// `hex::decode` was called on a base58 string, producing
/// "Invalid character 'g' at position 1" errors at runtime.
#[test]
fn extract_application_id_decodes_base58() {
    // Repr<[u8; 32]> serialises as base58 (canonical `Repr` serialization for the id field).
    use calimero_context_config::repr::Repr;

    let raw: [u8; 32] = [0xDE; 32];
    let encoded = Repr::new(raw).to_string(); // base58 string

    let json = serde_json::json!({ "id": encoded });
    let result = extract_application_id(&json).unwrap();
    assert_eq!(*result, raw);
}

#[test]
fn extract_application_id_rejects_hex() {
    // A hex string decodes to ~46 bytes via base58, causing a length
    // mismatch against the required 32-byte ApplicationId.
    let hex_str = hex::encode([0xDE; 32]);
    let json = serde_json::json!({ "id": hex_str });
    assert!(extract_application_id(&json).is_err());
}

#[test]
fn extract_application_id_missing_field_returns_error() {
    let json = serde_json::json!({});
    assert!(extract_application_id(&json).is_err());
}

// -----------------------------------------------------------------------
// Member capability tests
// -----------------------------------------------------------------------

#[test]
fn set_and_get_member_capability() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);

    // No capability stored yet
    assert!(CapabilitiesRepository::new(&store)
        .member_capability(&gid, &pk)
        .unwrap()
        .is_none());

    // Set capabilities
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &pk, 0b101)
        .unwrap();
    let caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &pk)
        .unwrap()
        .unwrap();
    assert_eq!(caps, 0b101);

    // Update capabilities
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &pk, 0b111)
        .unwrap();
    let caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &pk)
        .unwrap()
        .unwrap();
    assert_eq!(caps, 0b111);
}

#[test]
fn capability_zero_means_no_permissions() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x11; 32]);

    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &pk, 0)
        .unwrap();
    let caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &pk)
        .unwrap()
        .unwrap();
    assert_eq!(caps, 0);
    // All capability bits are off
    use calimero_context_config::MemberCapabilities;
    assert_eq!(caps & MemberCapabilities::CAN_CREATE_CONTEXT.bits(), 0);
    assert_eq!(caps & MemberCapabilities::CAN_INVITE_MEMBERS.bits(), 0);
    assert_eq!(caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(), 0);
}

#[test]
fn capabilities_isolated_per_member() {
    let store = test_store();
    let gid = test_group_id();
    let alice = PublicKey::from([0x12; 32]);
    let bob = PublicKey::from([0x13; 32]);

    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &alice, 0b001)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &bob, 0b110)
        .unwrap();

    assert_eq!(
        CapabilitiesRepository::new(&store)
            .member_capability(&gid, &alice)
            .unwrap()
            .unwrap(),
        0b001
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .member_capability(&gid, &bob)
            .unwrap()
            .unwrap(),
        0b110
    );
}

// -----------------------------------------------------------------------
// Default capabilities and visibility tests
// -----------------------------------------------------------------------

#[test]
fn set_and_get_default_capabilities() {
    let store = test_store();
    let gid = test_group_id();

    assert!(CapabilitiesRepository::new(&store)
        .default_capabilities(&gid)
        .unwrap()
        .is_none());

    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, 0b100)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&gid)
            .unwrap()
            .unwrap(),
        0b100
    );

    // Update
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, 0b111)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&gid)
            .unwrap()
            .unwrap(),
        0b111
    );
}

#[test]
fn set_and_get_subgroup_visibility() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let gid = test_group_id();

    // Absent key reads as Restricted (the safe default).
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Restricted
    );

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&gid, VisibilityMode::Open)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Open
    );

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&gid, VisibilityMode::Restricted)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Restricted
    );
}

// -----------------------------------------------------------------------
// Parent-chain membership inheritance for `Open` subgroups (issue #2256)
//
// The walk in `check_group_membership` treats `Open` as "inherit from
// parent if anchor cap allows" and `Restricted` (or absent) as a wall.
// These tests pin down the exact semantics, including admin override and
// the deepest-anchor cap-check rule.
// -----------------------------------------------------------------------

/// Tiny helper: link `child` under `parent` directly via the test/legacy
/// `nest_group` helper so we don't need to drive a full RootOp through
/// governance just to set up a tree shape for membership tests.
// -----------------------------------------------------------------------
// Capability-materialization ordering (PR #2368 root cause for the
// `MemberJoinedOpen rejected: no membership path` e2e failure).
//
// `add_group_member` materializes a non-admin member's per-member
// capability row by copying the group's `default_capabilities` — but
// ONLY if those defaults are already set in the store. A member added
// BEFORE the namespace default caps land therefore has no
// `CAN_JOIN_OPEN_SUBGROUPS` bit on this node, and a later
// `MemberJoinedOpen` from them fails `check_group_membership_path`.
// The `join_group` handler now sets `default_capabilities` before the
// catch-up apply so every `MemberJoined` in the batch materializes its
// caps correctly; these two tests pin the underlying invariant.
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// Effective membership for `Open` subgroups — issue #2371
//
// `join_subgroup_inheritance` returns 200 / `wasInherited: true` but
// writes no `GroupMember` row — `execute_member_joined_open` is a
// validate-only apply. `check_group_membership` correctly reports the
// inherited joiner as a member, yet `list_group_members` (which only
// reads stored rows) omits them, so an app keying "is the caller a
// member?" off the member list sees `false`. `enumerate_inherited_members`
// closes the gap: it recomputes inherited members from current ancestor
// state so callers can union it with the stored rows to get the
// effective member set the membership contract promises.
// -----------------------------------------------------------------------

#[test]
fn default_capabilities_include_can_join_open_subgroups() {
    use calimero_context_config::MemberCapabilities;

    // When a group has default capabilities containing
    // CAN_JOIN_OPEN_SUBGROUPS, a newly added non-admin member should
    // automatically get the bit. This is the load-bearing default that
    // makes `Open` subgroups inheritable without per-member admin action.
    let store = test_store();
    let gid = ContextGroupId::from([0x40; 32]);
    let alice = PublicKey::from([0x01; 32]);

    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &alice, GroupMemberRole::Member)
        .unwrap();

    let caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &alice)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits()
    );
}

#[test]
fn is_open_chain_to_namespace_walks_parent_chain_correctly() {
    use calimero_context_config::VisibilityMode;
    // Tree: ns -> mid -> leaf. This is the input shape the
    // visibility-flip encryption special-case in
    // `GroupGovernancePublisher` feeds into when it queries the
    // **parent chain** of a `SubgroupVisibilitySet` op (i.e. it
    // calls `CapabilitiesRepository::new(parent).is_open_chain_to_namespace(ns)` instead of
    // `(self, ns)`). The cases below pin down the contract that
    // path relies on.
    let store = test_store();
    let ns = ContextGroupId::from([0xA0; 32]);
    let mid = ContextGroupId::from([0xA1; 32]);
    let leaf = ContextGroupId::from([0xA2; 32]);
    nest_for_test(&store, &ns, &mid);
    nest_for_test(&store, &mid, &leaf);

    // Identity case: a group is not an "Open chain to itself" — the
    // namespace root has no parent and does not participate in
    // subgroup-style inheritance.
    assert!(!CapabilitiesRepository::new(&store)
        .is_open_chain_to_namespace(&ns, &ns)
        .unwrap());

    // Direct child of the namespace: parent chain trivially Open
    // when `mid` itself is Open.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Open)
        .unwrap();
    assert!(CapabilitiesRepository::new(&store)
        .is_open_chain_to_namespace(&mid, &ns)
        .unwrap());

    // Two-hop chain, all Open → boundary is namespace-wide.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&leaf, VisibilityMode::Open)
        .unwrap();
    assert!(CapabilitiesRepository::new(&store)
        .is_open_chain_to_namespace(&leaf, &ns)
        .unwrap());

    // Restricted wall at mid → boundary is NOT namespace-wide,
    // even if leaf itself is Open.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Restricted)
        .unwrap();
    assert!(!CapabilitiesRepository::new(&store)
        .is_open_chain_to_namespace(&leaf, &ns)
        .unwrap());

    // The visibility-flip publisher special-case calls this with
    // the *parent* of the flipping group — `mid` here, walking up
    // to `ns`. With mid currently Restricted that returns false;
    // re-open mid and confirm we get true.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Open)
        .unwrap();
    assert!(CapabilitiesRepository::new(&store)
        .is_open_chain_to_namespace(&mid, &ns)
        .unwrap());
}

#[test]
fn is_open_chain_to_namespace_bails_on_depth_overflow() {
    use super::namespace::MAX_NAMESPACE_DEPTH;
    use calimero_context_config::VisibilityMode;

    // Build a chain longer than MAX_NAMESPACE_DEPTH so the walk
    // exhausts its bound without finding the namespace. This used
    // to silently return Ok(false); the fix bails so authorization
    // and crypto-key selection both surface the corruption signal.
    let store = test_store();
    let ns = ContextGroupId::from([0xC0; 32]);
    let mut prev = ns;
    for i in 0..(MAX_NAMESPACE_DEPTH + 2) {
        let next = ContextGroupId::from([0xD0u8.wrapping_add(i as u8); 32]);
        nest_for_test_unchecked(&store, &prev, &next);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&next, VisibilityMode::Open)
            .unwrap();
        prev = next;
    }
    // Walking from the deepest node should hit the depth bound
    // before reaching `ns` and return an error rather than
    // Ok(false).
    let res = CapabilitiesRepository::new(&store).is_open_chain_to_namespace(&prev, &ns);
    assert!(
        res.is_err(),
        "is_open_chain_to_namespace must bail on MAX_NAMESPACE_DEPTH overflow, \
         got {res:?}"
    );
}

#[test]
fn default_capabilities_admin_override_propagates_to_new_member() {
    // Issue #2256 / PR #2261 regression: when an admin has overridden
    // the namespace's default capabilities to a non-`CAN_JOIN_OPEN_SUBGROUPS`
    // value, a newly added member should pick up *that* overridden value,
    // not the create-time default. This guards against a hard-coded
    // joiner-side fallback re-introducing itself: if some future change
    // causes `add_group_member_with_keys` to substitute its own constant
    // when the local default is anything other than the create-time one,
    // this test fires.
    let store = test_store();
    let gid = ContextGroupId::from([0x40; 32]);
    let alice = PublicKey::from([0x01; 32]);

    // Admin override: set default to 0 (no caps).
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, 0)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &alice, GroupMemberRole::Member)
        .unwrap();

    // alice should NOT have any capability bits; in particular she
    // should NOT have CAN_JOIN_OPEN_SUBGROUPS just because a hard-coded
    // path snuck it in.
    let caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &alice)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        caps, 0,
        "admin override default=0 should give member caps=0, got {caps}"
    );

    // Symmetric check with a non-zero non-default value.
    let bob = PublicKey::from([0x02; 32]);
    let custom = calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT.bits()
        | calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS.bits();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, custom)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bob, GroupMemberRole::Member)
        .unwrap();
    let bob_caps = CapabilitiesRepository::new(&store)
        .member_capability(&gid, &bob)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        bob_caps, custom,
        "admin override default={custom} should give member caps={custom}, got {bob_caps}"
    );
}

#[test]
fn defaults_isolated_per_group() {
    let store = test_store();
    let g1 = ContextGroupId::from([0x40; 32]);
    let g2 = ContextGroupId::from([0x41; 32]);

    use calimero_context_config::VisibilityMode;

    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&g1, 0b001)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&g2, 0b110)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&g1, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&g2, VisibilityMode::Restricted)
        .unwrap();

    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&g1)
            .unwrap()
            .unwrap(),
        0b001
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .default_capabilities(&g2)
            .unwrap()
            .unwrap(),
        0b110
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&g1)
            .unwrap(),
        VisibilityMode::Open
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&g2)
            .unwrap(),
        VisibilityMode::Restricted
    );
}

#[test]
fn context_member_capability_roundtrip_and_isolation() {
    let store = test_store();
    let gid = test_group_id();
    let context_a = ContextId::from([0x21; 32]);
    let context_b = ContextId::from([0x22; 32]);
    let alice = PublicKey::from([0x31; 32]);
    let bob = PublicKey::from([0x32; 32]);

    assert!(CapabilitiesRepository::new(&store)
        .context_member_capability(&gid, &context_a, &alice)
        .unwrap()
        .is_none());

    CapabilitiesRepository::new(&store)
        .set_context_member(&gid, &context_a, &alice, 0b001)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_context_member(&gid, &context_a, &bob, 0b010)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_context_member(&gid, &context_b, &alice, 0b111)
        .unwrap();

    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_a, &alice)
            .unwrap()
            .unwrap(),
        0b001
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_a, &bob)
            .unwrap()
            .unwrap(),
        0b010
    );
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .context_member_capability(&gid, &context_b, &alice)
            .unwrap()
            .unwrap(),
        0b111
    );
}

#[test]
fn delete_defaults_and_member_capabilities_clears_values() {
    let store = test_store();
    let gid = test_group_id();
    let alice = PublicKey::from([0x41; 32]);
    let bob = PublicKey::from([0x42; 32]);

    use calimero_context_config::VisibilityMode;

    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, 0b101)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&gid, VisibilityMode::Restricted)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &alice, 0b001)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &bob, 0b010)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .enumerate_members(&gid)
            .unwrap()
            .len(),
        2
    );

    CapabilitiesRepository::new(&store)
        .delete_default(&gid)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .delete_subgroup_visibility(&gid)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .delete_all_member_caps(&gid)
        .unwrap();

    assert!(CapabilitiesRepository::new(&store)
        .default_capabilities(&gid)
        .unwrap()
        .is_none());
    // Subgroup visibility's contract is "absent reads as Restricted",
    // so a successful delete is observed as the default value coming back.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Restricted
    );
    assert!(CapabilitiesRepository::new(&store)
        .member_capability(&gid, &alice)
        .unwrap()
        .is_none());
    assert!(CapabilitiesRepository::new(&store)
        .member_capability(&gid, &bob)
        .unwrap()
        .is_none());
    assert!(CapabilitiesRepository::new(&store)
        .enumerate_members(&gid)
        .unwrap()
        .is_empty());
}

// -----------------------------------------------------------------------
// Auto-group: node identity as admin (regression test for fix)
// -----------------------------------------------------------------------

/// When an auto-group is created, the node's identity (not a random one)
/// should be added as Admin. This test verifies that after
/// `add_group_member_with_keys` the identity is a member and admin of the
/// group — the same check that `listGroupMembers` and `joinGroupContext`
/// perform via `check_group_membership`.
#[test]
fn auto_group_node_identity_is_admin_member() {
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let context_id = ContextId::from([0xDD; 32]);
    let auto_group_id = ContextGroupId::from(*context_id.as_ref());

    // Simulate what create_context does: use node's group identity
    let node_sk = PrivateKey::random(&mut OsRng);
    let node_pk = node_sk.public_key();
    let sender_key = PrivateKey::random(&mut OsRng);

    // Save group meta (as create_context does for auto-groups)
    MetaRepository::new(&store)
        .save(
            &auto_group_id,
            &GroupMetaValue {
                app_key: [0u8; 32],
                target_application_id: ApplicationId::from([0xCC; 32]),
                upgrade_policy: UpgradePolicy::Automatic,
                created_at: 1_700_000_000,
                admin_identity: node_pk,
                owner_identity: node_pk,
                migration: None,
                auto_join: true,
            },
        )
        .unwrap();

    // Add node identity as admin with keys (as create_context does)
    MembershipRepository::new(&store)
        .add_member_with_keys(
            &auto_group_id,
            &node_pk,
            GroupMemberRole::Admin,
            Some(*node_sk.as_bytes()),
            Some(*sender_key.as_bytes()),
        )
        .unwrap();

    // Register the context in the group
    register_context_in_group(&store, &auto_group_id, &context_id).unwrap();

    // The node's identity should be recognized as a group member
    assert!(MembershipRepository::new(&store)
        .is_member(&auto_group_id, &node_pk)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_admin(&auto_group_id, &node_pk)
        .unwrap());

    // The group should have exactly 1 member
    assert_eq!(
        MembershipRepository::new(&store)
            .count(&auto_group_id)
            .unwrap(),
        1
    );

    // The context should be registered in the group
    assert_eq!(
        get_group_for_context(&store, &context_id).unwrap().unwrap(),
        auto_group_id
    );
}

/// A random identity that is NOT the node's group identity should NOT
/// pass membership checks — this is the bug scenario before the fix.
#[test]
fn auto_group_random_identity_not_found_by_node_check() {
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let auto_group_id = ContextGroupId::from([0xEE; 32]);

    // A random creator identity was added as admin
    let random_sk = PrivateKey::random(&mut OsRng);
    let random_pk = random_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&auto_group_id, &random_pk, GroupMemberRole::Admin)
        .unwrap();

    // The node's ACTUAL group identity is different
    let node_sk = PrivateKey::random(&mut OsRng);
    let node_pk = node_sk.public_key();

    // The random identity IS a member
    assert!(MembershipRepository::new(&store)
        .is_member(&auto_group_id, &random_pk)
        .unwrap());

    // But the node's identity is NOT a member — this is the bug
    assert!(!MembershipRepository::new(&store)
        .is_member(&auto_group_id, &node_pk)
        .unwrap());
}

#[test]
fn local_state_join_tracking_and_delete_group_rows_cleanup() {
    let store = test_store();
    let gid = ContextGroupId::from([0xC1; 32]);
    let context = ContextId::from([0xC2; 32]);
    let member = PublicKey::from([0xC3; 32]);
    let member2 = PublicKey::from([0xC4; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&gid, 0b111)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&gid, calimero_context_config::VisibilityMode::Restricted)
        .unwrap();
    MetadataRepository::new(&store)
        .set_group(
            &gid,
            &calimero_primitives::metadata::MetadataRecord {
                name: Some("g-alias".to_owned()),
                data: Default::default(),
                updated_at: 0,
                updated_by: [1_u8; 32].into(),
            },
        )
        .unwrap();

    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member2, GroupMemberRole::Member)
        .unwrap();
    MetadataRepository::new(&store)
        .set_member(
            &gid,
            &member2,
            &calimero_primitives::metadata::MetadataRecord {
                name: Some("member2".to_owned()),
                data: Default::default(),
                updated_at: 0,
                updated_by: [1_u8; 32].into(),
            },
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&gid, &member2, 0b010)
        .unwrap();
    set_local_gov_nonce(&store, &gid, &member, 7).unwrap();

    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let signer_sk = PrivateKey::random(&mut rng);
    let op =
        SignedGroupOp::sign(&signer_sk, gid.to_bytes().into(), vec![], 1, GroupOp::Noop).unwrap();
    let op_bytes = borsh::to_vec(&op).unwrap();
    append_op_log_entry(&store, &gid, 1, &op_bytes).unwrap();
    set_op_head(&store, &gid, 1, vec![[0x11; 32]]).unwrap();
    track_member_context_join(&store, &gid, &member2, &context, [0xAA; 32]).unwrap();

    // Two deny-list rows under this group — to assert teardown sweeps
    // the whole prefix, not just one entry.
    let denied_a = PublicKey::from([0xD1; 32]);
    let denied_b = PublicKey::from([0xD2; 32]);
    DenyListRepository::new(&store)
        .mark(&gid, &denied_a)
        .unwrap();
    DenyListRepository::new(&store)
        .mark(&gid, &denied_b)
        .unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &denied_a)
        .unwrap());
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &denied_b)
        .unwrap());

    assert_eq!(get_local_gov_nonce(&store, &gid, &member).unwrap(), Some(7));
    assert_eq!(read_op_log_after(&store, &gid, 0, 10).unwrap().len(), 1);
    assert_eq!(
        get_member_context_joins(&store, &gid, &member2)
            .unwrap()
            .len(),
        1
    );

    delete_group_local_rows(&store, &gid).unwrap();

    assert!(MetaRepository::new(&store).load(&gid).unwrap().is_none());
    assert!(MetadataRepository::new(&store)
        .group_metadata(&gid)
        .unwrap()
        .is_none());
    assert!(CapabilitiesRepository::new(&store)
        .default_capabilities(&gid)
        .unwrap()
        .is_none());
    // Subgroup visibility falls back to Restricted when the row is absent
    // — that's how a successful delete is observed by the typed API.
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        calimero_context_config::VisibilityMode::Restricted
    );
    assert!(CapabilitiesRepository::new(&store)
        .enumerate_members(&gid)
        .unwrap()
        .is_empty());
    assert!(MetadataRepository::new(&store)
        .enumerate_members(&gid)
        .unwrap()
        .is_empty());
    assert!(get_local_gov_nonce(&store, &gid, &member)
        .unwrap()
        .is_none());
    assert!(get_op_head(&store, &gid).unwrap().is_none());
    assert!(read_op_log_after(&store, &gid, 0, 10).unwrap().is_empty());
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&gid, &denied_a)
            .unwrap(),
        "deny-list entries must be swept during group teardown"
    );
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&gid, &denied_b)
            .unwrap(),
        "deny-list entries must be swept during group teardown"
    );
}

#[test]
fn tee_policy_and_quote_hash_scan_latest_and_match() {
    let store = test_store();
    let gid = ContextGroupId::from([0xD1; 32]);
    let quote_a = [0xE1; 32];
    let quote_b = [0xE2; 32];

    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let signer_sk = PrivateKey::random(&mut rng);
    let policy_1 = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m1".to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: false,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 1, &borsh::to_vec(&policy_1).unwrap()).unwrap();

    let joined = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes().into(),
        vec![],
        2,
        GroupOp::MemberJoinedViaTeeAttestation {
            member: PublicKey::from([0xD3; 32]),
            quote_hash: quote_a,
            mrtd: "m1".to_owned(),
            rtmr0: "r0".to_owned(),
            rtmr1: "r1".to_owned(),
            rtmr2: "r2".to_owned(),
            rtmr3: "r3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 2, &borsh::to_vec(&joined).unwrap()).unwrap();

    let policy_2 = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes().into(),
        vec![],
        3,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m2".to_owned()],
            allowed_rtmr0: vec!["x".to_owned()],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned(), "warn".to_owned()],
            accept_mock: true,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 3, &borsh::to_vec(&policy_2).unwrap()).unwrap();

    let latest = read_tee_admission_policy(&store, &gid).unwrap().unwrap();
    assert_eq!(latest.allowed_mrtd, vec!["m2".to_owned()]);
    assert!(latest.accept_mock);
    assert!(is_quote_hash_used(&store, &gid, &quote_a).unwrap());
    assert!(!is_quote_hash_used(&store, &gid, &quote_b).unwrap());
}

/// Replica-side TEE bootstrap regression guard (PR #2473, finding B).
///
/// This is the REPLICA counterpart to the owner-side coverage in
/// `crates/node/src/local_governance_node_e2e.rs::
/// ns_announce_admits_announcer_as_read_only_tee_member`. It exercises the
/// exact apply path a freshly-admitted ReadOnlyTee fleet node (B) takes when
/// its post-KeyDelivery retry batch replays the namespace's governance ops
/// that it did NOT author: a `TeeAdmissionPolicySet` (nonce 1) followed by a
/// `MemberJoinedViaTeeAttestation` (nonce 2), both arriving as encrypted
/// `NamespaceOp::Group` ops through `NamespaceGovernance::apply_signed_op`.
///
/// The membership op's apply reads the admission policy via
/// `read_required_tee_admission_policy`, which reconstructs the policy purely
/// by scanning the local group op-log (`group_store/tee.rs`). Before the fix,
/// a replica applied an op's state mutation but never wrote its op-log entry —
/// only the authoring node did — so the just-applied policy op was invisible
/// to the membership op and the apply was rejected with
/// "no TeeAdmissionPolicySet exists for group". The node then never recorded
/// its own membership.
///
/// The fix (`apply_group_op_inner` in `namespace_governance.rs`) persists each
/// handled op to the replica's op-log, so within the single retry batch the
/// policy op (nonce 1) commits its log entry before the membership op (nonce 2)
/// reads it back. This test FAILS (membership apply errors with
/// "no TeeAdmissionPolicySet exists") if that op-log persistence is removed.
/// Replica op-log dedup must survive head pruning (PR #2473, finding 4).
///
/// The replica apply path (`apply_group_op_inner`) appends every handled op to
/// the group op-log and guards against a re-received duplicate. The guard used
/// to consult the op-head's `dag_heads` — but `dag_heads` is only the CURRENT
/// DAG frontier: once a later op supersedes an earlier one, the earlier op's
/// content hash is pruned out of the head set. A `dag_heads`-based check would
/// then miss a superseded-then-replayed op (gossip dup / backfill replay during
/// a retry that never advanced the per-signer nonce) and append a SECOND log
/// entry, double-counting it in every log scan (`read_tee_admission_policy`,
/// `is_quote_hash_used`, `is_tee_admitted_identity`).
///
/// The fix dedups against the PERSISTED op-log
/// (`local_state::op_log_contains_content_hash`), which is monotonic. This test:
///   1. applies op A (policy set, nonce 1) and op B (policy set, nonce 2, with A
///      as its parent) via the real `NamespaceGovernance::apply_signed_op` path,
///   2. asserts A's content hash is PRUNED from the op-head's `dag_heads` (the
///      condition that broke the old check) yet `op_log_contains_content_hash`
///      still reports A as logged,
///   3. re-drives op A through the full apply path under the exact retry/backfill
///      condition the guard exists for (its per-signer nonce reset to 0, and its
///      namespace op-log entry removed so the namespace-level dedup does not
///      short-circuit first), and asserts the GROUP op-log still holds exactly
///      two entries — no duplicate.
///
/// With the old `dag_heads.contains` check step 3 appends a third entry and the
/// final assertion fails.
/// A stale op-head (crash between the entry `put` and the head `put`) must not
/// let a later op overwrite the orphan entry (PR #2473, finding B1).
///
/// `persist_group_op_log_entry` writes the op-log entry first, then the head
/// (non-atomic — `calimero-store` has no batch). A crash in between leaves an
/// ORPHAN entry at sequence N while the head still points at N-1. Deriving the
/// next op's sequence from `GroupOpHeadValue.sequence` would then reuse N and
/// silently overwrite the orphan (e.g. clobbering a `TeeAdmissionPolicySet`
/// that a later membership op depends on). The fix derives `next_seq` from the
/// ACTUAL max op-log sequence, so the next op always lands strictly above every
/// persisted entry.
///
/// This test:
///   1. applies op A (nonce 1) via the real apply path — entry + head at seq 1,
///   2. simulates the crash by rewinding the head to seq 0 (the entry stays),
///   3. applies a DIFFERENT op B (nonce 2) and asserts the op-log now holds
///      TWO entries (A preserved at seq 1, B appended at seq 2) — i.e. B did
///      not overwrite the orphan.
///      A partial bootstrap seed (meta written, admin member row missing) must be
///      repaired by a later seed call, not skipped forever (PR #2473, finding C).
///
/// `seed_bootstrap_admin_if_absent` writes two non-atomic rows: group meta and
/// the deliverer's member row. A crash between them leaves meta present but the
/// member row missing. Gating the whole seed on
/// `MetaRepository::new(..).load().is_some()` would return early forever and
/// never add the member row, so encrypted replay keeps failing the verifier-
/// membership check with no way to self-repair. The fix gates each row on its
/// own presence and always ensures the member row exists, so a later
/// `KeyDelivery` re-entry repairs the partial seed.
///
/// #2474: the seeded member row is now a non-authoritative `Member` (NOT
/// `Admin`) — founding authority comes from the `NamespaceCreated` genesis, not
/// the KeyDelivery signer. This test asserts the repair idempotency of that row.
#[test]
fn seed_bootstrap_admin_repairs_missing_member_row() {
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::NamespaceGovernance;

    let store = test_store();
    let mut rng = OsRng;

    let founder_sk = PrivateKey::random(&mut rng);
    let founder = founder_sk.public_key();

    let namespace_id = [0xC6u8; 32];
    let ns_gid = ContextGroupId::from(namespace_id);

    let gov = NamespaceGovernance::new(&store, namespace_id.into());

    // ---- First seed: both meta and the (non-authoritative) member row are
    // written. #2474: the member row is `Member`, not `Admin`. ----
    gov.seed_bootstrap_admin_if_absent(namespace_id, &founder)
        .expect("initial seed");
    assert!(MetaRepository::new(&store).load(&ns_gid).unwrap().is_some());
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "first seed must add the deliverer's member row (non-authoritative)"
    );

    // ---- Simulate the partial-seed crash: meta survives, member row lost. ----
    MembershipRepository::new(&store)
        .remove_member(&ns_gid, &founder)
        .unwrap();
    assert!(MetaRepository::new(&store).load(&ns_gid).unwrap().is_some());
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        None,
        "member row is gone, only meta remains (partial seed)"
    );

    // ---- Re-seed (later KeyDelivery re-entry): the OLD code returned early on
    // the meta gate and never repaired the member row. ----
    gov.seed_bootstrap_admin_if_absent(namespace_id, &founder)
        .expect("repair seed");
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&ns_gid, &founder)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "re-seed must repair the missing member row"
    );
}

fn append_tee_policy_op(store: &Store, group: &ContextGroupId, seq: u64, mrtd: &str) {
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let signer_sk = PrivateKey::random(&mut rng);
    let op = SignedGroupOp::sign(
        &signer_sk,
        group.to_bytes().into(),
        vec![],
        seq,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec![mrtd.to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: false,
        },
    )
    .unwrap();
    append_op_log_entry(store, group, seq, &borsh::to_vec(&op).unwrap()).unwrap();
}

#[test]
fn tee_policy_lookup_from_subgroup_returns_root() {
    // Policy set on the root — a lookup via a nested subgroup resolves up
    // the parent chain and returns the root's policy. Core of the
    // namespace-scoped policy decision (see
    // project_subgroup_policy_decision.md).
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let grandchild = ContextGroupId::from([0xE2; 32]);

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();
    append_tee_policy_op(&store, &root, 1, "mrtd-root");

    for gid in [root, child, grandchild] {
        let p = read_tee_admission_policy(&store, &gid)
            .unwrap()
            .expect("policy resolved via root");
        assert_eq!(p.allowed_mrtd, vec!["mrtd-root".to_owned()]);
    }
}

#[test]
fn tee_policy_lookup_from_subgroup_ignores_subgroup_own_bytes() {
    // A subgroup carrying a stale policy op in its own log (e.g. legacy
    // data written before we started rejecting subgroup-scoped policies)
    // must NOT be returned. The reader walks to the root; the root has
    // no policy, so the result is None.
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    append_tee_policy_op(&store, &child, 1, "mrtd-subgroup-ignored");

    assert!(
        read_tee_admission_policy(&store, &child).unwrap().is_none(),
        "subgroup's own policy bytes must be ignored"
    );
    assert!(read_tee_admission_policy(&store, &root).unwrap().is_none());
}

#[test]
fn tee_policy_lookup_on_root_without_policy_is_none() {
    let store = test_store();
    let root = ContextGroupId::from([0xC0; 32]);
    assert!(read_tee_admission_policy(&store, &root).unwrap().is_none());
}

#[test]
fn apply_tee_policy_op_on_subgroup_rejected() {
    // Even a signed, otherwise-valid TeeAdmissionPolicySet op targeting a
    // subgroup must be refused at apply time. Reader resolves to root, so
    // accepting the op would create dead data; rejecting it keeps state
    // aligned with the decision that policies are namespace-scoped.
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;
    let root = ContextGroupId::from([0xB0; 32]);
    let child = ContextGroupId::from([0xB1; 32]);
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&root, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        child.to_bytes().into(),
        vec![],
        1,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m".to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: false,
        },
    )
    .unwrap();

    let err = apply_local_signed_group_op(&store, &op).expect_err("apply on subgroup must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("namespace-scoped") || msg.contains("root"),
        "error should mention namespace scoping, got: {msg}"
    );
}

// -----------------------------------------------------------------------
// resolve_group_signing_key — ancestor hierarchy walk tests
// -----------------------------------------------------------------------

#[test]
fn resolve_signing_key_finds_key_on_self() {
    let store = test_store();
    let gid = ContextGroupId::from([0xD0; 32]);
    let pk = PublicKey::from([0xD1; 32]);
    let sk = [0xDD; 32];

    SigningKeysRepository::new(&store)
        .store_key(&gid, &pk, &sk)
        .unwrap();

    let found = SigningKeysRepository::new(&store)
        .resolve(&gid, &pk)
        .unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_walks_to_parent() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &pk, &sk)
        .unwrap();

    // Child should find root's key via parent walk
    let found = SigningKeysRepository::new(&store)
        .resolve(&child, &pk)
        .unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_walks_grandparent_chain() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let grandchild = ContextGroupId::from([0xD2; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xBB; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &pk, &sk)
        .unwrap();

    // Grandchild walks upward: grandchild -> child -> root, finds root's key
    let found = SigningKeysRepository::new(&store)
        .resolve(&grandchild, &pk)
        .unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_returns_nearest_ancestor() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let grandchild = ContextGroupId::from([0xD2; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let root_sk = [0xAA; 32];
    let child_sk = [0xBB; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();

    SigningKeysRepository::new(&store)
        .store_key(&root, &pk, &root_sk)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&child, &pk, &child_sk)
        .unwrap();

    // Grandchild should find child's key (nearest), not root's
    let found = SigningKeysRepository::new(&store)
        .resolve(&grandchild, &pk)
        .unwrap();
    assert_eq!(found, Some(child_sk));

    // Child should find its own key
    let found = SigningKeysRepository::new(&store)
        .resolve(&child, &pk)
        .unwrap();
    assert_eq!(found, Some(child_sk));
}

#[test]
fn resolve_signing_key_none_for_orphan() {
    let store = test_store();
    let orphan = ContextGroupId::from([0xD0; 32]);
    let pk = PublicKey::from([0x10; 32]);

    // No parent, no key stored anywhere
    let found = SigningKeysRepository::new(&store)
        .resolve(&orphan, &pk)
        .unwrap();
    assert_eq!(found, None);
}

#[test]
fn resolve_signing_key_wrong_identity_not_found() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let admin = PublicKey::from([0x10; 32]);
    let other = PublicKey::from([0x20; 32]);
    let sk = [0xCC; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &admin, &sk)
        .unwrap();

    // Different identity should not find the key
    let found = SigningKeysRepository::new(&store)
        .resolve(&child, &other)
        .unwrap();
    assert_eq!(found, None);

    // Correct identity should find it
    let found = SigningKeysRepository::new(&store)
        .resolve(&child, &admin)
        .unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_broken_by_unnest() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &pk, &sk)
        .unwrap();

    // Before unnest: child can find root's key
    assert_eq!(
        SigningKeysRepository::new(&store)
            .resolve(&child, &pk)
            .unwrap(),
        Some(sk)
    );

    // Unnest breaks the parent link
    NamespaceRepository::new(&store)
        .unnest(&root, &child)
        .unwrap();

    // After unnest: child can no longer walk to root
    assert_eq!(
        SigningKeysRepository::new(&store)
            .resolve(&child, &pk)
            .unwrap(),
        None
    );
}

#[test]
fn resolve_signing_key_survives_renesting() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &pk, &sk)
        .unwrap();

    // Unnest
    NamespaceRepository::new(&store)
        .unnest(&root, &child)
        .unwrap();
    assert_eq!(
        SigningKeysRepository::new(&store)
            .resolve(&child, &pk)
            .unwrap(),
        None
    );

    // Re-nest: key should be reachable again
    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();
    assert_eq!(
        SigningKeysRepository::new(&store)
            .resolve(&child, &pk)
            .unwrap(),
        Some(sk)
    );
}

#[test]
fn resolve_signing_key_none_when_exceeding_max_depth() {
    use super::namespace::MAX_NAMESPACE_DEPTH;

    let store = test_store();
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xEE; 32];

    // Build a chain of MAX_NAMESPACE_DEPTH + 1 groups (root + 16 children)
    let groups: Vec<ContextGroupId> = (0..=MAX_NAMESPACE_DEPTH)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[0] = 0xE0;
            bytes[1] = i as u8;
            ContextGroupId::from(bytes)
        })
        .collect();

    // Nest each group under the previous one: groups[0] -> groups[1] -> ... -> groups[16]
    for i in 0..MAX_NAMESPACE_DEPTH {
        NamespaceRepository::new(&store)
            .nest(&groups[i], &groups[i + 1])
            .unwrap();
    }

    // Store key only on the root
    SigningKeysRepository::new(&store)
        .store_key(&groups[0], &pk, &sk)
        .unwrap();

    // The deepest group (index MAX_NAMESPACE_DEPTH) is 16 levels below root.
    // The loop traverses MAX_NAMESPACE_DEPTH parent edges (matching
    // resolve_namespace), then does a final check on the reached group.
    // This means self + 16 edges + final check = covers the full chain.
    let at_boundary = SigningKeysRepository::new(&store)
        .resolve(&groups[MAX_NAMESPACE_DEPTH], &pk)
        .unwrap();
    assert_eq!(
        at_boundary,
        Some(sk),
        "key at root should be reachable at exactly MAX_NAMESPACE_DEPTH"
    );

    // One level shallower should also find it
    let within_limit = SigningKeysRepository::new(&store)
        .resolve(&groups[MAX_NAMESPACE_DEPTH - 1], &pk)
        .unwrap();
    assert_eq!(
        within_limit,
        Some(sk),
        "key should be reachable within depth limit"
    );
}

#[test]
fn resolve_reaches_root_at_max_depth() {
    use super::namespace::MAX_NAMESPACE_DEPTH;

    let store = test_store();

    // Chain of MAX_NAMESPACE_DEPTH + 2 groups so we can exercise both the
    // deepest legal subgroup (depth MAX) and one level past it (depth MAX+1).
    let groups: Vec<ContextGroupId> = (0..=MAX_NAMESPACE_DEPTH + 1)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[0] = 0xD0;
            bytes[1] = i as u8;
            ContextGroupId::from(bytes)
        })
        .collect();
    for i in 0..MAX_NAMESPACE_DEPTH + 1 {
        NamespaceRepository::new(&store)
            .nest(&groups[i], &groups[i + 1])
            .unwrap();
    }

    // Depth MAX (MAX edges to root) must resolve to the root. This is the
    // regression: the old exclusive `0..MAX` bound bailed `DepthExceeded`
    // here because reaching the root needs MAX+1 walk steps to observe its
    // `None` parent.
    assert_eq!(
        NamespaceRepository::new(&store)
            .resolve(&groups[MAX_NAMESPACE_DEPTH])
            .unwrap(),
        groups[0],
        "deepest legal subgroup (depth MAX) must resolve to the root",
    );

    // One level past MAX stays unresolvable — depth-(MAX+1) is unreachable in
    // production anyway, and this matches `check_path`'s inclusive bound.
    assert!(
        NamespaceRepository::new(&store)
            .resolve(&groups[MAX_NAMESPACE_DEPTH + 1])
            .is_err(),
        "depth MAX+1 must still bail DepthExceeded",
    );
}

// -----------------------------------------------------------------------
// governance_preflight logic — testing the store-level checks that
// governance_preflight orchestrates (admin auth + signing key resolution)
// -----------------------------------------------------------------------

#[test]
fn preflight_rejects_non_admin_when_required() {
    let store = test_store();
    let gid = ContextGroupId::from([0xF0; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    // Admin passes
    assert!(MembershipRepository::new(&store)
        .require_admin(&gid, &admin)
        .is_ok());
    // Non-admin fails
    assert!(MembershipRepository::new(&store)
        .require_admin(&gid, &member)
        .is_err());
    // Unknown identity fails
    let unknown = PublicKey::from([0x03; 32]);
    assert!(MembershipRepository::new(&store)
        .require_admin(&gid, &unknown)
        .is_err());
}

#[test]
fn preflight_signing_key_resolved_through_hierarchy() {
    // Simulates what governance_preflight does: resolve signing key for a
    // child group where the key only exists on the root (namespace).
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let sk = [0xAA; 32];

    // Set up root with meta + admin + signing key
    MetaRepository::new(&store)
        .save(&root, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&root, &admin, GroupMemberRole::Admin)
        .unwrap();
    SigningKeysRepository::new(&store)
        .store_key(&root, &admin, &sk)
        .unwrap();

    // Set up child nested under root, with meta + admin but NO signing key
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &admin, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&root, &child)
        .unwrap();

    // Verify: group exists, admin check passes, signing key resolves via parent
    assert!(MetaRepository::new(&store).load(&child).unwrap().is_some());
    assert!(MembershipRepository::new(&store)
        .require_admin(&child, &admin)
        .is_ok());
    let resolved = SigningKeysRepository::new(&store)
        .resolve(&child, &admin)
        .unwrap();
    assert_eq!(resolved, Some(sk), "signing key should resolve from root");
}

#[test]
fn preflight_fails_when_no_signing_key_in_hierarchy() {
    let store = test_store();
    let gid = ContextGroupId::from([0xF0; 32]);
    let admin = PublicKey::from([0x01; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    // No signing key stored anywhere

    let resolved = SigningKeysRepository::new(&store)
        .resolve(&gid, &admin)
        .unwrap();
    assert_eq!(resolved, None, "no signing key should be found");
}

#[test]
fn preflight_fails_for_nonexistent_group() {
    let store = test_store();
    let gid = ContextGroupId::from([0xF0; 32]);

    // Group doesn't exist — load_group_meta returns None
    assert!(MetaRepository::new(&store).load(&gid).unwrap().is_none());
}

// -----------------------------------------------------------------------
// recursive_remove_member — cascade removal through group hierarchy
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// restore_member_context_identities — local rejoiner ContextIdentity
// marker recovery on `MemberAdded` / `MemberJoinedOpen` apply. The
// cascade helper at `cascade_remove_member_from_group_tree` deletes the
// per-context `ContextIdentity` marker for the leaver/removed member;
// the rejoin arms must invert that on the local rejoiner's node so the
// rejoiner can author again. The marker is keyless — the signing key is
// resolved live from the namespace identity — so this only re-creates a
// presence bit, scoped to the local rejoiner (a no-op on other peers).
// -----------------------------------------------------------------------

#[test]
fn restore_member_context_identities_writes_missing_marker_rows() {
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x21; 32]);
    let sk_bytes = [0x99u8; 32];
    let ctx_a = ContextId::from([0xC1; 32]);
    let ctx_b = ContextId::from([0xC2; 32]);

    register_context_in_group(&store, &gid, &ctx_a).unwrap();
    register_context_in_group(&store, &gid, &ctx_b).unwrap();

    // The scope gate reads THIS node's namespace identity (`gid` resolves to
    // itself — no parent). Storing it for `member` makes this node the local
    // rejoiner; the function re-creates its keyless membership markers.
    NamespaceRepository::new(&store)
        .store_identity(&gid, &member, &sk_bytes, &[0u8; 32])
        .unwrap();

    restore_member_context_identities(&store, &gid, &member).unwrap();

    let handle = store.handle();
    for ctx in [&ctx_a, &ctx_b] {
        let key = calimero_store::key::ContextIdentity::new(*ctx, member);
        let row = handle
            .get(&key)
            .unwrap()
            .expect("marker row should be created");
        assert_eq!(
            row.private_key, None,
            "the marker is keyless — the signing key is resolved live from the namespace identity"
        );
        assert_eq!(row.sender_key, None, "marker carries no sender_key");
    }
}

#[test]
fn restore_member_context_identities_no_op_when_not_local_rejoiner() {
    // The scope gate: a node whose stored namespace identity is NOT
    // `member` must not write a marker row for `member` — marker recovery
    // is scoped to the local rejoiner. With no namespace identity stored
    // at all, the function is likewise a no-op.
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x21; 32]);
    let someone_else = PublicKey::from([0x42; 32]);
    let ctx = ContextId::from([0xC3; 32]);
    register_context_in_group(&store, &gid, &ctx).unwrap();

    // No namespace identity at all → no-op.
    restore_member_context_identities(&store, &gid, &member).unwrap();
    let key = calimero_store::key::ContextIdentity::new(ctx, member);
    assert!(
        !store.handle().has(&key).unwrap(),
        "no namespace identity stored → must not write a row"
    );

    // Namespace identity belongs to a different pk → still a no-op for
    // `member`.
    NamespaceRepository::new(&store)
        .store_identity(&gid, &someone_else, &[0x55; 32], &[0u8; 32])
        .unwrap();
    restore_member_context_identities(&store, &gid, &member).unwrap();
    assert!(
        !store.handle().has(&key).unwrap(),
        "namespace identity ≠ member → must not write a row for member"
    );
}

#[test]
fn restore_member_context_identities_is_idempotent() {
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x22; 32]);
    let original_sk = [0x11u8; 32];
    let original_sender = [0x44u8; 32];
    let ctx = ContextId::from([0xD1; 32]);
    register_context_in_group(&store, &gid, &ctx).unwrap();

    // This node is the local rejoiner — namespace identity stored for
    // `member`.
    NamespaceRepository::new(&store)
        .store_identity(&gid, &member, &original_sk, &[0u8; 32])
        .unwrap();

    // Pre-existing row from a (notional) successful prior `join_context`
    // — already populated with a real sender_key from a delivered
    // KeyDelivery. The restore must NOT overwrite it.
    {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextIdentity::new(ctx, member),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(original_sk),
                    sender_key: Some(original_sender),
                },
            )
            .unwrap();
    }

    restore_member_context_identities(&store, &gid, &member).unwrap();

    let handle = store.handle();
    let row = handle
        .get(&calimero_store::key::ContextIdentity::new(ctx, member))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.private_key,
        Some(original_sk),
        "existing private_key must be preserved (no overwrite)"
    );
    assert_eq!(
        row.sender_key,
        Some(original_sender),
        "existing sender_key must be preserved (would clobber an already-delivered key otherwise)"
    );
}

#[test]
fn restore_member_context_identities_leaves_existing_rows_untouched() {
    // Restore only fills in a MISSING marker; any pre-existing row is left
    // exactly as-is. In particular a standalone context's keyed row (with a
    // delivered sender_key) must not be clobbered into a keyless marker.
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x23; 32]);
    let sk_bytes = [0x66u8; 32];
    let delivered_sender = [0x77u8; 32];
    let ctx = ContextId::from([0xD2; 32]);
    register_context_in_group(&store, &gid, &ctx).unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&gid, &member, &sk_bytes, &[0u8; 32])
        .unwrap();

    // Pre-existing keyed row with a delivered sender_key.
    {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextIdentity::new(ctx, member),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(sk_bytes),
                    sender_key: Some(delivered_sender),
                },
            )
            .unwrap();
    }

    restore_member_context_identities(&store, &gid, &member).unwrap();

    let row = store
        .handle()
        .get(&calimero_store::key::ContextIdentity::new(ctx, member))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.private_key,
        Some(sk_bytes),
        "existing keyed row must be left untouched"
    );
    assert_eq!(
        row.sender_key,
        Some(delivered_sender),
        "an already-delivered sender_key must survive"
    );
}

#[test]
fn member_added_after_remove_restores_context_identity_for_local_rejoiner() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // The local rejoiner: their namespace identity is stored. Note
    // `gid` here is treated as both the group and the namespace root —
    // for a real subgroup the resolve_namespace walk would find the
    // parent, but for this unit test gid IS the namespace. The
    // subgroup-with-real-namespace variant is covered separately by
    // `member_added_after_remove_restores_context_identity_for_subgroup_with_real_namespace`.
    // Pin the flat-namespace assumption explicitly so a future change
    // to `resolve_namespace` that breaks the no-parent case is caught
    // here rather than silently passing.
    assert_eq!(
        NamespaceRepository::new(&store).resolve(&gid).unwrap(),
        gid,
        "flat-namespace test precondition: gid must resolve to itself"
    );
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let member_sk_bytes = *member_sk.as_bytes();
    NamespaceRepository::new(&store)
        .store_identity(&gid, &member_pk, &member_sk_bytes, &[0u8; 32])
        .unwrap();

    // Pre-state: member already added once + has ContextIdentity for
    // the context, then admin removes them which cascades the row
    // delete. Simulate by adding via add_group_member, registering the
    // context, writing the ContextIdentity directly, then issuing
    // MemberRemoved (which cascade-deletes).
    MembershipRepository::new(&store)
        .add_member(&gid, &member_pk, GroupMemberRole::Member)
        .unwrap();
    let ctx = ContextId::from([0xE7; 32]);
    register_context_in_group(&store, &gid, &ctx).unwrap();
    {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextIdentity::new(ctx, member_pk),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(member_sk_bytes),
                    sender_key: Some([0x77; 32]),
                },
            )
            .unwrap();
    }

    let removed = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        dummy_member_removed_op(member_pk),
    )
    .unwrap();
    apply_local_signed_group_op(&store, &removed).unwrap();

    // Confirm cascade ran — row gone.
    {
        let handle = store.handle();
        let key = calimero_store::key::ContextIdentity::new(ctx, member_pk);
        assert!(
            !handle.has(&key).unwrap(),
            "cascade must have deleted the ContextIdentity row"
        );
    }

    // Re-add via signed MemberAdded — the apply arm re-creates the local
    // rejoiner's keyless membership marker.
    let readded = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        2,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &readded).unwrap();

    // A keyless marker is written back (the key is resolved live), and the
    // signer-finder resolves it to the rejoiner's namespace identity.
    let key = calimero_store::key::ContextIdentity::new(ctx, member_pk);
    let row = store
        .handle()
        .get(&key)
        .unwrap()
        .expect("MemberAdded apply must re-create the rejoiner's marker row");
    assert_eq!(
        row.private_key, None,
        "the re-created marker is keyless — key resolved live from the namespace identity"
    );
    assert_eq!(
        find_local_signing_identity(&store, &ctx).unwrap(),
        Some(member_pk),
        "the rejoiner's signer must resolve to their namespace identity"
    );
}

#[test]
fn member_added_after_remove_restores_context_identity_for_subgroup_with_real_namespace() {
    // The first integration test conflates `gid` as both group and
    // namespace, which means `NamespaceRepository::new(group_id).resolve()` returns
    // `gid` itself (no parent walk) and the test does not exercise
    // the subgroup case. This test sets up a real namespace+subgroup
    // pair where the subgroup's resolved namespace is a different
    // ContextGroupId — pinning that the namespace-identity lookup at
    // the resolved namespace (not at `group_id`) correctly gates the
    // restore. This is the variant that mirrors the e2e workflow
    // shape (admin-add to a child subgroup, member rejoins after
    // remove).
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();

    // namespace (root) ── subgroup
    let ns_gid = ContextGroupId::from([0xD0; 32]);
    let subgroup = ContextGroupId::from([0xD1; 32]);
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup)
        .unwrap();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // Local rejoiner: namespace identity is stored under the NAMESPACE
    // id (not the subgroup id). The MemberAdded apply for the subgroup
    // must call `NamespaceRepository::new(subgroup).resolve()` → `ns_gid` and then read
    // the namespace identity from there.
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let member_sk_bytes: [u8; 32] = *member_sk.as_bytes();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk_bytes, &[0u8; 32])
        .unwrap();

    // Pre-state: member was a direct subgroup member with a context
    // identity, then admin removed them which cascade-deleted the row.
    MembershipRepository::new(&store)
        .add_member(&subgroup, &member_pk, GroupMemberRole::Member)
        .unwrap();
    let ctx = ContextId::from([0xE9; 32]);
    register_context_in_group(&store, &subgroup, &ctx).unwrap();
    {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextIdentity::new(ctx, member_pk),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(member_sk_bytes),
                    sender_key: Some([0x33; 32]),
                },
            )
            .unwrap();
    }
    let removed = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(member_pk),
    )
    .unwrap();
    apply_local_signed_group_op(&store, &removed).unwrap();
    let id_key = calimero_store::key::ContextIdentity::new(ctx, member_pk);
    assert!(
        !store.handle().has(&id_key).unwrap(),
        "cascade must have deleted the ContextIdentity row before the rejoin test"
    );

    // Re-add via signed MemberAdded targeting the SUBGROUP. The apply arm
    // must resolve the namespace from `subgroup` (yielding `ns_gid`), look up
    // the namespace identity there, find a match, and re-create the keyless
    // marker. The signer-finder must then resolve it via the same parent walk.
    let readded = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![],
        2,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &readded).unwrap();

    let row = store
        .handle()
        .get(&id_key)
        .unwrap()
        .expect("marker row must be re-created when group_id ≠ namespace_id");
    assert_eq!(
        row.private_key, None,
        "the re-created marker is keyless — key resolved live from the namespace identity"
    );
    assert_eq!(
        find_local_signing_identity(&store, &ctx).unwrap(),
        Some(member_pk),
        "signer must resolve via the subgroup → namespace parent walk"
    );
}

#[test]
fn member_joined_open_clears_deny_list_and_resolves_signer() {
    // The cursor[bot] HIGH-SEVERITY finding pinned by an integration
    // test: when `MemberJoinedOpen` applies, it must (a) `clear_denied`
    // for the rejoiner on the subgroup so peers stop dropping their
    // state-deltas, and (b) re-create the rejoiner's keyless membership
    // marker on the local rejoiner so the signer-finder resolves their
    // namespace identity and they can author again. Pre-fix the apply arm
    // did neither — the kick→inheritance-rejoin and leave→inheritance-
    // rejoin e2e flows hung in post-rejoin sync because the rejoiner's
    // writes were dropped at every peer's deny-list filter.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();

    // Namespace + Open subgroup + context structure:
    //   namespace (root) ── Open subgroup ── context
    let ns_id = [0xA0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let subgroup = ContextGroupId::from([0xA1u8; 32]);
    let ctx = ContextId::from([0xC1u8; 32]);

    // Admin is needed for the `is_group_admin_or_has_capability`
    // membership-policy gates (CAN_INVITE etc.) even though this
    // particular op only checks `MembershipPath::Inherited`.
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    register_context_in_group(&store, &subgroup, &ctx).unwrap();

    // Rejoiner: direct namespace member with CAN_JOIN_OPEN_SUBGROUPS,
    // not a direct subgroup member (post-leave / post-kick state).
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let member_sk_bytes: [u8; 32] = *member_sk.as_bytes();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &member_pk, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &ns_gid,
            &member_pk,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        )
        .unwrap();

    // Pre-state from a prior MemberLeft cascade: deny-list stamped,
    // ContextIdentity row deleted on the local rejoiner.
    DenyListRepository::new(&store)
        .mark(&subgroup, &member_pk)
        .unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&subgroup, &member_pk)
        .unwrap());
    let id_key = calimero_store::key::ContextIdentity::new(ctx, member_pk);
    assert!(!store.handle().has(&id_key).unwrap());

    // The local node IS the rejoiner — its namespace identity matches
    // `member_pk`. Without this gate the `restore_member_context_identities`
    // call would no-op (correctly — peers don't own the rejoiner's sk).
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk_bytes, &[0u8; 32])
        .unwrap();

    // Sign + apply a fresh `MemberJoinedOpen` for the rejoiner.
    let signed = SignedNamespaceOp::sign(
        &member_sk,
        ns_id.into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: member_pk,
            group_id: subgroup.to_bytes().into(),
        }),
    )
    .unwrap();
    apply_signed_namespace_op(&store, &signed).unwrap();

    // Assertion 1: deny-list cleared at the subgroup. This is
    // critical for peers — without it they continue dropping the
    // rejoiner's state-delta gossip at the receive filter.
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&subgroup, &member_pk)
            .unwrap(),
        "MemberJoinedOpen apply MUST clear the per-subgroup deny-list \
         entry so peers stop dropping the rejoiner's state-deltas"
    );

    // Assertion 2: the keyless membership marker is re-created, and the
    // signer-finder resolves it to the rejoiner's namespace identity — that
    // is what lets the local apply path author state-DAG ops again.
    let row = store
        .handle()
        .get(&id_key)
        .unwrap()
        .expect("marker row must be re-created on the local rejoiner");
    assert_eq!(
        row.private_key, None,
        "the re-created marker is keyless — key resolved live from the namespace identity"
    );
    assert_eq!(
        find_local_signing_identity(&store, &ctx).unwrap(),
        Some(member_pk),
        "the rejoiner's signer must resolve to their namespace identity"
    );
}

#[test]
fn member_joined_clears_deny_list_for_rejoiner() {
    // An open-invitation re-join (`RootOp::MemberJoined`) must clear the
    // per-group deny-list entry stamped by a prior `MemberLeft` /
    // `MemberRemoved`, exactly like its sibling arms (`MemberAdded`,
    // `MemberJoinedViaTeeAttestation`, `MemberJoinedOpen`). Pre-fix the
    // `MemberJoined` arm was a no-op, so a rejoined member kept a stale
    // `GroupDeniedMember` row and every peer permanently dropped the
    // rejoiner's state-delta traffic at the receive filter.
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;
    use sha2::{Digest, Sha256};

    let mut rng = OsRng;
    let store = test_store();

    // namespace (root) ── subgroup; the member re-joins the subgroup via
    // an admin-signed open invitation.
    let ns_id = [0xB0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let subgroup = ContextGroupId::from([0xB1u8; 32]);

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup)
        .unwrap();

    // Rejoiner: signs their own `MemberJoined` op, not yet a direct member.
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();

    // Pre-state from a prior `MemberLeft` / `MemberRemoved` cascade: the
    // member is stamped on the subgroup deny-list.
    DenyListRepository::new(&store)
        .mark(&subgroup, &member_pk)
        .unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&subgroup, &member_pk)
        .unwrap());

    // Admin-signed open invitation for the subgroup (no expiry).
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_pk.digest()),
        group_id: subgroup,
        expiration_timestamp: 0,
        invitation_nonce: [0x42; 32],
        invited_role: 1,
    };
    let inv_bytes = borsh::to_vec(&invitation).unwrap();
    let inv_sig = admin_sk.sign(&Sha256::digest(&inv_bytes)).unwrap();
    let signed_invitation = SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    };

    let signed = SignedNamespaceOp::sign(
        &member_sk,
        ns_id.into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: member_pk,
            signed_invitation,
        }),
    )
    .unwrap();
    apply_signed_namespace_op(&store, &signed).unwrap();

    // The join materialized the joiner's direct membership row...
    assert!(
        MembershipRepository::new(&store)
            .has_direct_member(&subgroup, &member_pk)
            .unwrap(),
        "MemberJoined must materialize the joiner's direct membership row"
    );
    // ...and cleared the stale deny-list entry so peers stop dropping the
    // rejoiner's state-deltas at the receive filter.
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&subgroup, &member_pk)
            .unwrap(),
        "MemberJoined apply MUST clear the per-group deny-list entry for \
         the rejoiner so peers stop dropping their state-deltas"
    );
}

#[test]
fn member_added_does_nothing_for_non_rejoiner_peers() {
    // On peers whose local namespace identity is NOT the rejoiner,
    // applying MemberAdded must NOT create a ContextIdentity row for
    // the rejoiner — those peers would write a row claiming to own a
    // private key they don't have, which would let them spoof state-
    // DAG ops as the rejoiner.
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // This node IS the admin — its namespace identity is admin_pk, not
    // the rejoiner's pk.
    let admin_sk_bytes = *admin_sk.as_bytes();
    NamespaceRepository::new(&store)
        .store_identity(&gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
        .unwrap();

    let rejoiner_pk = PrivateKey::random(&mut rng).public_key();
    let ctx = ContextId::from([0xE8; 32]);
    register_context_in_group(&store, &gid, &ctx).unwrap();

    let added = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: rejoiner_pk,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &added).unwrap();

    let handle = store.handle();
    let key = calimero_store::key::ContextIdentity::new(ctx, rejoiner_pk);
    assert!(
        !handle.has(&key).unwrap(),
        "non-rejoiner peers must NOT create a ContextIdentity row for someone else"
    );
}

// -----------------------------------------------------------------------
// create_recursive_invitations / collect_visible_descendant_groups —
// recursive namespace invitations must not leak into (or even enumerate)
// private subgroups the inviter cannot see.
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// NamespaceGovernance::apply_signed_op — governance state machine tests
// -----------------------------------------------------------------------

/// Re-applying an already-applied `SignedNamespaceOp` is a no-op: the
/// op-log already has it, so `apply_signed_op` short-circuits and doesn't
/// re-run side effects or re-append `delta_id` to the namespace DAG head
/// set. Regression for #2327 (duplicate heads → empty `GovernanceParentEdge`
/// → peers reject all of the node's deltas).
// ---------------------------------------------------------------------------
// Strict-tree refactor — orphan state is structurally impossible.
// See spec: docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Namespace-level teardown (issue #2226)
// ---------------------------------------------------------------------------

#[test]
fn delete_namespace_local_state_clears_identity_head_and_ops() {
    use calimero_primitives::identity::PublicKey;
    use calimero_store::key::{
        NamespaceGovHead, NamespaceGovHeadValue, NamespaceGovOp, NamespaceGovOpValue,
        NamespaceIdentity,
    };

    let store = test_store();
    let ns_id = ContextGroupId::from([0xA1; 32]);
    let ns_bytes = ns_id.to_bytes();

    let ns_pk = PublicKey::from([0x11; 32]);
    NamespaceRepository::new(&store)
        .store_identity(&ns_id, &ns_pk, &[0x22; 32], &[0x33; 32])
        .unwrap();

    {
        let mut handle = store.handle();
        handle
            .put(
                &NamespaceGovHead::new(ns_bytes),
                &NamespaceGovHeadValue {
                    sequence: 7,
                    dag_heads: vec![[0x44; 32]],
                },
            )
            .unwrap();
        for i in 0u8..5 {
            let mut delta = [0u8; 32];
            delta[0] = i;
            handle
                .put(
                    &NamespaceGovOp::new(ns_bytes, delta),
                    &NamespaceGovOpValue {
                        skeleton_bytes: vec![i],
                    },
                )
                .unwrap();
        }
    }

    // A second namespace must be left alone.
    let other_ns_id = ContextGroupId::from([0xB2; 32]);
    let other_ns_bytes = other_ns_id.to_bytes();
    let other_pk = PublicKey::from([0x55; 32]);
    NamespaceRepository::new(&store)
        .store_identity(&other_ns_id, &other_pk, &[0x66; 32], &[0x77; 32])
        .unwrap();
    {
        let mut handle = store.handle();
        handle
            .put(
                &NamespaceGovOp::new(other_ns_bytes, [0x88; 32]),
                &NamespaceGovOpValue {
                    skeleton_bytes: vec![0x99],
                },
            )
            .unwrap();
    }

    delete_namespace_local_state(&store, &ns_id).unwrap();

    let handle = store.handle();
    assert!(
        handle
            .get::<NamespaceIdentity>(&NamespaceIdentity::new(ns_bytes))
            .unwrap()
            .is_none(),
        "namespace identity should be cleared"
    );
    assert!(
        handle
            .get::<NamespaceGovHead>(&NamespaceGovHead::new(ns_bytes))
            .unwrap()
            .is_none(),
        "namespace gov head should be cleared"
    );
    for i in 0u8..5 {
        let mut delta = [0u8; 32];
        delta[0] = i;
        assert!(
            handle
                .get::<NamespaceGovOp>(&NamespaceGovOp::new(ns_bytes, delta))
                .unwrap()
                .is_none(),
            "namespace gov op {i} should be cleared"
        );
    }

    // Other namespace untouched.
    assert!(
        handle
            .get::<NamespaceIdentity>(&NamespaceIdentity::new(other_ns_bytes))
            .unwrap()
            .is_some(),
        "other namespace identity must survive"
    );
    assert!(
        handle
            .get::<NamespaceGovOp>(&NamespaceGovOp::new(other_ns_bytes, [0x88; 32]))
            .unwrap()
            .is_some(),
        "other namespace op must survive"
    );
}

/// Simulates the full teardown that `Handler<DeleteNamespaceRequest>`
/// performs locally: per-group `delete_group_local_rows` for every group in
/// the subtree (children-first) + parent/child edge cleanup, plus
/// `delete_namespace_local_state` for namespace-scoped rows. Exercises the
/// contract the HTTP `DELETE /admin-api/namespaces/:id` endpoint depends on
/// after the fix for issue #2226.
#[test]
fn delete_namespace_full_cascade_clears_subtree_and_namespace_state() {
    use calimero_primitives::identity::PublicKey;
    use calimero_store::key::{
        GroupChildIndex, GroupParentRef, NamespaceGovHead, NamespaceGovHeadValue, NamespaceGovOp,
        NamespaceGovOpValue, NamespaceIdentity,
    };

    let store = test_store();
    let ns_id = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    let grandchild = ContextGroupId::from([0xF2; 32]);

    MetaRepository::new(&store)
        .save(&ns_id, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&child, &test_meta())
        .unwrap();
    MetaRepository::new(&store)
        .save(&grandchild, &test_meta())
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_id, &child)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&child, &grandchild)
        .unwrap();

    let ctx_root = ContextId::from([0x10; 32]);
    let ctx_child = ContextId::from([0x11; 32]);
    let ctx_gc = ContextId::from([0x12; 32]);
    register_context_in_group(&store, &ns_id, &ctx_root).unwrap();
    register_context_in_group(&store, &child, &ctx_child).unwrap();
    register_context_in_group(&store, &grandchild, &ctx_gc).unwrap();

    let admin_pk = PublicKey::from([0xAA; 32]);
    MembershipRepository::new(&store)
        .add_member(&ns_id, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&grandchild, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let ns_bytes = ns_id.to_bytes();
    NamespaceRepository::new(&store)
        .store_identity(&ns_id, &admin_pk, &[0x22; 32], &[0x33; 32])
        .unwrap();
    {
        let mut handle = store.handle();
        handle
            .put(
                &NamespaceGovHead::new(ns_bytes),
                &NamespaceGovHeadValue {
                    sequence: 3,
                    dag_heads: vec![[0xCC; 32]],
                },
            )
            .unwrap();
        handle
            .put(
                &NamespaceGovOp::new(ns_bytes, [0x01; 32]),
                &NamespaceGovOpValue {
                    skeleton_bytes: vec![1],
                },
            )
            .unwrap();
    }

    // Execute the same children-first teardown the handler performs.
    let payload = NamespaceRepository::new(&store)
        .collect_subtree_for_cascade(&ns_id)
        .unwrap();
    let all = payload
        .descendant_groups
        .iter()
        .copied()
        .chain(std::iter::once(ns_id));
    for gid in all {
        for ctx in enumerate_group_contexts(&store, &gid, 0, usize::MAX).unwrap() {
            unregister_context_from_group(&store, &gid, &ctx).unwrap();
        }
        let parent = NamespaceRepository::new(&store).parent(&gid).unwrap();
        delete_group_local_rows(&store, &gid).unwrap();
        if let Some(parent) = parent {
            let mut handle = store.handle();
            handle.delete(&GroupParentRef::new(gid.to_bytes())).unwrap();
            handle
                .delete(&GroupChildIndex::new(parent.to_bytes(), gid.to_bytes()))
                .unwrap();
        }
    }
    delete_namespace_local_state(&store, &ns_id).unwrap();

    // Every group's meta must be gone.
    for gid in [ns_id, child, grandchild] {
        assert!(
            MetaRepository::new(&store).load(&gid).unwrap().is_none(),
            "group {gid:?} meta should be gone"
        );
    }

    // Every context must be unregistered from its owning group.
    for (gid, ctx) in [(ns_id, ctx_root), (child, ctx_child), (grandchild, ctx_gc)] {
        assert!(
            get_group_for_context(&store, &ctx).unwrap().is_none(),
            "context {ctx:?} should no longer resolve to group {gid:?}"
        );
    }

    // Edges must be gone.
    assert!(NamespaceRepository::new(&store)
        .parent(&child)
        .unwrap()
        .is_none());
    assert!(NamespaceRepository::new(&store)
        .parent(&grandchild)
        .unwrap()
        .is_none());
    assert!(NamespaceRepository::new(&store)
        .list_children(&ns_id)
        .unwrap()
        .is_empty());
    assert!(NamespaceRepository::new(&store)
        .list_children(&child)
        .unwrap()
        .is_empty());

    // Namespace-level rows must be gone.
    let handle = store.handle();
    assert!(handle
        .get::<NamespaceIdentity>(&NamespaceIdentity::new(ns_bytes))
        .unwrap()
        .is_none());
    assert!(handle
        .get::<NamespaceGovHead>(&NamespaceGovHead::new(ns_bytes))
        .unwrap()
        .is_none());
    assert!(handle
        .get::<NamespaceGovOp>(&NamespaceGovOp::new(ns_bytes, [0x01; 32]))
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// MemberSetAutoFollow (the auto-follow architecture doc)
// ---------------------------------------------------------------------------

/// `namespace_member_pubkeys` must not duplicate the meta admin when
/// the admin also has a `GroupMember` row (e.g. an explicit `MemberJoined`
/// op was emitted for them).
/// Members added via `add_group_member` continue to appear (no regression
/// from the meta-admin enrichment).
// ----------------------------------------------------------------------
// membership_status_at — integration tests
//
// Cover the three branches of the cross-DAG authorization primitive
// against a real in-memory `Store`. Catches regressions that pure-logic
// unit tests on `resolve_membership_from_transitions` (in
// `membership_status.rs`) can't catch — wiring bugs between
// `membership_status_at`, the materialized member set, and the namespace
// op log.
// ----------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Subgroup-management capabilities: CAN_CREATE_SUBGROUP / CAN_DELETE_SUBGROUP /
// CAN_MANAGE_VISIBILITY.
// ---------------------------------------------------------------------------

#[test]
fn permission_checker_subgroup_management_capabilities() {
    use calimero_context_config::MemberCapabilities;

    let store = test_store();
    let gid = ContextGroupId::from([0x9A; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let checker = PermissionChecker::new(&store, gid);

    // Admins pass all three unconditionally.
    assert!(checker.require_can_create_subgroup(&admin).is_ok());
    assert!(checker.require_can_delete_subgroup(&admin).is_ok());
    assert!(checker.require_can_manage_visibility(&admin).is_ok());

    // A bare member is denied all three.
    assert!(checker.require_can_create_subgroup(&member).is_err());
    assert!(checker.require_can_delete_subgroup(&member).is_err());
    assert!(checker.require_can_manage_visibility(&member).is_err());

    // CAN_CREATE_SUBGROUP only.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            MemberCapabilities::CAN_CREATE_SUBGROUP.bits(),
        )
        .unwrap();
    assert!(checker.require_can_create_subgroup(&member).is_ok());
    assert!(checker.require_can_delete_subgroup(&member).is_err());
    assert!(checker.require_can_manage_visibility(&member).is_err());

    // All three at once.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            MemberCapabilities::CAN_CREATE_SUBGROUP.bits()
                | MemberCapabilities::CAN_DELETE_SUBGROUP.bits()
                | MemberCapabilities::CAN_MANAGE_VISIBILITY.bits(),
        )
        .unwrap();
    assert!(checker.require_can_create_subgroup(&member).is_ok());
    assert!(checker.require_can_delete_subgroup(&member).is_ok());
    assert!(checker.require_can_manage_visibility(&member).is_ok());
}

#[test]
fn group_settings_subgroup_visibility_honors_can_manage_visibility() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    use super::group_settings::GroupSettingsService;

    let store = test_store();
    let gid = ContextGroupId::from([0x9B; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let svc = GroupSettingsService::new(&store, gid);

    // Admin can flip it.
    svc.set_subgroup_visibility(&admin, VisibilityMode::Open)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Open
    );

    // Member without the cap cannot.
    assert!(svc
        .set_subgroup_visibility(&member, VisibilityMode::Restricted)
        .is_err());

    // Granting CAN_MANAGE_VISIBILITY lets the member flip it.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            MemberCapabilities::CAN_MANAGE_VISIBILITY.bits(),
        )
        .unwrap();
    svc.set_subgroup_visibility(&member, VisibilityMode::Restricted)
        .unwrap();
    assert_eq!(
        CapabilitiesRepository::new(&store)
            .subgroup_visibility(&gid)
            .unwrap(),
        VisibilityMode::Restricted
    );
}

#[test]
fn set_upgrade_policy_admin_gated_and_blocks_flip_while_migration_pending() {
    use calimero_primitives::context::UpgradePolicy;

    use super::group_settings::GroupSettingsService;
    use crate::test_fixtures::sample_meta_with_admin;
    use crate::MetaRepository;

    let store = test_store();
    let gid = ContextGroupId::from([0xC1; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let mut meta = sample_meta_with_admin(admin);
    meta.upgrade_policy = UpgradePolicy::LazyOnAccess;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();

    let svc = GroupSettingsService::new(&store, gid);

    // Admin-gate (#27): a non-admin signer is rejected.
    assert!(svc
        .set_upgrade_policy(&member, &UpgradePolicy::Automatic)
        .is_err());

    // No migration pending: an admin may flip in either direction.
    svc.set_upgrade_policy(&admin, &UpgradePolicy::Automatic)
        .unwrap();
    svc.set_upgrade_policy(&admin, &UpgradePolicy::LazyOnAccess)
        .unwrap();

    // Pending migration (#6): flipping AWAY from LazyOnAccess is rejected (it
    // would strand un-accessed contexts), but staying LazyOnAccess is allowed.
    meta.upgrade_policy = UpgradePolicy::LazyOnAccess;
    meta.migration = Some(vec![1, 2, 3]);
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    assert!(svc
        .set_upgrade_policy(&admin, &UpgradePolicy::Automatic)
        .is_err());
    svc.set_upgrade_policy(&admin, &UpgradePolicy::LazyOnAccess)
        .unwrap();
}

// ---------------------------------------------------------------------
// Fast-path integration tests for `membership_status_at`
//
// These exercise Branch 1 of `membership_status_at` against a real
// in-memory `Store`: a `GovernanceParentEdge` whose heads equal the local
// DAG heads (both empty here), so the resolver short-circuits to a
// materialized-set lookup and never invokes `prefix_walk_membership`.
//
// What's covered:
//   * The fast path's read of the materialized member set is consistent
//     with what the apply-time check expects when the sender and the
//     receiver are at the same governance cut.
//   * The documented Branch 1 conflation of `Removed` into `NeverMember`
//     (the materialized set has no row for a removed signer, so the
//     fast path cannot distinguish "removed" from "was never a member"
//     without consulting the DAG).
//
// What's NOT covered here:
//   * The forward-only invariant — that lives in `prefix_walk_membership`
//     (Branch 3), where the BFS visits only the ancestry of the
//     position's heads. Exercising it end-to-end requires a non-empty
//     DAG with diverging heads, which means signed namespace ops,
//     keyring setup, and ancestor chains. That harness is tracked
//     separately. The per-transition resolver tests in
//     `membership_status.rs` (`prefix_walk_forward_only_*`) cover the
//     state-machine logic that the BFS feeds into.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// Per-group deny-list tests
//
// Exercise the marking / clearing primitives directly. Apply-path
// integration (MemberAdded clearing on re-add, MemberRemoved /
// MemberLeft marking) is covered by the `apply_local_signed_group_op_*`
// tests which can construct the full SignedGroupOp envelope; here we
// pin the store-level semantics so future refactors of the helper
// can't silently change behavior.
// ---------------------------------------------------------------------

#[test]
fn deny_list_starts_empty_for_new_member() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA0; 32]);
    assert!(!DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_mark_then_query_returns_true() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA1; 32]);

    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_clear_then_query_returns_false() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA2; 32]);

    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
    DenyListRepository::new(&store).clear(&gid, &pk).unwrap();
    assert!(!DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_mark_is_idempotent() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA3; 32]);

    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_clear_on_unmarked_is_noop() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA4; 32]);

    // Should not error or panic — clearing an absent entry is fine.
    DenyListRepository::new(&store).clear(&gid, &pk).unwrap();
    assert!(!DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_is_per_group_not_per_pubkey() {
    let store = test_store();
    let gid_a = ContextGroupId::from([0xB1; 32]);
    let gid_b = ContextGroupId::from([0xB2; 32]);
    let pk = PublicKey::from([0xA5; 32]);

    DenyListRepository::new(&store).mark(&gid_a, &pk).unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid_a, &pk)
        .unwrap());
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&gid_b, &pk)
            .unwrap(),
        "deny-list must be scoped to the group, not the pubkey — \
         a member denied in group A must still be allowed in group B"
    );
}

#[test]
fn deny_list_add_remove_add_cycle_ends_cleared() {
    // The semantics described in the design discussion: re-adding a
    // previously-removed member must restore network access. This test
    // pins that the deny-list reflects the *current* state, not a
    // historical audit log.
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0xA6; 32]);

    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    DenyListRepository::new(&store).clear(&gid, &pk).unwrap();
    DenyListRepository::new(&store).mark(&gid, &pk).unwrap();
    DenyListRepository::new(&store).clear(&gid, &pk).unwrap();
    assert!(!DenyListRepository::new(&store)
        .is_denied(&gid, &pk)
        .unwrap());
}

#[test]
fn deny_list_member_added_op_clears_existing_entry() {
    // Apply-path integration: a `MemberAdded` apply must clear any
    // existing deny-list entry for that member, even if they were
    // previously removed.
    use rand::rngs::OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin_pk = admin_sk.public_key();
    let target_pk = PublicKey::from([0xC1; 32]);

    // Bootstrap: a group meta + an admin member (so the signer has
    // permission to add members).
    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    meta.owner_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    // Seed the deny-list as if `target_pk` had previously been removed.
    DenyListRepository::new(&store)
        .mark(&gid, &target_pk)
        .unwrap();
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &target_pk)
        .unwrap());

    // Apply MemberAdded for target_pk.
    let op = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member: target_pk,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign MemberAdded");
    apply_local_signed_group_op(&store, &op).expect("apply MemberAdded");

    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&gid, &target_pk)
            .unwrap(),
        "MemberAdded must clear the deny-list entry to allow re-add"
    );
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&gid, &target_pk)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "member must actually be in the group after add"
    );
}

#[test]
fn deny_list_member_removed_op_marks_entry() {
    use rand::rngs::OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin_pk = admin_sk.public_key();
    let target_pk = PublicKey::from([0xC2; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    meta.owner_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target_pk, GroupMemberRole::Member)
        .unwrap();
    assert!(!DenyListRepository::new(&store)
        .is_denied(&gid, &target_pk)
        .unwrap());

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(target_pk),
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

    assert!(
        DenyListRepository::new(&store)
            .is_denied(&gid, &target_pk)
            .unwrap(),
        "MemberRemoved must mark the member as denied"
    );
}

#[test]
fn leave_then_admin_readd_restores_a_signable_context_identity() {
    // Follow-up to the re-entry work: a voluntary leaver whom an admin re-adds
    // must be able to AUTHOR again. Authoring needs a `ContextIdentity` row with
    // `private_key: Some(_)` for the context (that is what `get_context_members(
    // owned)` / `choose_owned_identity` look for). `MemberLeft` cascades those
    // rows away; the admin's `MemberAdded` apply must restore them via
    // `restore_member_context_identities`. The e2e observed the re-added leaver
    // stuck on `no owned identities found for context` — this pins the apply-path
    // half of that end to end.
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::VisibilityMode;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();

    // namespace (root) ── Open subgroup ── context, matching the e2e.
    let ns_id = [0xD0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let subgroup = ContextGroupId::from([0xD1u8; 32]);
    let ctx = ContextId::from([0xDCu8; 32]);

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    register_context_in_group(&store, &subgroup, &ctx).unwrap();

    // The leaver: a direct subgroup member. The local node IS the leaver, so its
    // namespace identity matches `member_pk` — required for the restore's
    // anti-spoof gate to write a real private key.
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    let member_sk_bytes: [u8; 32] = *member_sk.as_bytes();
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk_bytes, &[0u8; 32])
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &member_pk, GroupMemberRole::Member)
        .unwrap();
    // As `join_context` would: a signable ContextIdentity row.
    let id_key = calimero_store::key::ContextIdentity::new(ctx, member_pk);
    store
        .handle()
        .put(
            &id_key,
            &calimero_store::types::ContextIdentity {
                private_key: Some(member_sk_bytes),
                sender_key: Some([0x11; 32]),
            },
        )
        .unwrap();
    assert!(
        store
            .handle()
            .get(&id_key)
            .unwrap()
            .unwrap()
            .private_key
            .is_some(),
        "precondition: the member can author before leaving"
    );

    // Leave the subgroup: MemberLeft cascades the ContextIdentity away.
    let left = SignedGroupOp::sign(
        &member_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberLeft {
            member: member_pk,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberLeft");
    apply_local_signed_group_op(&store, &left).expect("apply MemberLeft");
    assert!(
        store.handle().get(&id_key).unwrap().is_none(),
        "MemberLeft must cascade the leaver's ContextIdentity row away"
    );

    // Admin re-adds via `add_group_members` (GroupOp::MemberAdded).
    let add = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![left.content_hash().unwrap()],
        1,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign MemberAdded");
    apply_local_signed_group_op(&store, &add).expect("apply MemberAdded");

    // THE ASSERTION: the re-added leaver can author again. A keyless marker is
    // re-created and the signer-finder resolves it to their namespace identity —
    // that is what unblocks 'no owned identities found for context'.
    let row = store
        .handle()
        .get(&id_key)
        .unwrap()
        .expect("MemberAdded apply must re-create the marker row");
    assert_eq!(
        row.private_key, None,
        "the re-created marker is keyless — key resolved live from the namespace identity"
    );
    assert_eq!(
        find_local_signing_identity(&store, &ctx).unwrap(),
        Some(member_pk),
        "the re-added leaver's signer must resolve to their namespace identity — \
         without it, sync loops forever on 'no owned identities found for context'"
    );
}

#[test]
fn deny_list_remove_then_readd_clears_entry_via_apply_path() {
    use rand::rngs::OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin_pk = admin_sk.public_key();
    let target_pk = PublicKey::from([0xC3; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    meta.owner_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target_pk, GroupMemberRole::Member)
        .unwrap();

    // Remove.
    let rm = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(target_pk),
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &rm).expect("apply MemberRemoved");
    assert!(DenyListRepository::new(&store)
        .is_denied(&gid, &target_pk)
        .unwrap());

    // Re-add.
    let add = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![rm.content_hash().unwrap()],
        2,
        GroupOp::MemberAdded {
            member: target_pk,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign MemberAdded");
    apply_local_signed_group_op(&store, &add).expect("apply MemberAdded");
    assert!(
        !DenyListRepository::new(&store)
            .is_denied(&gid, &target_pk)
            .unwrap(),
        "re-add must clear the deny-list entry — semantics from design discussion"
    );
}

// ---------------------------------------------------------------------------
// Re-entry control, exercised through the real op-apply path.
//
// The pre-existing deny-list rejoin tests above stamp `DenyListRepository::mark`
// directly rather than applying a `MemberRemoved` / `MemberLeft` op, so they
// never produce a re-entry block and say nothing about whether a kick actually
// sticks. These do: every exit here is a signed op, and every re-join attempt is
// a signed op, so the whole gate is under test end to end.
// ---------------------------------------------------------------------------

/// namespace root ── subgroup, with `admin` administering the subgroup.
/// Returns `(ns_id, ns_gid, subgroup)`.
fn reentry_fixture(
    store: &Store,
    admin_pk: PublicKey,
) -> ([u8; 32], ContextGroupId, ContextGroupId) {
    let ns_id = [0xE0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let subgroup = ContextGroupId::from([0xE1u8; 32]);

    MetaRepository::new(store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(store)
        .nest(&ns_gid, &subgroup)
        .unwrap();

    (ns_id, ns_gid, subgroup)
}

/// An admin-signed, non-expiring open invitation to `group_id` bearing `nonce`.
fn signed_invitation_for(
    admin_sk: &PrivateKey,
    group_id: ContextGroupId,
    nonce: [u8; 32],
) -> calimero_context_config::types::SignedGroupOpenInvitation {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use sha2::{Digest, Sha256};

    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id,
        expiration_timestamp: 0,
        invitation_nonce: nonce,
        invited_role: 1,
    };
    let inv_bytes = borsh::to_vec(&invitation).unwrap();
    let inv_sig = admin_sk.sign(&Sha256::digest(&inv_bytes)).unwrap();
    SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

/// Apply a `RootOp::MemberJoined` signed by the joiner themselves.
fn apply_member_joined(
    store: &Store,
    ns_id: [u8; 32],
    member_sk: &PrivateKey,
    signed_invitation: calimero_context_config::types::SignedGroupOpenInvitation,
    nonce: u64,
) -> EyreResult<()> {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

    let signed = SignedNamespaceOp::sign(
        member_sk,
        ns_id.into(),
        vec![],
        nonce,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: member_sk.public_key(),
            signed_invitation,
        }),
    )
    .unwrap();
    apply_signed_namespace_op(store, &signed).map(|_result| ())
}

#[test]
fn a_removed_member_cannot_rejoin_even_with_a_freshly_issued_invitation() {
    use rand::rngs::OsRng;

    // The whole point of the ban: an open invitation is a bearer token that
    // anyone can present, so if a kick only invalidated the invitation the
    // kicked member USED, the admin could never keep them out of a group with a
    // live join link. A removal has to outrank every invitation, including ones
    // minted after the removal.
    let mut rng = OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let (ns_id, _ns_gid, subgroup) = reentry_fixture(&store, admin_pk);

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &member_pk, GroupMemberRole::Member)
        .unwrap();

    // The admin kicks them.
    let rm = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(member_pk),
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &rm).expect("apply MemberRemoved");

    // A brand-new invitation, minted after the kick, with a nonce they have
    // never seen.
    let fresh = signed_invitation_for(&admin_sk, subgroup, [0x77; 32]);
    let err = apply_member_joined(&store, ns_id, &member_sk, fresh, 1)
        .expect_err("a removed member must not be able to rejoin by invitation");

    assert!(
        format!("{err:#}").contains("cannot rejoin"),
        "expected a removal rejection, got: {err:#}"
    );
    assert!(
        !MembershipRepository::new(&store)
            .has_direct_member(&subgroup, &member_pk)
            .unwrap(),
        "the rejected join must not have materialized a member row"
    );
}

#[test]
fn an_admin_re_add_is_the_way_back_in_for_a_removed_member() {
    use rand::rngs::OsRng;

    // The ban is not a tombstone — an admin can undo it, and `MemberAdded` is
    // the only thing that does. This is the counterpart to the test above: it
    // pins that the block is lifted by the admin-gated op and nothing else.
    let mut rng = OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let (_ns_id, _ns_gid, subgroup) = reentry_fixture(&store, admin_pk);

    let member_pk = PublicKey::from([0xC7; 32]);
    MembershipRepository::new(&store)
        .add_member(&subgroup, &member_pk, GroupMemberRole::Member)
        .unwrap();

    let rm = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(member_pk),
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &rm).expect("apply MemberRemoved");
    assert_eq!(
        ReentryRepository::new(&store)
            .block_of(&subgroup, &member_pk)
            .unwrap(),
        Some(calimero_store::key::GroupExitReason::Removed)
    );

    let add = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![rm.content_hash().unwrap()],
        2,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign MemberAdded");
    apply_local_signed_group_op(&store, &add).expect("apply MemberAdded");

    assert!(
        ReentryRepository::new(&store)
            .block_of(&subgroup, &member_pk)
            .unwrap()
            .is_none(),
        "an admin re-adding a removed member must lift the ban"
    );
    assert!(MembershipRepository::new(&store)
        .has_direct_member(&subgroup, &member_pk)
        .unwrap());
}

#[test]
fn a_leaver_cannot_replay_their_invitation_but_a_fresh_one_readmits_them() {
    use rand::rngs::OsRng;

    // Leaving is not a ban — they can come back. What they cannot do is walk
    // back in on the invitation they walked out on; that one is spent for them,
    // so re-entry has to be an explicit re-invite.
    let mut rng = OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let (ns_id, _ns_gid, subgroup) = reentry_fixture(&store, admin_pk);

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();

    // Join with invitation A.
    let invite_a = signed_invitation_for(&admin_sk, subgroup, [0xA1; 32]);
    apply_member_joined(&store, ns_id, &member_sk, invite_a.clone(), 1)
        .expect("first join with a fresh invitation must succeed");
    assert!(MembershipRepository::new(&store)
        .has_direct_member(&subgroup, &member_pk)
        .unwrap());

    // Walk out.
    let left = SignedGroupOp::sign(
        &member_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberLeft {
            member: member_pk,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberLeft");
    apply_local_signed_group_op(&store, &left).expect("apply MemberLeft");
    assert!(!MembershipRepository::new(&store)
        .has_direct_member(&subgroup, &member_pk)
        .unwrap());

    // Replaying invitation A must not readmit them.
    let err = apply_member_joined(&store, ns_id, &member_sk, invite_a, 2)
        .expect_err("replaying the invitation they left with must be rejected");
    assert!(
        format!("{err:#}").contains("already used this invitation"),
        "expected a consumed-invitation rejection, got: {err:#}"
    );
    assert!(!MembershipRepository::new(&store)
        .has_direct_member(&subgroup, &member_pk)
        .unwrap());

    // A freshly issued invitation does.
    let invite_b = signed_invitation_for(&admin_sk, subgroup, [0xB2; 32]);
    apply_member_joined(&store, ns_id, &member_sk, invite_b, 3)
        .expect("a re-invited leaver must be able to rejoin");
    assert!(
        MembershipRepository::new(&store)
            .has_direct_member(&subgroup, &member_pk)
            .unwrap(),
        "being re-invited is exactly how a voluntary leaver comes back"
    );
}

#[test]
fn a_shared_open_invitation_still_admits_others_after_one_member_burns_it() {
    use rand::rngs::OsRng;

    // Consumption is per-identity, not global. An open invitation is a bearer
    // token with no invitee field — a link in a channel that many people join
    // with — so burning it globally on first use would break the shared link for
    // everyone else. Only the identity that used it is barred from replaying it.
    let mut rng = OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let (ns_id, _ns_gid, subgroup) = reentry_fixture(&store, admin_pk);

    let bob_sk = PrivateKey::random(&mut rng);
    let carol_sk = PrivateKey::random(&mut rng);
    let shared = signed_invitation_for(&admin_sk, subgroup, [0x5A; 32]);

    // Bob joins with the shared link, then leaves — spending it for himself.
    apply_member_joined(&store, ns_id, &bob_sk, shared.clone(), 1).expect("bob joins");
    let left = SignedGroupOp::sign(
        &bob_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberLeft {
            member: bob_sk.public_key(),
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberLeft");
    apply_local_signed_group_op(&store, &left).expect("bob leaves");

    // Carol has never used it, so the same link still lets her in.
    apply_member_joined(&store, ns_id, &carol_sk, shared, 1)
        .expect("the shared invitation must still admit an identity that never used it");
    assert!(MembershipRepository::new(&store)
        .has_direct_member(&subgroup, &carol_sk.public_key())
        .unwrap());
}

#[test]
fn a_kicked_member_cannot_re_inherit_into_the_open_subgroup_they_were_kicked_from() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use rand::rngs::OsRng;

    // Inheritance is the back door that makes a subgroup kick meaningless: an
    // Open subgroup admits any parent member holding CAN_JOIN_OPEN_SUBGROUPS
    // automatically, so the kicked member simply re-inherits. That is exactly
    // what `group-kick-and-rejoin-keyshare` exercised as the SUCCESS path, and
    // it is now a rejection.
    let mut rng = OsRng;
    let store = test_store();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let ns_id = [0xE0u8; 32];
    let ns_gid = ContextGroupId::from(ns_id);
    let subgroup = ContextGroupId::from([0xE1u8; 32]);

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MetaRepository::new(&store)
        .save(&subgroup, &sample_meta_with_admin(admin_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns_gid, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();

    // Bob is a namespace member who may join Open subgroups, and holds a direct
    // row in the subgroup.
    let bob_sk = PrivateKey::random(&mut rng);
    let bob_pk = bob_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &bob_pk, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &ns_gid,
            &bob_pk,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        )
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&subgroup, &bob_pk, GroupMemberRole::Member)
        .unwrap();

    // The admin kicks Bob from the subgroup. He keeps his namespace membership
    // and his join capability, so his inheritance path into the Open subgroup is
    // fully intact — only the block stands in his way.
    let rm = SignedGroupOp::sign(
        &admin_sk,
        subgroup.to_bytes().into(),
        vec![],
        1,
        dummy_member_removed_op(bob_pk),
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &rm).expect("apply MemberRemoved");

    let signed = SignedNamespaceOp::sign(
        &bob_sk,
        ns_id.into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: bob_pk,
            group_id: subgroup,
        }),
    )
    .unwrap();
    let err = apply_signed_namespace_op(&store, &signed)
        .expect_err("a kicked member must not re-inherit back into the subgroup");

    assert!(
        format!("{err:#}").contains("cannot re-enter by inheritance"),
        "expected a re-entry rejection, got: {err:#}"
    );
    assert!(
        !MembershipRepository::new(&store)
            .has_direct_member(&subgroup, &bob_pk)
            .unwrap(),
        "the rejected inheritance join must not have materialized a member row"
    );
}

// ---------------------------------------------------------------------------
// Metadata records: CAN_MANAGE_METADATA gate + state-hash exclusion.
// ---------------------------------------------------------------------------

#[test]
fn permission_checker_can_manage_metadata() {
    use calimero_context_config::MemberCapabilities;

    let store = test_store();
    let gid = ContextGroupId::from([0x9C; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let checker = PermissionChecker::new(&store, gid);

    // Admin always passes.
    assert!(checker.require_can_manage_metadata(&admin).is_ok());
    // Bare member denied.
    assert!(checker.require_can_manage_metadata(&member).is_err());
    // Granting the cap flips it.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &member,
            MemberCapabilities::CAN_MANAGE_METADATA.bits(),
        )
        .unwrap();
    assert!(checker.require_can_manage_metadata(&member).is_ok());
}

#[test]
fn metadata_set_does_not_change_group_state_hash() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let before = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::GroupMetadataSet {
            name: Some("renamed".to_owned()),
            data: {
                let mut d = std::collections::BTreeMap::new();
                let _ = d.insert("topic".to_owned(), "general".to_owned());
                d
            },
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();

    let after = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        before, after,
        "GroupMetadataSet must not perturb the group state hash"
    );
    assert_eq!(
        MetadataRepository::new(&store)
            .group_metadata(&gid)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("renamed")
    );
}

#[test]
fn member_metadata_self_set_allowed_others_gated() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::MemberCapabilities;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();

    let alice_sk = PrivateKey::random(&mut rng);
    let alice_pk = alice_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &alice_pk, GroupMemberRole::Member)
        .unwrap();
    let bob_sk = PrivateKey::random(&mut rng);
    let bob_pk = bob_sk.public_key();
    MembershipRepository::new(&store)
        .add_member(&gid, &bob_pk, GroupMemberRole::Member)
        .unwrap();

    // Alice sets her own metadata — allowed.
    let op = SignedGroupOp::sign(
        &alice_sk,
        gid_bytes.into(),
        vec![],
        1,
        GroupOp::MemberMetadataSet {
            member: alice_pk,
            name: Some("alice".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();

    // Alice tries to set Bob's metadata — rejected (no CAN_MANAGE_METADATA).
    let op_bad = SignedGroupOp::sign(
        &alice_sk,
        gid_bytes.into(),
        vec![],
        2,
        GroupOp::MemberMetadataSet {
            member: bob_pk,
            name: Some("not-bob".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());

    // Group-level metadata by a bare member — rejected.
    let op_group = SignedGroupOp::sign(
        &alice_sk,
        gid_bytes.into(),
        vec![],
        3,
        GroupOp::GroupMetadataSet {
            name: Some("nope".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_group).is_err());

    // Grant CAN_MANAGE_METADATA — now Alice can set Bob's and the group's.
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &gid,
            &alice_pk,
            MemberCapabilities::CAN_MANAGE_METADATA.bits(),
        )
        .unwrap();
    let op_ok = SignedGroupOp::sign(
        &alice_sk,
        gid_bytes.into(),
        vec![],
        4,
        GroupOp::GroupMetadataSet {
            name: Some("renamed".to_owned()),
            data: Default::default(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_ok).unwrap();
    assert_eq!(
        MetadataRepository::new(&store)
            .group_metadata(&gid)
            .unwrap()
            .and_then(|r| r.name)
            .as_deref(),
        Some("renamed")
    );
}

// ---------------------------------------------------------------------
// Trusted-anchor set — `trusted_anchors_for_group`
//
// Anchor set per RFC decision #22: `{Owner} ∪ {Admins} ∪ {ReadOnlyTee}`.
// These tests pin the membership rule against the materialized member
// set; the peer-selection wiring that consumes this is tested
// separately in the node crate.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// Cross-DAG state-hash claims on MemberRemoved / MemberLeft
//
// `MemberRemoved` and `MemberLeft` carry signed `expected_group_state_hash`
// and `expected_context_state_hashes`. Receivers compute the same hashes
// post-apply and compare; mismatch surfaces a structured warn log
// (does not roll back the apply). These tests pin the precomputation
// helpers and the equivalence between precomputed and actually-applied
// state hashes.
// ---------------------------------------------------------------------

#[test]
fn compute_group_state_hash_after_remove_matches_post_apply_hash() {
    // The sign-time precompute must produce the same hash that
    // `compute_group_state_hash` returns AFTER a real apply.
    // Without this equivalence, every honest receiver would observe a
    // spurious mismatch on every MemberRemoved.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let to_remove = PublicKey::from([0xB1; 32]);
    let bystander = PublicKey::from([0xB2; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &to_remove, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bystander, GroupMemberRole::Member)
        .unwrap();

    let precomputed = MetaRepository::new(&store)
        .compute_state_hash_after_remove(&gid, &to_remove)
        .unwrap();

    // Actually remove and recompute via the real helper.
    MembershipRepository::new(&store)
        .remove_member(&gid, &to_remove)
        .unwrap();
    let actual = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();

    assert_eq!(
        precomputed, actual,
        "precomputed post-remove hash must equal actually-applied hash"
    );
}

#[test]
fn compute_group_state_hash_after_remove_non_member_is_idempotent() {
    // If `removed_member` isn't currently in the set, the precompute
    // returns the same hash as `compute_group_state_hash` on the
    // current state. The op apply path bails on non-members
    // separately; this helper just hashes deterministically over
    // whatever it finds.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let stranger = PublicKey::from([0xCC; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();

    let current = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();
    let precomputed = MetaRepository::new(&store)
        .compute_state_hash_after_remove(&gid, &stranger)
        .unwrap();

    assert_eq!(current, precomputed);
}

#[test]
fn snapshot_context_state_hashes_returns_sorted_by_context_id() {
    // Deterministic ordering is required because the snapshot lands
    // inside a signed op whose content hash is the dedup key; a
    // non-deterministic order would produce different content hashes
    // for the same logical op and break gossip dedup.
    use calimero_store::key::ContextMeta;
    use calimero_store::types::ContextMeta as ContextMetaValue;

    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();

    // Register three contexts in non-sorted order, give each a
    // distinct root_hash so we can verify the values come back paired
    // with the right context.
    let cid_c = ContextId::from([0xCC; 32]);
    let cid_a = ContextId::from([0xAA; 32]);
    let cid_b = ContextId::from([0xBB; 32]);
    for cid in [cid_c, cid_a, cid_b] {
        register_context_in_group(&store, &gid, &cid).unwrap();
        let mut handle = store.handle();
        let meta = ContextMetaValue::new(
            calimero_store::key::ApplicationMeta::new(
                calimero_primitives::application::ApplicationId::from([0u8; 32]),
            ),
            *AsRef::<[u8; 32]>::as_ref(&cid),
            vec![],
            None,
        );
        handle.put(&ContextMeta::new(cid), &meta).unwrap();
    }

    let snapshot = MetaRepository::new(&store)
        .snapshot_context_state_hashes(&gid)
        .unwrap();
    let ids: Vec<ContextId> = snapshot.iter().map(|(c, _)| *c).collect();

    assert_eq!(
        ids,
        vec![cid_a, cid_b, cid_c],
        "must be sorted by ContextId"
    );
    // Per-entry root_hash matches the meta we wrote.
    for (cid, root) in &snapshot {
        assert_eq!(
            root,
            AsRef::<[u8; 32]>::as_ref(cid),
            "snapshot root_hash must equal the value in ContextMeta"
        );
    }
}

#[test]
fn snapshot_context_state_hashes_skips_unmaterialized_contexts() {
    // A context registered in the group index but missing its
    // ContextMeta (fresh node not joined yet) must be skipped, not
    // hashed as zeros — zero-hashing would force a spurious mismatch
    // on every receiver that has the context materialized.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();

    let cid = ContextId::from([0xAB; 32]);
    register_context_in_group(&store, &gid, &cid).unwrap();
    // Deliberately do NOT write a ContextMeta for this context.

    let snapshot = MetaRepository::new(&store)
        .snapshot_context_state_hashes(&gid)
        .unwrap();
    assert!(
        snapshot.is_empty(),
        "unmaterialized contexts must be skipped, got {snapshot:?}"
    );
}

#[test]
fn diff_sorted_context_hashes_pins_merge_scan_semantics() {
    // Pin the linear-merge divergence logic that replaced the
    // earlier two-`BTreeMap` build. Each case asserts on the
    // categorized buckets — hash_differs, only_in_expected,
    // only_in_actual — so the warn-log routing at the call site is
    // also covered.
    use super::diff_sorted_context_hashes;
    let group_id = test_group_id();
    let cid_a = ContextId::from([0x01; 32]);
    let cid_b = ContextId::from([0x02; 32]);
    let cid_c = ContextId::from([0x03; 32]);
    let cid_d = ContextId::from([0x04; 32]);
    let h_a = [0xAA; 32];
    let h_b = [0xBB; 32];
    let h_b_alt = [0xB0; 32];
    let h_c = [0xCC; 32];
    let h_d = [0xDD; 32];

    // Identical — every bucket empty.
    let expected = vec![(cid_a, h_a), (cid_b, h_b)];
    let actual = vec![(cid_a, h_a), (cid_b, h_b)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert!(diff.is_empty());

    // Same ids, different hash on one — that id lands in hash_differs
    // paired with the EXPECTED hash (reconcile uses this to verify
    // received state against the signed canonical value).
    let actual = vec![(cid_a, h_a), (cid_b, h_b_alt)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert_eq!(diff.hash_differs, vec![(cid_b, h_b)]);
    assert!(diff.only_in_expected.is_empty());
    assert!(diff.only_in_actual.is_empty());

    // Expected has an id actual lacks (fresh-node case) — only_in_expected.
    let expected = vec![(cid_a, h_a), (cid_b, h_b), (cid_c, h_c)];
    let actual = vec![(cid_a, h_a), (cid_c, h_c)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert!(diff.hash_differs.is_empty());
    assert_eq!(diff.only_in_expected, vec![cid_b]);
    assert!(diff.only_in_actual.is_empty());

    // Actual has an id expected lacks (receiver-ahead) — only_in_actual.
    let expected = vec![(cid_a, h_a), (cid_c, h_c)];
    let actual = vec![(cid_a, h_a), (cid_b, h_b), (cid_c, h_c)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert!(diff.hash_differs.is_empty());
    assert!(diff.only_in_expected.is_empty());
    assert_eq!(diff.only_in_actual, vec![cid_b]);

    // Mixed: one matching (cid_a), one hash-diff (cid_b) carrying
    // its expected hash, one only-in-expected (cid_c), one
    // only-in-actual (cid_d).
    let expected = vec![(cid_a, h_a), (cid_b, h_b), (cid_c, h_c)];
    let actual = vec![(cid_a, h_a), (cid_b, h_b_alt), (cid_d, h_d)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert_eq!(diff.hash_differs, vec![(cid_b, h_b)]);
    assert_eq!(diff.only_in_expected, vec![cid_c]);
    assert_eq!(diff.only_in_actual, vec![cid_d]);

    // One side empty — everything lands in the other bucket.
    let actual: Vec<(ContextId, [u8; 32])> = Vec::new();
    let expected = vec![(cid_a, h_a), (cid_b, h_b)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert_eq!(diff.only_in_expected, vec![cid_a, cid_b]);
    assert!(diff.hash_differs.is_empty());
    assert!(diff.only_in_actual.is_empty());

    let expected: Vec<(ContextId, [u8; 32])> = Vec::new();
    let actual = vec![(cid_a, h_a), (cid_b, h_b)];
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert_eq!(diff.only_in_actual, vec![cid_a, cid_b]);
    assert!(diff.hash_differs.is_empty());
    assert!(diff.only_in_expected.is_empty());

    // Both empty — every bucket empty.
    let expected: Vec<(ContextId, [u8; 32])> = Vec::new();
    let actual: Vec<(ContextId, [u8; 32])> = Vec::new();
    let diff = diff_sorted_context_hashes(&group_id, "test", &expected, &actual);
    assert!(diff.is_empty());
}

#[test]
#[should_panic(expected = "expected context-hash snapshot must be strictly sorted")]
fn diff_sorted_context_hashes_panics_on_unsorted_expected() {
    // Pins the sorted-input debug assertion. Catches dev / test
    // misuse before it becomes a quiet false-divergence-report bug.
    // The signed-op wire content hash is computed over the snapshot
    // as sorted, so an unsorted `expected` on the wire would have
    // failed dedup / verification upstream — this assertion is a
    // safety net for in-process callers.
    use super::diff_sorted_context_hashes;
    let group_id = test_group_id();
    let cid_a = ContextId::from([0x01; 32]);
    let cid_b = ContextId::from([0x02; 32]);
    let unsorted = vec![(cid_b, [0u8; 32]), (cid_a, [0u8; 32])];
    let sorted = vec![(cid_a, [0u8; 32]), (cid_b, [0u8; 32])];
    let _ = diff_sorted_context_hashes(&group_id, "test", &unsorted, &sorted);
}

#[test]
#[should_panic(expected = "actual context-hash snapshot must be strictly sorted")]
fn diff_sorted_context_hashes_panics_on_unsorted_actual() {
    use super::diff_sorted_context_hashes;
    let group_id = test_group_id();
    let cid_a = ContextId::from([0x01; 32]);
    let cid_b = ContextId::from([0x02; 32]);
    let sorted = vec![(cid_a, [0u8; 32]), (cid_b, [0u8; 32])];
    let unsorted = vec![(cid_b, [0u8; 32]), (cid_a, [0u8; 32])];
    let _ = diff_sorted_context_hashes(&group_id, "test", &sorted, &unsorted);
}

#[test]
fn apply_with_precomputed_real_hashes_matches_post_apply_view() {
    // End-to-end sanity check that the convergence pipeline closes
    // on the happy path: an admin precomputes the signed claims via
    // the real sign-time helpers, signs and applies a `MemberRemoved`,
    // and the receiver's post-apply state matches the signer's
    // simulation. Every other test in this crate uses the
    // `dummy_member_removed_op` helper which signs zeros — those
    // tests cover apply semantics but not the verify path, so a
    // future regression in the precompute-vs-actual equivalence
    // would slip through without this one.
    use calimero_context_client::local_governance::SignedGroupOp;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();
    let target_pk = PublicKey::from([0xD0; 32]);
    let bystander_pk = PublicKey::from([0xD1; 32]);

    // Bootstrap: a meta + admin + target + bystander member set.
    let mut meta = test_meta();
    meta.admin_identity = admin_pk;
    meta.owner_identity = admin_pk;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target_pk, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bystander_pk, GroupMemberRole::Member)
        .unwrap();

    // Real sign-time precomputation: admin's view of the post-apply
    // state, signed alongside the op.
    let expected_group_state_hash = MetaRepository::new(&store)
        .compute_state_hash_after_remove(&gid, &target_pk)
        .unwrap();
    let expected_context_state_hashes = MetaRepository::new(&store)
        .snapshot_context_state_hashes(&gid)
        .unwrap();

    let signed = SignedGroupOp::sign(
        &admin_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberRemoved {
            member: target_pk,
            expected_group_state_hash,
            expected_context_state_hashes: expected_context_state_hashes.clone(),
        },
    )
    .expect("sign MemberRemoved with real claims");

    // Apply on a sibling store that started from the same state.
    let receiver = test_store();
    MetaRepository::new(&receiver).save(&gid, &meta).unwrap();
    MembershipRepository::new(&receiver)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&receiver)
        .add_member(&gid, &target_pk, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&receiver)
        .add_member(&gid, &bystander_pk, GroupMemberRole::Member)
        .unwrap();
    apply_local_signed_group_op(&receiver, &signed).expect("apply MemberRemoved");

    // Receiver's actual post-apply hashes match the signed expected.
    // This is what `verify_post_apply_state_hashes` checks internally;
    // the apply succeeds with no warning when these match. If the
    // helpers ever drift from the live `compute_group_state_hash` /
    // `Snapshot::root_hash` semantics, this assertion catches it
    // before the warn-log path fires on every honest apply.
    let receiver_group_hash = MetaRepository::new(&receiver)
        .compute_state_hash(&gid)
        .unwrap();
    assert_eq!(
        receiver_group_hash, expected_group_state_hash,
        "receiver's post-apply group state hash must equal the signer's pre-apply simulation"
    );
    let receiver_context_hashes = MetaRepository::new(&receiver)
        .snapshot_context_state_hashes(&gid)
        .unwrap();
    assert_eq!(
        receiver_context_hashes, expected_context_state_hashes,
        "receiver's post-apply per-context snapshot must equal the signer's"
    );
}

#[test]
fn cascade_remove_member_does_not_change_group_state_hash() {
    // Pins the invariant relied on by the ordering comment at the
    // `verify_post_apply_state_hashes` call site: cascade-removal
    // touches `ContextIdentity` rows only, never `GroupMeta` or
    // `GroupMember` rows, so the group state hash before and after
    // a cascade is identical. If a future refactor makes cascade
    // also touch group-level rows, this test fires and the ordering
    // comment in `apply_group_op_mutations` must be revisited
    // (otherwise the post-apply hash would diverge from the
    // pre-apply simulation on every honest receiver).
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xD0; 32]);
    let context_id = ContextId::from([0xE0; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();

    // Register a context and write a ContextIdentity for `target`
    // — exactly the row cascade_remove_member_from_group_tree
    // deletes.
    register_context_in_group(&store, &gid, &context_id).unwrap();
    let id_key = calimero_store::key::ContextIdentity::new(context_id, target);
    let mut handle = store.handle();
    handle
        .put(
            &id_key,
            &calimero_store::types::ContextIdentity {
                private_key: None,
                sender_key: None,
            },
        )
        .unwrap();
    drop(handle);

    let hash_before = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();
    cascade_remove_member_from_group_tree(&store, &gid, &target).unwrap();
    let hash_after = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();

    assert_eq!(
        hash_before, hash_after,
        "cascade-removal must not change the group state hash — \
         it touches ContextIdentity rows, which are not in the hash inputs"
    );
    // Sanity: the row it WAS supposed to delete is gone.
    let handle = store.handle();
    assert!(
        !handle.has(&id_key).unwrap(),
        "cascade should have deleted target's ContextIdentity row"
    );
}

#[test]
fn mark_denied_does_not_change_group_state_hash() {
    // Mirrors the cascade invariant test: the verify-call-site
    // ordering comment claims `mark_denied` doesn't touch
    // `compute_group_state_hash`'s inputs. This pins it — a future
    // refactor that moves the denial flag into the `GroupMember`
    // row (instead of a separate `GroupDeniedMember` column) would
    // trip this test and force a rethink of where the verify call
    // sits relative to other mutations.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xD0; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .remove_member(&gid, &target)
        .unwrap();

    let hash_before = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();
    DenyListRepository::new(&store).mark(&gid, &target).unwrap();
    let hash_after = MetaRepository::new(&store)
        .compute_state_hash(&gid)
        .unwrap();

    assert_eq!(
        hash_before, hash_after,
        "mark_denied must not change the group state hash — \
         it writes a GroupDeniedMember row, not GroupMeta or GroupMember"
    );
}

#[test]
fn compute_group_state_hash_after_remove_never_returns_zeros_for_real_group() {
    // SHA-256 of any real `GroupMeta` + member set is astronomically
    // unlikely to produce all-zeros. This test pins the practical
    // guarantee: for a populated group with a real meta and at least
    // one member, the precomputed post-remove hash must NOT be the
    // sentinel value `[0u8; 32]`. If a future bug short-circuits the
    // hasher and returns zeros, every receiver would silently treat
    // the signed claim as "no claim" and the convergence check would
    // be effectively disabled. This catches that class of regression
    // at the helper boundary.
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xD0; 32]);
    let bystander = PublicKey::from([0xD1; 32]);

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bystander, GroupMemberRole::Member)
        .unwrap();

    let hash = MetaRepository::new(&store)
        .compute_state_hash_after_remove(&gid, &target)
        .unwrap();
    assert_ne!(
        hash, [0u8; 32],
        "post-remove hash collided with the no-claim sentinel — \
         convergence check would be silently disabled"
    );
}

#[test]
fn apply_group_op_mutations_surfaces_divergence_on_hash_mismatch() {
    // The verify path surfaces a structured `DivergenceReport` up
    // through `apply_group_op_mutations` so the node handler can
    // route it to the reconcile-via-anchor path. Without this
    // plumbing, the existing warn log would be the only signal and
    // recovery would require operator intervention.
    use super::apply_group_op_mutations;

    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xD0; 32]);
    let bystander = PublicKey::from([0xD1; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bystander, GroupMemberRole::Member)
        .unwrap();

    // Sign-time would precompute the real post-apply hash. Here we
    // deliberately supply a wrong one — the receiver's apply will
    // recompute and detect the mismatch.
    let wrong_hash = [0xFFu8; 32];
    let op = GroupOp::MemberRemoved {
        member: target,
        expected_group_state_hash: wrong_hash,
        expected_context_state_hashes: Vec::new(),
    };

    let (handled, divergence, _pending_events) = apply_group_op_mutations(
        &store,
        &gid,
        &admin,
        &op,
        &[],
        &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
    )
    .unwrap();
    assert!(handled, "MemberRemoved should be handled");
    let report = divergence.expect("hash mismatch must produce a DivergenceReport");
    assert!(
        report.group_hash_diverges,
        "group hash should diverge from the wrong expected"
    );
    assert_eq!(report.op_kind, "MemberRemoved");
    assert_eq!(report.group_id, gid);
}

#[test]
fn apply_group_op_mutations_no_divergence_on_matching_hash() {
    // Mirror test: when the signed expected hash matches the real
    // post-apply hash, the apply path returns `None` for divergence
    // and no reconcile fires.
    use super::apply_group_op_mutations;

    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let target = PublicKey::from([0xD0; 32]);
    let bystander = PublicKey::from([0xD1; 32]);

    let mut meta = test_meta();
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &target, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &bystander, GroupMemberRole::Member)
        .unwrap();

    let real_post_apply_hash = MetaRepository::new(&store)
        .compute_state_hash_after_remove(&gid, &target)
        .unwrap();
    let op = GroupOp::MemberRemoved {
        member: target,
        expected_group_state_hash: real_post_apply_hash,
        expected_context_state_hashes: Vec::new(),
    };

    let (handled, divergence, _pending_events) = apply_group_op_mutations(
        &store,
        &gid,
        &admin,
        &op,
        &[],
        &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
    )
    .unwrap();
    assert!(handled);
    assert!(
        divergence.is_none(),
        "no divergence expected when hashes match, got {divergence:?}"
    );
}

// -----------------------------------------------------------------------
// Effective capabilities for `Open` subgroups — issue #2378
//
// Sibling of #2371/#2372. `get_member_capabilities` (admin-api) gates on
// `get_effective_member_capabilities`. An inherited Open-subgroup joiner
// has no stored `GroupMember` row (`execute_member_joined_open` is
// validate-only), yet `check_group_membership` reports them as a member —
// the same effective-membership contract `list_group_members` honours
// post-#2372. The gate must therefore recognise inherited members and
// report their effective per-member bitmask as `0` ("member, no extra
// delegated bits"). It must NOT change direct-member or non-member
// answers, and a `Restricted` subgroup must remain a wall.
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// `subgroup_visible_to` — the visibility decision behind the
// `list_subgroups` admin endpoint (PR #2361). `Open` children are
// public; `Restricted` children are listed only for the parent-group
// admin or a direct member of the child. These pin every cell of the
// visibility matrix the handler relies on.
// -----------------------------------------------------------------------

// ---------------------------------------------------------------------
// is_tee_admitted_identity
// ---------------------------------------------------------------------

#[test]
fn is_tee_admitted_identity_matches_tee_joined_member() {
    let store = test_store();
    let mut rng = rand::thread_rng();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let tee_node = PublicKey::from([0x42; 32]);
    let ordinary = PublicKey::from([0x43; 32]);

    let signer_sk = PrivateKey::random(&mut rng);
    let tee_op = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberJoinedViaTeeAttestation {
            member: tee_node,
            quote_hash: [0x01; 32],
            mrtd: "m1".to_owned(),
            rtmr0: "r0".to_owned(),
            rtmr1: "r1".to_owned(),
            rtmr2: "r2".to_owned(),
            rtmr3: "r3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 1, &borsh::to_vec(&tee_op).unwrap()).unwrap();

    assert!(is_tee_admitted_identity(&store, &gid, &tee_node).unwrap());
    assert!(!is_tee_admitted_identity(&store, &gid, &ordinary).unwrap());
}

// ---------------------------------------------------------------------
// is_authoritative_namespace_identity
// ---------------------------------------------------------------------

#[cfg(test)]
mod auto_follow_tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::apply_local_signed_group_op;

    fn seed(
        rng: &mut OsRng,
    ) -> (
        calimero_store::Store,
        calimero_context_config::types::ContextGroupId,
        [u8; 32],
        PrivateKey,
        PrivateKey,
    ) {
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(rng);
        let member_sk = PrivateKey::random(rng);
        MembershipRepository::new(&store)
            .add_member(&gid, &admin_sk.public_key(), GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &member_sk.public_key(), GroupMemberRole::Member)
            .unwrap();
        (store, gid, gid_bytes, admin_sk, member_sk)
    }

    #[test]
    fn admin_can_set_member_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = MembershipRepository::new(&store)
            .member_value(&gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts);
        assert!(val.auto_follow.subgroups);
    }

    #[test]
    fn member_can_set_own_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, _admin_sk, member_sk) = seed(&mut rng);

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = MembershipRepository::new(&store)
            .member_value(&gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts);
        assert!(!val.auto_follow.subgroups);
    }

    #[test]
    fn non_admin_cannot_set_others_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, _admin_sk, member_sk) = seed(&mut rng);

        // `other_sk` is a real member of the group — we add them first so
        // the authorization check is the reason the op is rejected, not a
        // missing-target lookup. If the handler's check order is ever
        // refactored to look up the target before checking auth, this
        // test would still correctly assert "non-admin, non-self rejected".
        let other_sk = PrivateKey::random(&mut rng);
        MembershipRepository::new(&store)
            .add_member(&gid, &other_sk.public_key(), GroupMemberRole::Member)
            .unwrap();

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: other_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::AutoFollowAuthFailed)
        ));

        // Sanity: the target's flags were not mutated by the
        // rejected op. The target was added via the seed() helper
        // which uses `add_group_member` directly — with the new
        // default that means {contexts: true, subgroups: false}.
        // The point of this test is that the failed op didn't
        // SHIFT the values, not that they were originally false.
        let val = MembershipRepository::new(&store)
            .member_value(&gid, &other_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts, "default contexts=true preserved");
        assert!(
            !val.auto_follow.subgroups,
            "default subgroups=false preserved"
        );
    }

    #[test]
    fn rejects_non_member_target() {
        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _member_sk) = seed(&mut rng);
        let stranger = PrivateKey::random(&mut rng).public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: stranger,
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<MembershipError>(),
            Some(MembershipError::NotMember { .. })
        ));
    }

    #[test]
    fn default_flags_match_default_impl_and_preserved_on_role_change() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        // Initial state matches `AutoFollowFlags::default()`. Post-#2422
        // that's {contexts: true, subgroups: false} — explicit assertion
        // on the exact shape so a future default flip can't slip through.
        let before = MembershipRepository::new(&store)
            .member_value(&gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(before.auto_follow.contexts);
        assert!(!before.auto_follow.subgroups);

        // Member turns on contexts
        let op1 = SignedGroupOp::sign(
            &member_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op1).unwrap();

        // Admin changes role — flags must survive
        let op2 = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberRoleSet {
                member: member_sk.public_key(),
                role: GroupMemberRole::ReadOnly,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op2).unwrap();

        let after = MembershipRepository::new(&store)
            .member_value(&gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert_eq!(after.role, GroupMemberRole::ReadOnly);
        assert!(after.auto_follow.contexts);
    }

    /// End-to-end path without the actor:
    ///   add_group_member → MemberSetAutoFollow → ContextRegistered.
    ///
    /// Asserts every stage lands in the store correctly and that the
    /// op-apply event channel fires the events the Phase 3 handler
    /// depends on. Exercises the full Phase 1–4 wiring short of the
    /// actor-driven `join_context` call, which needs a full merod
    /// instance (covered by the deferred merobox e2e workflow).
    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn end_to_end_event_fires_after_op_apply() {
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::blobs::BlobId;
        use calimero_primitives::context::ContextId;

        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        // Subscribe BEFORE applying ops so we don't miss events.
        let mut rx = op_events::subscribe();

        // 1. MemberSetAutoFollow on self
        let set_flags = SignedGroupOp::sign(
            &member_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &set_flags).unwrap();

        // Verify state landed
        let value = MembershipRepository::new(&store)
            .member_value(&gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(value.auto_follow.contexts);
        assert!(value.auto_follow.subgroups);

        // 2. ContextRegistered op (admin registers a new context).
        let context_id = ContextId::from([0x77; 32]);
        let register = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::ContextRegistered {
                context_id,
                application_id: ApplicationId::from([0xAA; 32]),
                blob_id: BlobId::from([0xBB; 32]),
                source: "test://app".to_owned(),
                service_name: None,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &register).unwrap();

        // 3. The handler sees two events: AutoFollowSet + ContextRegistered.
        //    Drain events and assert both fired with the right payloads.
        //    The channel is process-wide so other tests may interleave —
        //    filter by our tag.
        // Match on (group_id, member_pk) for AutoFollowSet and on
        // (group_id, context_id) for ContextRegistered — other tests
        // running in parallel share the same global event channel and
        // `test_group_id()`, so group_id alone is not a unique filter.
        let expected_member = member_sk.public_key();
        let mut saw_auto_follow = false;
        let mut saw_context_registered = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline && !(saw_auto_follow && saw_context_registered) {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(OpEvent::AutoFollowSet {
                    group_id,
                    member,
                    contexts,
                    subgroups,
                })) if group_id == gid_bytes && member == expected_member => {
                    assert!(contexts);
                    assert!(subgroups);
                    saw_auto_follow = true;
                }
                Ok(Ok(OpEvent::ContextRegistered {
                    group_id,
                    context_id: got,
                })) if group_id == gid_bytes && got == context_id => {
                    saw_context_registered = true;
                }
                Ok(Ok(_)) => {} // other events from parallel tests
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        assert!(saw_auto_follow, "AutoFollowSet event should have fired");
        assert!(
            saw_context_registered,
            "ContextRegistered event should have fired"
        );
    }

    /// #2422 Option 2: a `GroupOp::MemberAdded` apply path now ALSO
    /// emits a synthesized `OpEvent::AutoFollowSet { contexts: true }`
    /// when the freshly-written member row carries the new default
    /// (`AutoFollowFlags::default() == {contexts: true, subgroups: false}`).
    /// Without this, the auto-follow handler would only react to
    /// FUTURE `OpEvent::ContextRegistered` events — pre-existing
    /// contexts in the group at join-time would be missed.
    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn member_added_emits_synthesized_auto_follow_set() {
        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _existing_member_sk) = seed(&mut rng);

        // Subscribe BEFORE applying ops so the broadcast channel
        // doesn't drop events we care about.
        let mut rx = op_events::subscribe();

        // A brand-new joiner — not in the seed() pair.
        let new_member_sk = PrivateKey::random(&mut rng);
        let new_member_pk = new_member_sk.public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberAdded {
                member: new_member_pk,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        // Verify the storage-side fix landed first: the new member's
        // row has `auto_follow.contexts == true` via the new Default.
        let value = MembershipRepository::new(&store)
            .member_value(
                &calimero_context_config::types::ContextGroupId::from(gid_bytes),
                &new_member_pk,
            )
            .unwrap()
            .unwrap();
        assert!(
            value.auto_follow.contexts,
            "new member should default to contexts=true post-#2422"
        );
        assert!(
            !value.auto_follow.subgroups,
            "subgroups stays false (TEE-only path until non-TEE admission op exists)"
        );

        // Now drain events and confirm both `MemberAdded` and the
        // synthesized `AutoFollowSet` fired for this exact member.
        // Other tests in the same process share the global event
        // channel, so filter on `member == new_member_pk`. The
        // deadline is generous (10s) so the test stays reliable
        // under heavy parallel-test load on CI — the events are
        // emitted synchronously from `apply_local_signed_group_op`
        // before we even start polling, so on an unloaded run the
        // first `recv()` returns immediately.
        let mut saw_member_added = false;
        let mut saw_auto_follow = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && !(saw_member_added && saw_auto_follow) {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(OpEvent::MemberAdded {
                    group_id, member, ..
                })) if group_id == gid_bytes && member == new_member_pk => {
                    saw_member_added = true;
                }
                Ok(Ok(OpEvent::AutoFollowSet {
                    group_id,
                    member,
                    contexts,
                    subgroups,
                })) if group_id == gid_bytes && member == new_member_pk => {
                    assert!(
                        contexts,
                        "synthesized AutoFollowSet must carry contexts=true (Option 2)"
                    );
                    assert!(
                        !subgroups,
                        "synthesized AutoFollowSet mirrors stored subgroups (false by default)"
                    );
                    saw_auto_follow = true;
                }
                Ok(Ok(_)) => {} // unrelated events from parallel tests
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        assert!(
            saw_member_added,
            "MemberAdded event should have fired for the new joiner"
        );
        assert!(
            saw_auto_follow,
            "synthesized AutoFollowSet should have fired for the new joiner (#2422 Option 2)"
        );
    }

    /// Verifies the opt-out path is preserved: if a member is added
    /// and then their contexts flag is explicitly turned off via
    /// `MemberSetAutoFollow`, the stored row reflects false. The
    /// synthesized `AutoFollowSet` from `MemberAdded` carries the
    /// default true, but a subsequent explicit SetMemberAutoFollow
    /// must be honored.
    #[test]
    fn explicit_opt_out_after_member_added_is_preserved() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, _) = seed(&mut rng);

        let target_sk = PrivateKey::random(&mut rng);
        let target_pk = target_sk.public_key();

        // Add member — picks up the new default {true, false}
        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(
                &admin_sk,
                gid_bytes.into(),
                vec![],
                1,
                GroupOp::MemberAdded {
                    member: target_pk,
                    role: GroupMemberRole::Member,
                },
            )
            .unwrap(),
        )
        .unwrap();

        // Explicit opt-out (member acts on self)
        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(
                &target_sk,
                gid_bytes.into(),
                vec![],
                1,
                GroupOp::MemberSetAutoFollow {
                    target: target_pk,
                    auto_follow_contexts: false,
                    auto_follow_subgroups: false,
                },
            )
            .unwrap(),
        )
        .unwrap();

        let value = MembershipRepository::new(&store)
            .member_value(&gid, &target_pk)
            .unwrap()
            .unwrap();
        assert!(
            !value.auto_follow.contexts,
            "explicit opt-out via SetMemberAutoFollow must stick"
        );
    }

    /// #2770: when `MemberAdded` fires, the op-log entry must already be
    /// persisted. Before the post-persist drain, the handler emitted the
    /// event BEFORE the op-log append, so a subscriber reacting to the
    /// event could read a log that did not yet contain the op.
    ///
    /// The event is delivered on a process-wide `tokio::broadcast`, and
    /// `apply_local_signed_group_op` sends synchronously. To catch the
    /// ordering (not merely the post-apply steady state), a dedicated
    /// observer thread parks on `recv()` and snapshots the op-log THE
    /// MOMENT the event arrives — concurrently with the apply call still
    /// running on this thread. Pre-fix (emit-before-persist) the observer
    /// wakes during the gap between `notify` and `persist_*` and reads a
    /// log that does NOT yet contain the op; post-fix the drain runs only
    /// after the append, so the observer always reads `true`.
    ///
    /// NOTE: this is a DETERMINISTIC GREEN (proves the fixed persist-then-notify
    /// ordering) but only a BEST-EFFORT RED (regression detection) — see the
    /// `Barrier` comment in the body for why a fully deterministic RED would need
    /// a test-only sync hook in the apply hot path, which we deliberately omit.
    #[test]
    #[serial_test::serial]
    fn member_added_event_fires_after_op_log_append() {
        use std::sync::{Arc, Barrier};

        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, _existing_member_sk) = seed(&mut rng);

        // Subscribe BEFORE spawning the observer / applying.
        let mut rx = op_events::subscribe();

        let new_member_sk = PrivateKey::random(&mut rng);
        let new_member_pk = new_member_sk.public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberAdded {
                member: new_member_pk,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();
        let content_hash = op.content_hash().unwrap();

        // Observer thread: blocks on `recv()`, then snapshots whether the
        // op-log already contains our op at the instant the event fires.
        // `Store` is `Clone` (shared handle to the same backing DB), so the
        // snapshot reads the same state the applying thread is writing.
        let observer_store = store.clone();
        // Rendezvous: both threads wait here so the observer enters its
        // `try_recv` spin loop concurrently with the applier, not after the
        // applier has finished (the window the old pre-loop ready-signal left
        // open).
        //
        // RED/GREEN strength: for the FIXED code (persist-then-notify) this is a
        // DETERMINISTIC green — the event can only be observed after the append,
        // so the snapshot is always `true` regardless of thread timing. As a RED
        // regression-catcher it is best-effort: in principle a regression
        // (notify-then-persist) is only caught if the observer reads the event
        // inside the notify→persist window. In practice that window is reliably
        // hit because after the barrier the applier must run the membership
        // mutations AND the op-log append (substantial store work) before it
        // reaches `notify`, whereas the observer only needs to enter a tight
        // spin loop — orders of magnitude less work — so it is already polling
        // long before the event fires. A FULLY deterministic RED would need a
        // synchronization point injected BETWEEN the append and the flush in the
        // production apply path; we deliberately do not add a test-only hook to
        // that hot path. The structural guarantee is the persist-then-flush
        // ordering visible in `apply_local_signed_group_op` itself.
        let barrier = Arc::new(Barrier::new(2));
        let observer_barrier = Arc::clone(&barrier);
        let observer = std::thread::spawn(move || {
            observer_barrier.wait();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                match rx.try_recv() {
                    Ok(OpEvent::MemberAdded {
                        group_id, member, ..
                    }) if group_id == gid_bytes && member == new_member_pk => {
                        return Some(
                            crate::local_state::op_log_contains_content_hash(
                                &observer_store,
                                &gid,
                                &content_hash,
                            )
                            .unwrap(),
                        );
                    }
                    Ok(_) => {} // unrelated events from parallel tests
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                        // `yield_now` (not `spin_loop`): poll tightly but let the
                        // scheduler run the applier thread, so a single-core or
                        // cooperative scheduler can't starve it into the 10s
                        // deadline. Still polls fast enough to snapshot close to
                        // the broadcast send.
                        std::thread::yield_now();
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        // Capacity-induced miss (parallel tests flooded the
                        // channel). Fail loudly rather than returning `None`,
                        // which the caller would misreport as "event never
                        // fired" and bury the real cause.
                        panic!(
                            "observer lagged by {n} events before MemberAdded — test inconclusive"
                        );
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
                }
            }
        });

        // Release the observer and immediately apply: the observer is now
        // spinning on `try_recv`, racing the applier's persist.
        barrier.wait();
        apply_local_signed_group_op(&store, &op).unwrap();

        let log_contained_at_emit = observer
            .join()
            .unwrap()
            .expect("MemberAdded event should have fired for the new joiner");
        assert!(
            log_contained_at_emit,
            "op-log entry must be persisted before MemberAdded fires (#2770)"
        );
    }

    /// #2770: re-applying an already-logged op must NOT re-fire its event.
    /// The queued events are dropped on the content-hash dedup early-return.
    #[test]
    #[serial_test::serial]
    fn replayed_group_op_does_not_re_emit() {
        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _existing_member_sk) = seed(&mut rng);

        let new_member_sk = PrivateKey::random(&mut rng);
        let new_member_pk = new_member_sk.public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes.into(),
            vec![],
            1,
            GroupOp::MemberAdded {
                member: new_member_pk,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();

        // Subscribe BEFORE the first apply so the receiver is in place ahead of
        // every emit — there is no subscribe-to-apply gap a parallel-test flood
        // could exploit. We then count how many `MemberAdded` events for
        // (gid, member) appear across BOTH applies: the first must emit exactly
        // one, the replay (already-logged) must emit none, so the total is 1.
        // This is stronger than asserting absence — it doubles as a POSITIVE
        // control (an event mechanism that silently fired zero times would fail
        // here too) — and it is timing-independent because both applies and
        // `op_events::notify` are synchronous on THIS thread, so all events are
        // buffered before the single drain below.
        let mut rx = op_events::subscribe();
        apply_local_signed_group_op(&store, &op).unwrap(); // first apply emits
        apply_local_signed_group_op(&store, &op).unwrap(); // replay (already logged)

        let mut member_added = 0usize;
        loop {
            match rx.try_recv() {
                Ok(OpEvent::MemberAdded {
                    group_id, member, ..
                }) if group_id == gid_bytes && member == new_member_pk => member_added += 1,
                Ok(_) => {} // unrelated events from parallel tests
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    // A capacity flood (1024) from parallel tests could drop the
                    // events we count — fail loudly rather than under-counting
                    // into a false pass.
                    panic!("drain lagged by {n} events — replay re-emit check inconclusive");
                }
                Err(_) => break, // Empty (drained) or Closed — nothing more to read
            }
        }
        assert_eq!(
            member_added, 1,
            "first apply must emit MemberAdded exactly once and the replay must \
             not re-emit it (#2770)"
        );
    }

    /// #2770 (namespace RootOp path): when `SubgroupCreated` fires, the
    /// namespace op-log entry for the driving `RootOp::GroupCreated` must
    /// already be persisted. Same concurrent-observer technique as
    /// `member_added_event_fires_after_op_log_append`, but on the
    /// namespace-apply path (`NamespaceGovernance::apply_signed_op` →
    /// `dispatch_root_op` → `group_created::apply`): a dedicated observer
    /// thread parks on `recv()` and snapshots
    /// `NamespaceOpLogService::contains_op(delta_id)` the instant the
    /// event arrives — concurrently with the apply still running. Pre-fix
    /// (the dormant direct `notify_op_event` in `group_created::apply`)
    /// the observer would wake before `store_operation`, reading a log
    /// that does NOT yet contain the op; post-fix the events are drained
    /// only after the namespace op is appended, so the snapshot is always
    /// `true`.
    ///
    /// NOTE: deterministic GREEN, best-effort RED — same characterization as
    /// `member_added_event_fires_after_op_log_append` (see its body comment).
    #[test]
    #[serial_test::serial]
    fn subgroup_created_event_fires_after_namespace_op_persist() {
        use std::sync::{Arc, Barrier};

        use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

        use super::NamespaceGovernance;
        use crate::op_events::{self, OpEvent};
        use crate::NamespaceOpLogService;

        let mut rng = OsRng;
        let admin_sk = PrivateKey::random(&mut rng);
        let admin_sk_bytes: [u8; 32] = *admin_sk.as_bytes();
        let admin_pk = admin_sk.public_key();

        let ns_id = [0xA0u8; 32];
        let ns_gid = calimero_context_config::types::ContextGroupId::from(ns_id);
        let new_group_id = [0xCCu8; 32];

        // Minimal namespace root: admin meta + admin membership + the
        // local namespace identity (so the originator-style apply path is
        // exercised end to end).
        let store = test_store();
        MetaRepository::new(&store)
            .save(&ns_gid, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        NamespaceRepository::new(&store)
            .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
            .unwrap();

        let op = SignedNamespaceOp::sign(
            &admin_sk,
            ns_id.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::GroupCreated {
                group_id: new_group_id.into(),
                parent_id: ns_id.into(),
                restricted: true,
            }),
        )
        .unwrap();
        // `delta_id` is the op's content hash — the key the namespace
        // op-log is indexed by (see `NamespaceOpLogService::store_signed_operation`).
        let delta_id = op.content_hash().unwrap();

        // Subscribe BEFORE spawning the observer / applying.
        let mut rx = op_events::subscribe();

        // Observer thread: blocks on `recv()`, then snapshots whether the
        // namespace op-log already contains the op at the instant the
        // `SubgroupCreated` event fires. `Store` is `Clone` (shared handle
        // to the same backing DB), so the snapshot reads the same state
        // the applying thread is writing.
        let observer_store = store.clone();
        // Rendezvous: observer enters its `try_recv` spin loop concurrently with
        // the applier. Same RED/GREEN characterization as the group-path test
        // (`member_added_event_fires_after_op_log_append`): deterministic GREEN
        // for the fixed persist-then-notify ordering, best-effort RED for a
        // regression (the applier runs the RootOp mutations + `store_operation`
        // before `notify`, so the observer is reliably spinning first); a fully
        // deterministic RED would need a hot-path test hook we deliberately omit.
        let barrier = Arc::new(Barrier::new(2));
        let observer_barrier = Arc::clone(&barrier);
        let observer = std::thread::spawn(move || {
            observer_barrier.wait();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                match rx.try_recv() {
                    Ok(OpEvent::SubgroupCreated {
                        namespace_id,
                        parent_group_id,
                        child_group_id,
                    }) if namespace_id == ns_id.into()
                        && parent_group_id == ns_id
                        && child_group_id == new_group_id =>
                    {
                        return Some(
                            NamespaceOpLogService::new(&observer_store, ns_id.into())
                                .contains_op(delta_id)
                                .unwrap(),
                        );
                    }
                    Ok(_) => {} // unrelated events from parallel tests
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                        // `yield_now` (not `spin_loop`): poll tightly but let the
                        // scheduler run the applier thread (single-core / coop
                        // scheduler safety). Still snapshots close to the send.
                        std::thread::yield_now();
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        // Capacity-induced miss: fail loudly rather than
                        // misreporting it as "event never fired".
                        panic!("observer lagged by {n} events before SubgroupCreated — test inconclusive");
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
                }
            }
        });

        // Release the observer and immediately apply.
        barrier.wait();
        NamespaceGovernance::new(&store, ns_id.into())
            .apply_signed_op(&op)
            .unwrap();

        let log_contained_at_emit = observer
            .join()
            .unwrap()
            .expect("SubgroupCreated event should have fired for the new subgroup");
        assert!(
            log_contained_at_emit,
            "namespace op-log entry must be persisted before SubgroupCreated fires (#2770)"
        );
    }
}

// ---------------------------------------------------------------------
// Role-scoped `TeeMemberRemoved` op-event emission (apply-path).
//
// `OpEvent::TeeMemberRemoved` is fired ALONGSIDE `OpEvent::MemberRemoved`
// from the apply path whenever the removed member's stored role was
// `ReadOnlyTee`. This is the wake-up signal for the
// `calimero_context::self_purge` listener (TEE eviction → hard-purge).
// For non-TEE removals only `MemberRemoved` is emitted (soft-leave
// path preserved for kick-and-readd / rejoin-via-keyshare /
// inheritance-rejoin workflows).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tee_member_removed_event_tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::apply_local_signed_group_op;
    use crate::op_events::{self, OpEvent};

    /// Drain events from `rx` for up to 500ms, counting how many
    /// `MemberRemoved` and `TeeMemberRemoved` events landed for our
    /// `(gid, member)` tuple. Other in-flight events (from parallel
    /// tests on the process-wide channel) are filtered out by the
    /// tuple-match guard.
    /// Count `MemberRemoved` / `TeeMemberRemoved` events emitted for
    /// `(gid_bytes, member)`.
    ///
    /// Deterministic, not time-based. `op_events::notify` performs a
    /// synchronous `broadcast::Sender::send` from inside the apply arm, so
    /// once `apply_local_signed_group_op` has returned every event the op
    /// emitted is already sitting in `rx`'s buffer. Draining with
    /// `try_recv` until the buffer is empty therefore observes the
    /// complete, final event set with no polling window — no 500ms sleep,
    /// no CI-load flake (the old fixed-duration poll could both waste
    /// 500ms on the happy path and, under load, miss a late event). The
    /// `group_id`/`member` filter still discards events that parallel
    /// tests interleave onto the process-wide channel.
    ///
    fn count_removed_events_for(
        rx: &mut tokio::sync::broadcast::Receiver<OpEvent>,
        gid_bytes: [u8; 32],
        member: PublicKey,
    ) -> (usize, usize) {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut member_removed = 0;
        let mut tee_member_removed = 0;
        loop {
            match rx.try_recv() {
                Ok(OpEvent::MemberRemoved {
                    group_id,
                    member: m,
                }) if group_id == gid_bytes && m == member => {
                    member_removed += 1;
                }
                Ok(OpEvent::TeeMemberRemoved {
                    group_id,
                    member: m,
                }) if group_id == gid_bytes && m == member => {
                    tee_member_removed += 1;
                }
                Ok(_) => {} // unrelated parallel-test events
                // Parallel tests overran the shared buffer. Keep draining;
                // `try_recv` resumes at the oldest still-buffered event. If
                // our own events were the ones dropped the counts come up
                // short and the assertion fails loudly — never a silent
                // flaky pass.
                Err(TryRecvError::Lagged(_)) => continue,
                // Empty or Closed: nothing left to drain.
                Err(_) => break,
            }
        }
        (member_removed, tee_member_removed)
    }

    /// Like [`count_removed_events_for`] but tallies TWO `(gid, member)`
    /// tuples in a SINGLE drain. `count_removed_events_for` drains `rx`
    /// to empty, so calling it twice (once per gid) after one apply
    /// loses the second gid's events. The cascade tests need to assert
    /// both the root and the subgroup pair from one apply, so they must
    /// share one drain.
    #[allow(clippy::type_complexity)]
    fn count_removed_events_for_two(
        rx: &mut tokio::sync::broadcast::Receiver<OpEvent>,
        gid_a: [u8; 32],
        gid_b: [u8; 32],
        member: PublicKey,
    ) -> ((usize, usize), (usize, usize)) {
        use tokio::sync::broadcast::error::TryRecvError;
        let (mut a_mr, mut a_tmr) = (0, 0);
        let (mut b_mr, mut b_tmr) = (0, 0);
        loop {
            match rx.try_recv() {
                Ok(OpEvent::MemberRemoved {
                    group_id,
                    member: m,
                }) if m == member => {
                    if group_id == gid_a {
                        a_mr += 1;
                    } else if group_id == gid_b {
                        b_mr += 1;
                    }
                    // Events on any OTHER group_id are intentionally dropped:
                    // `op_events` is a PROCESS-GLOBAL broadcast bus. The cascade
                    // callers are `#[serial_test::serial]` (matching #2808), so
                    // no other serial op-event test emits concurrently — but
                    // serial does NOT exclude parallel *non-serial* tests, which
                    // could still reuse this `member` key on the bus. An assert-
                    // "no unexpected group_id" guard is therefore still unsound.
                    // The exact-count assertions in the callers are nonetheless
                    // contamination-proof because each caller's `gid_a`/`gid_b`
                    // are byte patterns UNIQUE to that test and we filter
                    // strictly on them: any other test sharing the member key
                    // emits on its own (different) group_ids, which fall through
                    // uncounted. Over-cascade WITHIN a test's own topology is
                    // what the callers pin via those exact counts.
                }
                Ok(OpEvent::TeeMemberRemoved {
                    group_id,
                    member: m,
                }) if m == member => {
                    if group_id == gid_a {
                        a_tmr += 1;
                    } else if group_id == gid_b {
                        b_tmr += 1;
                    }
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        ((a_mr, a_tmr), (b_mr, b_tmr))
    }

    /// Removing a `ReadOnlyTee` member via `GroupOp::MemberRemoved`
    /// must emit BOTH `MemberRemoved` and `TeeMemberRemoved` for the
    /// same `(group_id, member)`.
    #[test]
    #[serial_test::serial]
    fn member_removed_op_emits_tee_event_for_readonly_tee_role() {
        let store = test_store();
        let gid = test_group_id();
        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin_pk = admin_sk.public_key();
        let tee_pk = PublicKey::from([0xE1; 32]);

        let mut meta = test_meta();
        meta.admin_identity = admin_pk;
        meta.owner_identity = admin_pk;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &tee_pk, GroupMemberRole::ReadOnlyTee)
            .unwrap();

        // Subscribe BEFORE apply so we don't miss the fire.
        let mut rx = op_events::subscribe();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid.to_bytes().into(),
            vec![],
            1,
            dummy_member_removed_op(tee_pk),
        )
        .expect("sign MemberRemoved");
        apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

        let (mr, tmr) = count_removed_events_for(&mut rx, gid.to_bytes(), tee_pk);
        assert_eq!(
            mr, 1,
            "MemberRemoved must still fire for a TEE removal (auto-follow + downstream rely on it)"
        );
        assert_eq!(
            tmr, 1,
            "TeeMemberRemoved MUST fire for a removal whose stored role was ReadOnlyTee"
        );
    }

    /// Removing a regular `Member` via `GroupOp::MemberRemoved` must
    /// emit ONLY `MemberRemoved` and never `TeeMemberRemoved` — this
    /// is what preserves the soft-leave path for the 4 e2e workflows
    /// `group-{kick-and-readd-deny-list, kick-and-rejoin-keyshare,
    /// leave-namespace, leave-then-rejoin-via-inheritance}` that
    /// closing #2653 was about.
    #[test]
    #[serial_test::serial]
    fn member_removed_op_does_not_emit_tee_event_for_regular_member() {
        let store = test_store();
        let gid = test_group_id();
        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin_pk = admin_sk.public_key();
        let target_pk = PublicKey::from([0xE2; 32]);

        let mut meta = test_meta();
        meta.admin_identity = admin_pk;
        meta.owner_identity = admin_pk;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &target_pk, GroupMemberRole::Member)
            .unwrap();

        let mut rx = op_events::subscribe();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid.to_bytes().into(),
            vec![],
            1,
            dummy_member_removed_op(target_pk),
        )
        .expect("sign MemberRemoved");
        apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

        let (mr, tmr) = count_removed_events_for(&mut rx, gid.to_bytes(), target_pk);
        assert_eq!(
            mr, 1,
            "MemberRemoved must fire for a regular-Member removal"
        );
        assert_eq!(
            tmr, 0,
            "TeeMemberRemoved MUST NOT fire for a non-TEE removal (soft-leave path preserved)"
        );
    }

    /// A namespace-root `MemberRemoved` of a `ReadOnlyTee` must cascade
    /// into descendant subgroups — INCLUDING a `Restricted` subgroup —
    /// removing the TEE's row there and emitting per-subgroup
    /// `MemberRemoved` + `TeeMemberRemoved`. This lets a namespace owner
    /// evict a TEE fleet node namespace-wide; sound because a TEE's
    /// subgroup membership derives from namespace-level attestation
    /// policy, not the subgroup admin's choice.
    #[test]
    #[serial_test::serial]
    fn member_removed_root_readonly_tee_cascades_into_restricted_subgroup() {
        use calimero_context_config::VisibilityMode;

        let store = test_store();

        // namespace (root) ── Restricted subgroup
        let ns_gid = ContextGroupId::from([0xD0; 32]);
        let subgroup = ContextGroupId::from([0xD1; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_gid, &subgroup)
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Restricted)
            .unwrap();

        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin_pk = admin_sk.public_key();
        let tee_pk = PublicKey::from([0xE1; 32]);

        MetaRepository::new(&store)
            .save(&ns_gid, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MetaRepository::new(&store)
            .save(&subgroup, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        // TEE has a direct row at BOTH the root and the Restricted subgroup.
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &tee_pk, GroupMemberRole::ReadOnlyTee)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&subgroup, &tee_pk, GroupMemberRole::ReadOnlyTee)
            .unwrap();

        // Subscribe BEFORE apply so we don't miss the fire.
        let mut rx = op_events::subscribe();

        // Root admin removes the TEE at the namespace root.
        let op = SignedGroupOp::sign(
            &admin_sk,
            ns_gid.to_bytes().into(),
            vec![],
            1,
            dummy_member_removed_op(tee_pk),
        )
        .expect("sign MemberRemoved");
        apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

        // Cascade removed the TEE's row in the Restricted subgroup …
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&subgroup, &tee_pk)
                .unwrap(),
            None,
            "root TEE removal MUST cascade into the Restricted subgroup row"
        );
        // … and the root row too.
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns_gid, &tee_pk)
                .unwrap(),
            None,
            "root row must be removed"
        );
        // Cascade deny-lists the TEE in the subgroup.
        assert!(
            DenyListRepository::new(&store)
                .is_denied(&subgroup, &tee_pk)
                .unwrap(),
            "cascade must deny-list the TEE in the subgroup"
        );

        let (root_pair, sub_pair) =
            count_removed_events_for_two(&mut rx, ns_gid.to_bytes(), subgroup.to_bytes(), tee_pk);
        assert_eq!(
            sub_pair,
            (1, 1),
            "subgroup must see one MemberRemoved + one TeeMemberRemoved from the cascade"
        );
        assert_eq!(
            root_pair,
            (1, 1),
            "root must see one MemberRemoved + one TeeMemberRemoved"
        );
    }

    /// A root TEE removal must also purge the TEE's `ContextIdentity` rows in
    /// an Open subgroup it only INHERITED into — i.e. where it auto-followed
    /// contexts (Fix B) without ever holding a direct `GroupMember` row.
    /// Those subgroups are excluded from the per-direct-row event loop, so the
    /// regression guarded here is a stranded-identity leak: the cascade must
    /// still run `cascade_remove_member` over every descendant, while the
    /// per-subgroup `MemberRemoved`/`TeeMemberRemoved` events stay gated to
    /// direct rows.
    #[test]
    #[serial_test::serial]
    fn member_removed_root_readonly_tee_purges_inherited_open_subgroup_identity() {
        use calimero_context_config::VisibilityMode;

        let store = test_store();

        // namespace (root) ── Open subgroup (TEE has NO direct row here)
        let ns_gid = ContextGroupId::from([0xD4; 32]);
        let subgroup = ContextGroupId::from([0xD5; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_gid, &subgroup)
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .unwrap();

        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin_pk = admin_sk.public_key();
        let tee_pk = PublicKey::from([0xE3; 32]);

        MetaRepository::new(&store)
            .save(&ns_gid, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MetaRepository::new(&store)
            .save(&subgroup, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        // TEE has a direct row ONLY at the root — in the Open subgroup its
        // membership is inherited, so there is no `GroupMember` row there.
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &tee_pk, GroupMemberRole::ReadOnlyTee)
            .unwrap();

        // Auto-follow gave the inherited TEE a `ContextIdentity` row under a
        // context registered in the Open subgroup, despite no direct row.
        let context = ContextId::from([0xC5; 32]);
        register_context_in_group(&store, &subgroup, &context).unwrap();
        let identity_key = calimero_store::key::ContextIdentity::new(context, tee_pk);
        {
            let mut handle = store.handle();
            handle
                .put(
                    &identity_key,
                    &calimero_store::types::ContextIdentity {
                        private_key: Some([7u8; 32]),
                        sender_key: None,
                    },
                )
                .unwrap();
        }
        assert!(
            store.handle().has(&identity_key).unwrap(),
            "test precondition: inherited ContextIdentity row exists"
        );

        let mut rx = op_events::subscribe();

        let op = SignedGroupOp::sign(
            &admin_sk,
            ns_gid.to_bytes().into(),
            vec![],
            1,
            dummy_member_removed_op(tee_pk),
        )
        .expect("sign MemberRemoved");
        apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

        // The stranded inherited identity row is purged …
        assert!(
            !store.handle().has(&identity_key).unwrap(),
            "root TEE removal MUST purge inherited Open-subgroup ContextIdentity rows"
        );

        // … but no per-subgroup membership event fires (no direct row), while
        // the root still emits its pair. `tee_pk`/`subgroup` are unique to this
        // test, so the shared-bus event counts are not contaminated.
        let (root_pair, sub_pair) =
            count_removed_events_for_two(&mut rx, ns_gid.to_bytes(), subgroup.to_bytes(), tee_pk);
        assert_eq!(
            sub_pair,
            (0, 0),
            "inherited subgroup (no direct row) must NOT emit cascade membership events"
        );
        assert_eq!(
            root_pair,
            (1, 1),
            "root must see one MemberRemoved + one TeeMemberRemoved"
        );
    }

    /// The mirror-image guard: a namespace-root `MemberRemoved` of a
    /// regular `Member` must NOT cascade. The root row is removed
    /// (today's behavior) but the `Restricted` subgroup row is
    /// PRESERVED — the #2256 Restricted-subgroup membership wall holds
    /// for non-TEE members.
    #[test]
    #[serial_test::serial]
    fn member_removed_root_regular_member_does_not_cascade() {
        use calimero_context_config::VisibilityMode;

        let store = test_store();

        // namespace (root) ── Restricted subgroup
        let ns_gid = ContextGroupId::from([0xD2; 32]);
        let subgroup = ContextGroupId::from([0xD3; 32]);
        NamespaceRepository::new(&store)
            .nest(&ns_gid, &subgroup)
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Restricted)
            .unwrap();

        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin_pk = admin_sk.public_key();
        let member_pk = PublicKey::from([0xE2; 32]);

        MetaRepository::new(&store)
            .save(&ns_gid, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MetaRepository::new(&store)
            .save(&subgroup, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        // Regular member has a direct row at BOTH root and subgroup.
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &member_pk, GroupMemberRole::Member)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&subgroup, &member_pk, GroupMemberRole::Member)
            .unwrap();

        let mut rx = op_events::subscribe();

        let op = SignedGroupOp::sign(
            &admin_sk,
            ns_gid.to_bytes().into(),
            vec![],
            1,
            dummy_member_removed_op(member_pk),
        )
        .expect("sign MemberRemoved");
        apply_local_signed_group_op(&store, &op).expect("apply MemberRemoved");

        // Root row removed (today's behavior) …
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&ns_gid, &member_pk)
                .unwrap(),
            None,
            "root row must be removed"
        );
        // … but the Restricted subgroup row is PRESERVED — no cascade.
        assert_eq!(
            MembershipRepository::new(&store)
                .role_of(&subgroup, &member_pk)
                .unwrap(),
            Some(GroupMemberRole::Member),
            "regular-member root removal MUST NOT cascade — Restricted wall holds (#2256)"
        );

        let (root_pair, sub_pair) = count_removed_events_for_two(
            &mut rx,
            ns_gid.to_bytes(),
            subgroup.to_bytes(),
            member_pk,
        );
        assert_eq!(
            sub_pair,
            (0, 0),
            "no cascade events for a non-TEE root removal"
        );
        assert_eq!(
            root_pair,
            (1, 0),
            "root sees only MemberRemoved (no TeeMemberRemoved) for a regular member"
        );
    }

    /// Same role-scoped contract on the `MemberLeft` (self-leave) arm:
    /// a `ReadOnlyTee` self-leave emits both events; an `Admin`/
    /// `Member` self-leave emits only `MemberRemoved`.
    #[test]
    #[serial_test::serial]
    fn member_left_op_emits_tee_event_only_for_readonly_tee_role() {
        // Case 1: TEE self-leave fires both.
        {
            let store = test_store();
            let gid = test_group_id();
            // Distinct admin so the leaver is not the last admin (the
            // apply path bails on `OwnerCannotSelfLeave` /
            // `LastAdminCannotLeave` otherwise).
            let admin_sk = PrivateKey::random(&mut OsRng);
            let admin_pk = admin_sk.public_key();
            let tee_sk = PrivateKey::random(&mut OsRng);
            let tee_pk = tee_sk.public_key();

            let mut meta = test_meta();
            meta.admin_identity = admin_pk;
            meta.owner_identity = admin_pk;
            MetaRepository::new(&store).save(&gid, &meta).unwrap();
            MembershipRepository::new(&store)
                .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
                .unwrap();
            MembershipRepository::new(&store)
                .add_member(&gid, &tee_pk, GroupMemberRole::ReadOnlyTee)
                .unwrap();

            let mut rx = op_events::subscribe();

            let op = SignedGroupOp::sign(
                &tee_sk,
                gid.to_bytes().into(),
                vec![],
                1,
                GroupOp::MemberLeft {
                    member: tee_pk,
                    expected_group_state_hash: [0u8; 32],
                    expected_context_state_hashes: Vec::new(),
                },
            )
            .expect("sign MemberLeft (TEE)");
            apply_local_signed_group_op(&store, &op).expect("apply MemberLeft (TEE)");

            let (mr, tmr) = count_removed_events_for(&mut rx, gid.to_bytes(), tee_pk);
            assert_eq!(
                mr, 1,
                "MemberRemoved must fire for a TEE self-leave (existing subscribers)"
            );
            assert_eq!(
                tmr, 1,
                "TeeMemberRemoved MUST fire for a TEE self-leave (purge hygiene)"
            );
        }
        // Case 2: regular-Member self-leave fires only MemberRemoved.
        {
            let store = test_store();
            let gid = test_group_id();
            let admin_sk = PrivateKey::random(&mut OsRng);
            let admin_pk = admin_sk.public_key();
            let leaver_sk = PrivateKey::random(&mut OsRng);
            let leaver_pk = leaver_sk.public_key();

            let mut meta = test_meta();
            meta.admin_identity = admin_pk;
            meta.owner_identity = admin_pk;
            MetaRepository::new(&store).save(&gid, &meta).unwrap();
            MembershipRepository::new(&store)
                .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
                .unwrap();
            MembershipRepository::new(&store)
                .add_member(&gid, &leaver_pk, GroupMemberRole::Member)
                .unwrap();

            let mut rx = op_events::subscribe();

            let op = SignedGroupOp::sign(
                &leaver_sk,
                gid.to_bytes().into(),
                vec![],
                1,
                GroupOp::MemberLeft {
                    member: leaver_pk,
                    expected_group_state_hash: [0u8; 32],
                    expected_context_state_hashes: Vec::new(),
                },
            )
            .expect("sign MemberLeft (regular)");
            apply_local_signed_group_op(&store, &op).expect("apply MemberLeft (regular)");

            let (mr, tmr) = count_removed_events_for(&mut rx, gid.to_bytes(), leaver_pk);
            assert_eq!(mr, 1, "MemberRemoved must fire for a regular self-leave");
            assert_eq!(
                tmr, 0,
                "TeeMemberRemoved MUST NOT fire for a regular self-leave (soft-leave path preserved)"
            );
        }
    }
}

#[test]
fn placeholder_admin_identity_never_equals_a_real_key() {
    // #599: documents/pins WHY the all-zeros sentinel
    // (`PLACEHOLDER_ADMIN_IDENTITY`) is safe to use in place of an
    // `Option<PublicKey>`: no legitimate Ed25519 signing identity can ever
    // serialize to all-zeros, so the genesis established-check can never
    // confuse a placeholder for a real founder (or vice-versa).
    //
    // An Ed25519 public key is `A = a·B`, a point in the prime-order subgroup
    // generated by the basepoint `B`. The all-zeros encoding decodes to the
    // curve's identity / low-order (torsion) point, which lies OUTSIDE that
    // prime-order subgroup, so no public key derived from a secret scalar can
    // equal it. We assert that across many freshly generated keypairs the
    // public key is NEVER the sentinel.
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let sentinel = placeholder_admin_identity();
    assert_eq!(
        *sentinel.digest(),
        PLACEHOLDER_ADMIN_IDENTITY,
        "the PublicKey form of the sentinel must round-trip to the all-zeros bytes"
    );

    let mut rng = OsRng;
    for _ in 0..256 {
        let pk = PrivateKey::random(&mut rng).public_key();
        assert_ne!(
            pk, sentinel,
            "a freshly generated keypair's public key must never equal the all-zeros sentinel"
        );
        assert_ne!(
            *pk.digest(),
            PLACEHOLDER_ADMIN_IDENTITY,
            "a freshly generated keypair's raw bytes must never be all-zeros"
        );
    }

    // The sentinel is not a valid signing key: it cannot verify a signature
    // produced by any real key, and (being the identity/torsion point) is not a
    // usable verifying key. Asserting it rejects a real signature confirms it
    // can never stand in for a legitimate signing identity.
    let signer = PrivateKey::random(&mut rng);
    let sig = signer.sign(b"genesis").expect("sign test message");
    assert!(
        sentinel.verify(b"genesis", &sig).is_err(),
        "the all-zeros sentinel must not verify a signature from a real key — \
         it is not a legitimate signing identity"
    );
}

// -----------------------------------------------------------------------
// Cascade authority determinism / cross-replica convergence
// -----------------------------------------------------------------------

/// CONVERGENCE PIN: two logical replicas with DIFFERENT fold progress on a
/// matched descendant's capabilities MUST reach the SAME apply/bail outcome
/// for the same signed cascade op.
///
/// The only difference between the two stores is descendant `D`'s
/// `MANAGE_APPLICATION` capability for the signer — modelling a concurrent
/// `MemberCapabilitySet` cap-revoke on `D` that one replica has folded and the
/// other has not. Under the old per-descendant LIVE pre-scan, the replica that
/// folded the revoke BAILED the whole op while the other APPLIED it, permanently
/// diverging `target_application_id`/`app_key`. The fix authorizes the cascade
/// once against the root admin, so the descendant's live caps no longer flip the
/// outcome. Runs on the STANDALONE group-DAG path (`apply_local_signed_group_op`,
/// LIVE fallback), so it proves convergence without relying on the at-cut
/// authorizer.
#[test]
fn cascade_authority_is_root_only_and_converges_despite_descendant_cap_skew() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use calimero_storage::logical_clock::HybridTimestamp;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    let root = ContextGroupId::from([0x70; 32]);
    let descendant = ContextGroupId::from([0xD1; 32]);
    let from_app_key = [0x11; 32];
    let to_app_key = [0x22; 32];
    let app_v1 = ApplicationId::from([0xC1; 32]);
    let app_v2 = ApplicationId::from([0xC2; 32]);
    // A different admin for `D`, so the signer is NOT admin-of-D via meta.
    let other_admin = PublicKey::from([0x09; 32]);

    // Build a store where `signer_has_cap_on_descendant` is the ONLY knob:
    // whether the signer holds MANAGE_APPLICATION on the (Restricted) descendant.
    let build = |signer_has_cap_on_descendant: bool| {
        let store = test_store();

        // Root: signer is a direct admin, on `from_app_key`.
        let mut root_meta = sample_meta_with_admin(admin_pk);
        root_meta.app_key = from_app_key;
        root_meta.target_application_id = app_v1;
        MetaRepository::new(&store).save(&root, &root_meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&root, &admin_pk, GroupMemberRole::Admin)
            .unwrap();

        // Descendant: matched (same `from_app_key`) but admin'd by someone else.
        // Left at the DEFAULT Restricted visibility, so the signer does NOT
        // inherit admin authority across the boundary.
        let mut d_meta = sample_meta_with_admin(other_admin);
        d_meta.app_key = from_app_key;
        d_meta.target_application_id = app_v1;
        MetaRepository::new(&store)
            .save(&descendant, &d_meta)
            .unwrap();
        nest_for_test(&store, &root, &descendant);

        if signer_has_cap_on_descendant {
            // Replica that has NOT folded the cap-revoke: signer still holds
            // MANAGE_APPLICATION on the descendant (old pre-scan passes).
            CapabilitiesRepository::new(&store)
                .set_member_capability(
                    &descendant,
                    &admin_pk,
                    calimero_context_config::MemberCapabilities::MANAGE_APPLICATION.bits(),
                )
                .unwrap();
        }
        // else: replica that HAS folded the cap-revoke — no cap on the
        // descendant (old pre-scan bails the whole op).

        store
    };

    let sign_cascade = || {
        SignedGroupOp::sign(
            &admin_sk,
            root.to_bytes().into(),
            vec![],
            1,
            GroupOp::CascadeUpgrade {
                from_app_key: from_app_key.into(),
                app_key: to_app_key.into(),
                target_application_id: app_v2,
                migration: None,
                cascade_hlc: HybridTimestamp::zero(),
            },
        )
        .expect("sign CascadeUpgrade")
    };

    let store_behind = build(true); // cap-revoke NOT yet folded
    let store_synced = build(false); // cap-revoke folded

    let res_behind = apply_local_signed_group_op(&store_behind, &sign_cascade());
    let res_synced = apply_local_signed_group_op(&store_synced, &sign_cascade());

    // Convergence: both replicas MUST reach the same apply/bail outcome. On the
    // old code, `res_behind` is Ok and `res_synced` is Err -> divergence.
    assert_eq!(
        res_behind.is_ok(),
        res_synced.is_ok(),
        "cascade apply/bail outcome diverged across replicas with different \
         descendant-cap fold progress: behind={:?} synced={:?}",
        res_behind.as_ref().map(|_| ()),
        res_synced.as_ref().map(|_| ()),
    );
    res_behind.expect("cascade authorized by root admin must apply (behind replica)");
    res_synced.expect("cascade authorized by root admin must apply (synced replica)");

    // And both must have actually mutated the matched descendant identically.
    for (label, store) in [("behind", &store_behind), ("synced", &store_synced)] {
        let d = MetaRepository::new(store)
            .load(&descendant)
            .unwrap()
            .expect("descendant meta");
        assert_eq!(
            d.app_key, to_app_key,
            "descendant must be cascaded to the new app_key on the {label} replica"
        );
        assert_eq!(
            d.target_application_id, app_v2,
            "descendant must point at the new target on the {label} replica"
        );
        // The sticky cascade fence must be stamped identically on both
        // replicas — it is the boundary the state-delta HLC fence reads, and a
        // dropped `repo.save` would silently regress it while the meta asserts
        // above still pass.
        let up = UpgradesRepository::new(store)
            .load(&descendant)
            .unwrap()
            .expect("descendant upgrade record");
        assert_eq!(
            up.cascade_hlc,
            Some(HybridTimestamp::zero()),
            "descendant must carry the signed cascade_hlc fence on the {label} replica"
        );
    }
}

// -----------------------------------------------------------------------
// Apply-time authority must resolve at the op's causal cut, not against the
// receiver's live rows.
//
// The settings ops (`TargetApplicationSet`, `GroupMigrationSet`,
// `UpgradePolicySet`, `DefaultCapabilitiesSet`, `SubgroupVisibilitySet`) run
// their gates through `GroupSettingsService`, which used to build a LIVE
// `PermissionChecker` regardless of the apply context. That made the verdict a
// function of each replica's fold progress: a replica that had folded a
// concurrent capability revoke rejected the op (and, because the reject path
// never advances the DAG head, stalled every descendant of it) while a replica
// that had not folded the revoke applied it. Permanent divergence.
//
// These tests pin the fix by making the at-cut source DISAGREE with the live
// rows in both directions. If the at-cut verdict is the one honored, the live
// rows are irrelevant — which is exactly the property that makes the decision
// replica-independent.
mod apply_auth_at_cut {
    use super::*;
    use crate::test_fixtures::{FixedAuthorizer, TEST_CUT as CUT};
    use calimero_context_config::types::AppKey;
    use calimero_context_config::MemberCapabilities;
    use calimero_governance_types::GroupOp;

    /// Group whose live rows grant `signer` full admin authority.
    fn store_with_live_admin(signer: &PublicKey) -> (Store, ContextGroupId) {
        let store = test_store();
        let gid = test_group_id();
        let mut meta = test_meta();
        meta.admin_identity = *signer;
        meta.owner_identity = *signer;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, signer, GroupMemberRole::Admin)
            .unwrap();
        (store, gid)
    }

    /// Group whose live rows grant `signer` NOTHING — not even a membership row.
    fn store_with_live_stranger(signer: &PublicKey) -> (Store, ContextGroupId) {
        let store = test_store();
        let gid = test_group_id();
        let other = PublicKey::from([0x77; 32]);
        let mut meta = test_meta();
        meta.admin_identity = other;
        meta.owner_identity = other;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &other, GroupMemberRole::Admin)
            .unwrap();
        let _ = signer;
        (store, gid)
    }

    fn target_application_set_op() -> GroupOp {
        GroupOp::TargetApplicationSet {
            app_key: AppKey::from([0x5A; 32]),
            target_application_id: ApplicationId::from([0x5B; 32]),
        }
    }

    #[test]
    fn target_application_set_honors_at_cut_grant_over_live_denial() {
        // The catching-up replica: its live rows say the signer has no authority
        // (it has not yet folded the grant), but the projection at the op's cut
        // says the signer WAS authorized as of the op's own parents. The op must
        // apply — otherwise this replica rejects an op its peers accept.
        let signer = PublicKey::from([0x11; 32]);
        let (store, gid) = store_with_live_stranger(&signer);

        let (handled, _divergence, _events) = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &CUT,
            &FixedAuthorizer(true),
        )
        .expect("at-cut grant must authorize the settings op despite live rows denying");
        assert!(handled, "TargetApplicationSet should be handled");

        let meta = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
        assert_eq!(
            meta.app_key, [0x5A; 32],
            "the mutation must actually land, not just be reported handled"
        );
    }

    #[test]
    fn target_application_set_honors_at_cut_denial_over_live_grant() {
        // The mirror: live rows would grant (this replica has not folded the
        // revoke), but at the op's cut the signer was NOT authorized. The op must
        // be rejected — otherwise this replica applies an op its peers reject.
        let signer = PublicKey::from([0x11; 32]);
        let (store, gid) = store_with_live_admin(&signer);
        let before = MetaRepository::new(&store).load(&gid).unwrap().unwrap();

        let err = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &CUT,
            &FixedAuthorizer(false),
        )
        .expect_err("at-cut denial must reject the settings op despite live rows granting");
        assert!(
            format!("{err:#}").contains("lacks permission"),
            "expected an authorization failure, got: {err:#}"
        );

        let after = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
        assert_eq!(
            before.app_key, after.app_key,
            "a rejected settings op must not mutate group meta"
        );
    }

    #[test]
    fn default_capabilities_set_honors_at_cut_verdict() {
        // `DefaultCapabilitiesSet` gates on `require_admin`, and is itself a
        // capability op — gating a capability op on LIVE capabilities is the
        // tightest version of the divergence loop, so pin both directions.
        let signer = PublicKey::from([0x11; 32]);

        let (store, gid) = store_with_live_stranger(&signer);
        let op = GroupOp::DefaultCapabilitiesSet {
            capabilities: MemberCapabilities::MANAGE_MEMBERS,
        };
        let (handled, _, _) =
            apply_group_op_mutations(&store, &gid, &signer, &op, &CUT, &FixedAuthorizer(true))
                .expect("at-cut admin grant must authorize DefaultCapabilitiesSet");
        assert!(handled);
        assert_eq!(
            CapabilitiesRepository::new(&store)
                .default_capabilities(&gid)
                .unwrap(),
            Some(MemberCapabilities::MANAGE_MEMBERS.bits()),
            "the default-capability mutation must land"
        );

        let (store, gid) = store_with_live_admin(&signer);
        let op = GroupOp::DefaultCapabilitiesSet {
            capabilities: MemberCapabilities::MANAGE_MEMBERS,
        };
        let _ = apply_group_op_mutations(&store, &gid, &signer, &op, &CUT, &FixedAuthorizer(false))
            .expect_err(
                "at-cut denial must reject DefaultCapabilitiesSet despite a live admin row",
            );
    }

    #[test]
    fn two_replicas_with_opposite_live_rows_agree_at_the_same_cut() {
        // The divergence scenario end to end, as two replicas of the SAME op.
        //
        // Replica A folded a concurrent capability revoke; replica B has not.
        // Their live rows therefore disagree. Both resolve the op at its own cut,
        // so both MUST reach the same verdict and the same resulting state — that
        // agreement is the whole property, and it is what live-resolution broke.
        let signer = PublicKey::from([0x11; 32]);
        let (store_a, gid_a) = store_with_live_stranger(&signer); // revoke folded
        let (store_b, gid_b) = store_with_live_admin(&signer); // revoke not folded

        for (store, gid, label) in [
            (&store_a, &gid_a, "revoke-folded replica"),
            (&store_b, &gid_b, "revoke-unfolded replica"),
        ] {
            let (handled, _, _) = apply_group_op_mutations(
                store,
                gid,
                &signer,
                &target_application_set_op(),
                &CUT,
                &FixedAuthorizer(true),
            )
            .unwrap_or_else(|e| panic!("{label} must apply the op at the cut, got: {e:#}"));
            assert!(handled, "{label}: op should be handled");

            let meta = MetaRepository::new(store).load(gid).unwrap().unwrap();
            assert_eq!(
                meta.app_key, [0x5A; 32],
                "{label}: both replicas must converge on the same app_key"
            );
        }
    }
}

// -----------------------------------------------------------------------
// A removal must not mint a group key that peers will reject.
//
// The publish gate for removal is admin OR `MANAGE_MEMBERS`; the rotation's
// receive gate is strict admin, checked against the namespace identity that signs
// the outer op. A non-admin `MANAGE_MEMBERS` holder therefore used to: mint a new
// key, store it locally at the TOP epoch (making it this node's "current" key),
// attach a rotation every peer silently rejected, and thereafter encrypt every
// group op under a key no other node held — peers buffering them as undecryptable
// forever. A group-wide liveness break, not a skipped rotation.
//
// Fail closed instead: refuse the removal. Groups encrypted under the NAMESPACE
// key never rotate on removal, so a `MANAGE_MEMBERS` holder may still remove there.
mod rotation_gate_alignment {
    use super::*;
    use crate::group_governance_publisher::ensure_rotation_is_publishable;
    use crate::test_fixtures::bootstrap_namespace_with_admin;
    use calimero_context_config::VisibilityMode;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    /// Namespace + a subgroup nested under it. The subgroup's visibility is left at
    /// the default (Restricted), so it encrypts under its OWN key and a removal
    /// there rotates.
    fn namespace_with_subgroup() -> (Store, ContextGroupId, ContextGroupId, PublicKey) {
        let store = test_store();
        let ns_id = [0xF1u8; 32];
        let ns_gid = ContextGroupId::from(ns_id);
        let (_admin_sk, admin_pk) = bootstrap_namespace_with_admin(&store, ns_id);

        let sub_gid = ContextGroupId::from([0xF2u8; 32]);
        MetaRepository::new(&store)
            .save(&sub_gid, &sample_meta_with_admin(admin_pk))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&sub_gid, &admin_pk, GroupMemberRole::Admin)
            .unwrap();
        nest_for_test(&store, &ns_gid, &sub_gid);

        (store, ns_gid, sub_gid, admin_pk)
    }

    /// Repoint this node's namespace identity at a fresh keypair that holds no
    /// admin row anywhere — the non-admin `MANAGE_MEMBERS` holder's node.
    fn make_namespace_identity_a_non_admin(store: &Store, ns_gid: &ContextGroupId) -> PublicKey {
        let sk_bytes: [u8; 32] = rand::Rng::gen(&mut OsRng);
        let sk = PrivateKey::from(sk_bytes);
        let pk = sk.public_key();
        NamespaceRepository::new(store)
            .store_identity(ns_gid, &pk, &sk_bytes, &[0u8; 32])
            .unwrap();
        pk
    }

    #[test]
    fn restricted_group_removal_by_non_admin_is_refused() {
        let (store, ns_gid, sub_gid, _admin) = namespace_with_subgroup();
        let non_admin = make_namespace_identity_a_non_admin(&store, &ns_gid);

        assert!(
            !PermissionChecker::new(&store, sub_gid)
                .is_admin(&non_admin)
                .unwrap(),
            "precondition: this node's namespace identity must not be an admin of the subgroup"
        );

        let err = ensure_rotation_is_publishable(&store, sub_gid).expect_err(
            "a removal that must rotate, from a node whose rotation peers would reject, \
             must be refused rather than split the keyring",
        );
        assert!(
            format!("{err:#}").contains("splitting the keyring"),
            "the error should explain the keyring split it is preventing, got: {err:#}"
        );
    }

    #[test]
    fn restricted_group_removal_by_admin_is_allowed() {
        // The mirror: the node's namespace identity IS an admin of the subgroup, so
        // peers will accept its rotation. The removal must go through.
        let (store, _ns_gid, sub_gid, admin) = namespace_with_subgroup();

        assert!(
            PermissionChecker::new(&store, sub_gid)
                .is_admin(&admin)
                .unwrap(),
            "precondition: the bootstrapped namespace identity is the subgroup admin"
        );

        ensure_rotation_is_publishable(&store, sub_gid)
            .expect("an admin's removal rotates cleanly and must be allowed");
    }

    #[test]
    fn open_chain_group_removal_by_non_admin_is_allowed() {
        // An Open subgroup under an Open chain encrypts with the NAMESPACE key, so a
        // removal mints no per-subgroup key and there is no rotation to reject. The
        // MANAGE_MEMBERS holder keeps the ability to remove here — narrowing the gate
        // must not over-reach into groups that never rotate.
        let (store, ns_gid, sub_gid, _admin) = namespace_with_subgroup();
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&sub_gid, VisibilityMode::Open)
            .unwrap();
        let _non_admin = make_namespace_identity_a_non_admin(&store, &ns_gid);

        assert!(
            CapabilitiesRepository::new(&store)
                .is_open_chain_to_namespace(&sub_gid, &ns_gid)
                .unwrap(),
            "precondition: the subgroup must sit on a fully-Open chain to the namespace"
        );

        ensure_rotation_is_publishable(&store, sub_gid).expect(
            "an Open-chain group never rotates on removal, so a non-admin removal must \
             still be permitted",
        );
    }
}

// -----------------------------------------------------------------------
// An unresolvable cut must PARK the op, not guess from live.
//
// The projection abstains for two very different reasons, and the apply gates used
// to collapse them into one "fall back to live" branch:
//
//   * no cut to resolve against (a genesis op, or no apply-auth context at all) —
//     live is correct, nothing contradicts it;
//   * the cut is real but this node has not folded its ancestry — live is a
//     DIFFERENT cut (this replica's current one).
//
// In the second case the verdict became a function of fold progress. A replica that
// had folded a concurrent capability revoke rejected an op its peers applied; and
// since the reject path never advances the DAG head, every op descending from it
// stalled on that replica alone. Permanent, silent divergence.
//
// Now the gate refuses to answer: `AuthorityUndecidable`, which the DAG treats like
// any other apply error (head not advanced, nonce not burned), so the op is retried
// once the missing history arrives. A loud stall beats a quiet divergence.
mod undecidable_authority_parks {
    use super::*;
    use crate::test_fixtures::{FixedAuthorizer, UnresolvableAuthorizer, TEST_CUT as CUT};
    use calimero_context_config::types::AppKey;
    use calimero_governance_types::GroupOp;

    fn target_application_set_op() -> GroupOp {
        GroupOp::TargetApplicationSet {
            app_key: AppKey::from([0x5A; 32]),
            target_application_id: ApplicationId::from([0x5B; 32]),
        }
    }

    fn group_with_live_admin(signer: &PublicKey) -> (Store, ContextGroupId) {
        let store = test_store();
        let gid = test_group_id();
        let mut meta = test_meta();
        meta.admin_identity = *signer;
        meta.owner_identity = *signer;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, signer, GroupMemberRole::Admin)
            .unwrap();
        (store, gid)
    }

    fn group_with_live_stranger() -> (Store, ContextGroupId) {
        let store = test_store();
        let gid = test_group_id();
        let other = PublicKey::from([0x77; 32]);
        let mut meta = test_meta();
        meta.admin_identity = other;
        meta.owner_identity = other;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &other, GroupMemberRole::Admin)
            .unwrap();
        (store, gid)
    }

    fn assert_undecidable(err: &eyre::Report) {
        assert!(
            format!("{err:#}").contains("authority undecidable"),
            "expected AuthorityUndecidable (a retryable park), got: {err:#}"
        );
    }

    #[test]
    fn unresolvable_cut_refuses_rather_than_granting_from_live() {
        // Live rows would GRANT. Guessing from them would apply an op the peers
        // (resolving at the real cut) might reject.
        let signer = PublicKey::from([0x11; 32]);
        let (store, gid) = group_with_live_admin(&signer);

        let err = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &CUT,
            &UnresolvableAuthorizer,
        )
        .expect_err("an unresolvable cut must not be answered from the live rows");
        assert_undecidable(&err);

        let meta = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
        assert_ne!(
            meta.app_key, [0x5A; 32],
            "a parked op must not mutate state — it has not been authorized yet"
        );
    }

    #[test]
    fn unresolvable_cut_refuses_rather_than_denying_from_live() {
        // The load-bearing direction. Live rows would DENY, and the old code turned
        // that into a hard rejection — permanent on this replica, because its live
        // state (with the concurrent revoke folded) never changes back. Peers that
        // had not folded the revoke applied the op. That is the reported divergence.
        //
        // The rejection must instead be an UNDECIDABLE, which is retryable.
        let signer = PublicKey::from([0x11; 32]);
        let (store, gid) = group_with_live_stranger();

        let err = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &CUT,
            &UnresolvableAuthorizer,
        )
        .expect_err("an unresolvable cut must not harden into a permanent rejection");
        assert_undecidable(&err);
    }

    #[test]
    fn both_replicas_park_instead_of_disagreeing() {
        // The divergence, replayed as two replicas of the SAME op with OPPOSITE live
        // rows and neither able to resolve the cut.
        //
        // Before: replica A (revoke folded) rejected forever, replica B (revoke not
        // folded) applied. After: both park. They still agree — which is the only
        // property that matters — and both proceed once the ancestry arrives.
        let signer = PublicKey::from([0x11; 32]);
        let (store_a, gid_a) = group_with_live_stranger(); // would have REJECTED
        let (store_b, gid_b) = group_with_live_admin(&signer); // would have APPLIED

        for (store, gid, label) in [
            (&store_a, &gid_a, "revoke-folded replica"),
            (&store_b, &gid_b, "revoke-unfolded replica"),
        ] {
            let err = match apply_group_op_mutations(
                store,
                gid,
                &signer,
                &target_application_set_op(),
                &CUT,
                &UnresolvableAuthorizer,
            ) {
                Ok(_) => panic!("{label} must park, not reach a verdict of its own"),
                Err(e) => e,
            };
            assert_undecidable(&err);

            let meta = MetaRepository::new(store).load(gid).unwrap().unwrap();
            assert_ne!(
                meta.app_key, [0x5A; 32],
                "{label}: a parked op must leave state untouched"
            );
        }
    }

    #[test]
    fn resolvable_cut_still_decides_and_a_genesis_op_still_uses_live() {
        // The guard must not swallow the cases where abstention is legitimate.
        //
        // 1. A resolvable cut decides normally (no spurious parking).
        let signer = PublicKey::from([0x11; 32]);
        let (store, gid) = group_with_live_stranger();
        let (handled, _, _) = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &CUT,
            &FixedAuthorizer(true),
        )
        .expect("a resolvable cut must decide, not park");
        assert!(handled);

        // 2. No apply-auth context (empty cut + live-fallback authorizer) — the emit
        //    path, the local apply, tests. Abstention here means "no cut", so live
        //    decides and a live admin is authorized. This must NOT become undecidable.
        let (store, gid) = group_with_live_admin(&signer);
        let (handled, _, _) = apply_group_op_mutations(
            &store,
            &gid,
            &signer,
            &target_application_set_op(),
            &[],
            &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
        )
        .expect("a construction with no apply-auth context must still resolve live");
        assert!(handled);
    }
}

// -----------------------------------------------------------------------
// Forward secrecy on self-leave.
//
// A rotation is minted by whoever PUBLISHES the op that triggers it. For an
// admin-initiated removal that works — the publisher stays in the group. For a
// self-leave it cannot: the publisher IS the leaver, who would have to mint the very
// key they are being cut off from (and would keep it), and peers reject a rotation
// from a non-admin anyway. Before this, `MemberLeft` simply did no rotation at all:
// the leaver kept the namespace key and every Restricted subgroup key, indefinitely.
//
// So the leave records the debt and a remaining admin pays it. These tests pin the
// recording half (which groups owe a rotation, and which correctly owe nothing) and
// the discharge half (`GroupKeyRotated` clears the row, only for an admin, and
// idempotently — which is what makes a two-admin race harmless).
mod self_leave_rotation {
    use super::*;
    use crate::pending_rotation::group_rotates_on_departure;
    use crate::test_fixtures::bootstrap_namespace_with_admin;
    use crate::PendingRotationRepository;
    use calimero_context_config::VisibilityMode;
    use calimero_governance_types::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    /// A namespace root plus a subgroup, with `admin` in charge of both and `leaver` a
    /// plain member of both. The subgroup's visibility is left at the default
    /// (Restricted), so it encrypts under its own key.
    struct Fixture {
        store: Store,
        ns_gid: ContextGroupId,
        sub_gid: ContextGroupId,
        admin: PublicKey,
        leaver_sk: PrivateKey,
        leaver: PublicKey,
    }

    fn fixture() -> Fixture {
        let store = test_store();
        let ns_id = [0xA1u8; 32];
        let ns_gid = ContextGroupId::from(ns_id);
        let (_admin_sk, admin) = bootstrap_namespace_with_admin(&store, ns_id);

        let leaver_sk = PrivateKey::random(&mut OsRng);
        let leaver = leaver_sk.public_key();

        let sub_gid = ContextGroupId::from([0xA2u8; 32]);
        MetaRepository::new(&store)
            .save(&sub_gid, &sample_meta_with_admin(admin))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&sub_gid, &admin, GroupMemberRole::Admin)
            .unwrap();
        nest_for_test(&store, &ns_gid, &sub_gid);

        // The leaver is a direct member of both the root and the subgroup.
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &leaver, GroupMemberRole::Member)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&sub_gid, &leaver, GroupMemberRole::Member)
            .unwrap();

        Fixture {
            store,
            ns_gid,
            sub_gid,
            admin,
            leaver_sk,
            leaver,
        }
    }

    fn apply_member_left(f: &Fixture, group: &ContextGroupId) {
        let op = SignedGroupOp::sign(
            &f.leaver_sk,
            group.to_bytes().into(),
            vec![],
            1,
            GroupOp::MemberLeft {
                member: f.leaver,
                expected_group_state_hash: [0u8; 32],
                expected_context_state_hashes: Vec::new(),
            },
        )
        .expect("sign MemberLeft");
        apply_local_signed_group_op(&f.store, &op).expect("apply MemberLeft");
    }

    #[test]
    fn which_groups_owe_a_rotation_on_departure() {
        let f = fixture();

        // A Restricted subgroup holds its OWN key, which the leaver has. Rotate.
        assert!(
            group_rotates_on_departure(&f.store, &f.sub_gid).unwrap(),
            "a Restricted subgroup encrypts under its own key — a departure must rotate it"
        );

        // The namespace ROOT holds the namespace key, which also decrypts every Open
        // subgroup beneath it. A member who leaves the namespace keeps that key unless
        // it is rotated — this is the hole that makes a namespace-leave meaningless
        // without rotation.
        assert!(
            group_rotates_on_departure(&f.store, &f.ns_gid).unwrap(),
            "the namespace root holds the namespace key — leaving it must rotate that key"
        );

        // An Open subgroup on a fully-Open chain is encrypted with the NAMESPACE key,
        // which the leaver still holds by virtue of namespace membership. Minting a
        // per-subgroup key would revoke nothing — it would just go unused.
        CapabilitiesRepository::new(&f.store)
            .set_subgroup_visibility(&f.sub_gid, VisibilityMode::Open)
            .unwrap();
        assert!(
            !group_rotates_on_departure(&f.store, &f.sub_gid).unwrap(),
            "an Open subgroup is encrypted with the namespace key — a per-subgroup rotation \
             there would revoke nothing, so none is owed"
        );
    }

    #[test]
    fn leaving_a_restricted_subgroup_records_the_rotation_debt() {
        let f = fixture();
        let pending = PendingRotationRepository::new(&f.store);

        assert!(
            !pending.is_pending(&f.sub_gid, &f.leaver).unwrap(),
            "precondition: nothing owed before the leave"
        );

        apply_member_left(&f, &f.sub_gid);

        assert!(
            pending.is_pending(&f.sub_gid, &f.leaver).unwrap(),
            "a self-leave from a Restricted subgroup must record the forward-secrecy debt — \
             without it, nobody ever rotates and the leaver keeps the subgroup key"
        );
        // The row is written by the deterministic apply, so every node has it. That is
        // what lets any remaining admin pick the work up with no coordination.
        assert_eq!(
            pending.departed_for_group(&f.sub_gid).unwrap(),
            vec![f.leaver],
            "the group's worklist must name exactly the departed member"
        );
    }

    #[test]
    fn leaving_an_open_subgroup_records_nothing() {
        let f = fixture();
        CapabilitiesRepository::new(&f.store)
            .set_subgroup_visibility(&f.sub_gid, VisibilityMode::Open)
            .unwrap();

        apply_member_left(&f, &f.sub_gid);

        assert!(
            !PendingRotationRepository::new(&f.store)
                .is_pending(&f.sub_gid, &f.leaver)
                .unwrap(),
            "an Open subgroup is encrypted with the namespace key, so a departure owes no \
             per-subgroup rotation — recording one would queue work that revokes nothing"
        );
    }

    #[test]
    fn leaving_the_namespace_records_debt_for_the_root_and_every_restricted_descendant() {
        // The namespace-leave cascade. The leaver held a row in the root AND in a
        // Restricted subgroup, and holds a key for each. Both must be rotated, or they
        // go on reading whichever one was missed.
        let f = fixture();
        let pending = PendingRotationRepository::new(&f.store);

        apply_member_left(&f, &f.ns_gid);

        assert!(
            pending.is_pending(&f.ns_gid, &f.leaver).unwrap(),
            "leaving the namespace must rotate the NAMESPACE key — otherwise the leaver keeps \
             reading the root and every Open subgroup under it"
        );
        assert!(
            pending.is_pending(&f.sub_gid, &f.leaver).unwrap(),
            "the cascade must also rotate every Restricted descendant the leaver had a row in \
             — each has its own key, which the leaver holds"
        );

        let mut backlog = pending.all_pending().unwrap();
        backlog.sort_by_key(|(g, _)| g.to_bytes());
        let mut expected = vec![(f.ns_gid, f.leaver), (f.sub_gid, f.leaver)];
        expected.sort_by_key(|(g, _)| g.to_bytes());
        assert_eq!(
            backlog, expected,
            "the whole backlog is what a restarting admin drains — it must list every group \
             that still owes a rotation, and nothing else"
        );
    }

    #[test]
    fn group_key_rotated_discharges_the_debt_and_is_idempotent() {
        let f = fixture();
        let pending = PendingRotationRepository::new(&f.store);
        apply_member_left(&f, &f.sub_gid);
        assert!(pending.is_pending(&f.sub_gid, &f.leaver).unwrap());

        // The admin's rotation carries the new key; applying it discharges the debt.
        let admin_sk = NamespaceRepository::new(&f.store)
            .identity_record(&f.ns_gid)
            .unwrap()
            .map(|i| PrivateKey::from(i.private_key))
            .expect("namespace identity");
        assert_eq!(admin_sk.public_key(), f.admin);

        let rotate = |nonce: u64| {
            SignedGroupOp::sign(
                &admin_sk,
                f.sub_gid.to_bytes().into(),
                vec![],
                nonce,
                GroupOp::GroupKeyRotated { departed: f.leaver },
            )
            .expect("sign GroupKeyRotated")
        };

        apply_local_signed_group_op(&f.store, &rotate(1)).expect("admin's rotation must apply");
        assert!(
            !pending.is_pending(&f.sub_gid, &f.leaver).unwrap(),
            "GroupKeyRotated must discharge the pending row"
        );

        // Two admins racing both publish a rotation. The second to land finds nothing
        // left to clear and must simply no-op — if it errored, a perfectly ordinary
        // race would wedge the group's governance DAG on every replica.
        apply_local_signed_group_op(&f.store, &rotate(2))
            .expect("a second, redundant rotation must be a harmless no-op, not an error");
        assert!(!pending.is_pending(&f.sub_gid, &f.leaver).unwrap());
    }

    #[test]
    fn a_non_admin_cannot_discharge_the_debt() {
        // The pending row says "this group still owes a rotation". Clearing it is a
        // claim that the rotation happened. Peers accept a rotation only from an admin,
        // so if a non-admin could clear the row, the group would believe it had rotated
        // while every peer threw the key away — the debt would vanish unpaid.
        let f = fixture();
        let pending = PendingRotationRepository::new(&f.store);
        apply_member_left(&f, &f.sub_gid);

        let outsider_sk = PrivateKey::random(&mut OsRng);
        MembershipRepository::new(&f.store)
            .add_member(
                &f.sub_gid,
                &outsider_sk.public_key(),
                GroupMemberRole::Member,
            )
            .unwrap();

        let op = SignedGroupOp::sign(
            &outsider_sk,
            f.sub_gid.to_bytes().into(),
            vec![],
            1,
            GroupOp::GroupKeyRotated { departed: f.leaver },
        )
        .expect("sign GroupKeyRotated");

        let _ = apply_local_signed_group_op(&f.store, &op)
            .expect_err("a non-admin's GroupKeyRotated must be rejected");
        assert!(
            pending.is_pending(&f.sub_gid, &f.leaver).unwrap(),
            "the debt must survive a rejected rotation — otherwise it is silently written off \
             and no admin ever pays it"
        );
    }
}

// -----------------------------------------------------------------------
// The crypto half of self-leave forward secrecy.
//
// Recording the debt and publishing a `GroupKeyRotated` is bookkeeping. What actually
// revokes the leaver's read access is that the rotation's envelopes are wrapped for
// everyone who remains and for NOBODY who left. These tests assert that directly, and
// assert the convergence property that lets several admins rotate concurrently without
// coordination.
mod self_leave_rotation_crypto {
    use super::*;
    use crate::test_fixtures::bootstrap_namespace_with_admin;
    use crate::GroupKeyring;
    use calimero_governance_types::SignedGroupOp;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;
    use rand::Rng;

    #[test]
    fn the_rotation_wraps_the_new_key_for_everyone_except_the_leaver() {
        let store = test_store();
        let ns_id = [0xB1u8; 32];
        let ns_gid = ContextGroupId::from(ns_id);
        let (admin_sk, admin) = bootstrap_namespace_with_admin(&store, ns_id);

        let stayer_sk = PrivateKey::random(&mut OsRng);
        let stayer = stayer_sk.public_key();
        let leaver_sk = PrivateKey::random(&mut OsRng);
        let leaver = leaver_sk.public_key();

        MembershipRepository::new(&store)
            .add_member(&ns_gid, &stayer, GroupMemberRole::Member)
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &leaver, GroupMemberRole::Member)
            .unwrap();

        let new_key: [u8; 32] = OsRng.gen();
        let rotation = GroupKeyring::new(&store, ns_gid)
            .build_rotation(&new_key, &admin_sk, Some(&leaver))
            .expect("build rotation excluding the leaver");

        let recipients: Vec<PublicKey> = rotation.envelopes.iter().map(|e| e.recipient).collect();

        assert!(
            !recipients.contains(&leaver),
            "the leaver must get NO envelope — an envelope for them would hand back the very \
             key the rotation exists to cut them off from"
        );
        assert!(
            recipients.contains(&stayer),
            "every remaining member must get an envelope, or the rotation locks them out of \
             their own group"
        );
        assert!(
            recipients.contains(&admin),
            "the rotating admin must also hold the new key"
        );

        // The leaver cannot unwrap what was never wrapped for them: even handed the
        // whole rotation, no envelope decrypts under their key.
        for envelope in &rotation.envelopes {
            assert!(
                GroupKeyring::unwrap_for_recipient(
                    &leaver_sk,
                    &ns_gid.to_bytes(),
                    Some(&admin),
                    envelope,
                )
                .is_err(),
                "no envelope in the rotation may be unwrappable by the departed member"
            );
        }

        // ...while a member who stayed unwraps their envelope and gets the real key.
        let stayer_envelope = rotation
            .envelopes
            .iter()
            .find(|e| e.recipient == stayer)
            .expect("the stayer has an envelope");
        let unwrapped = GroupKeyring::unwrap_for_recipient(
            &stayer_sk,
            &ns_gid.to_bytes(),
            Some(&admin),
            stayer_envelope,
        )
        .expect("a remaining member must be able to unwrap the new key");
        assert_eq!(
            unwrapped, new_key,
            "the unwrapped key must be the key that was minted"
        );
    }

    #[test]
    fn two_admins_rotating_concurrently_converge_on_one_key_and_neither_is_the_leavers() {
        // This is why the design needs no election. Two admins both react to the same
        // departure and mint DIFFERENT keys. Every node must still end up agreeing which
        // key is current — and, whichever wins, the leaver must hold neither.
        let store_a = test_store();
        let store_b = test_store();
        let gid = test_group_id();

        // Two rotations, minted independently, landing at different DAG sequences.
        let key_from_admin_1: [u8; 32] = OsRng.gen();
        let key_from_admin_2: [u8; 32] = OsRng.gen();

        // Replica A sees admin 1's rotation first, then admin 2's. Replica B sees them
        // in the opposite order — the whole point is that arrival order must not matter.
        let ring_a = GroupKeyring::new(&store_a, gid);
        ring_a.store_key_with_epoch(&key_from_admin_1, 10).unwrap();
        ring_a.store_key_with_epoch(&key_from_admin_2, 11).unwrap();

        let ring_b = GroupKeyring::new(&store_b, gid);
        ring_b.store_key_with_epoch(&key_from_admin_2, 11).unwrap();
        ring_b.store_key_with_epoch(&key_from_admin_1, 10).unwrap();

        let current_a = ring_a.load_current_key_record().unwrap().unwrap();
        let current_b = ring_b.load_current_key_record().unwrap().unwrap();

        assert_eq!(
            current_a.group_key, current_b.group_key,
            "both replicas must select the SAME current key regardless of the order the two \
             concurrent rotations arrived in — otherwise the group splits its keyring"
        );
        assert_eq!(
            current_a.group_key, key_from_admin_2,
            "the higher epoch wins, deterministically"
        );

        // And the safety property that makes the race benign: both candidate keys were
        // minted by rotations that excluded the leaver, so whichever wins, the leaver
        // has neither. Losing the race costs a wasted key, never forward secrecy.
        assert_ne!(key_from_admin_1, key_from_admin_2);
    }

    #[test]
    fn the_leaver_cannot_publish_their_own_rotation() {
        // Belt-and-braces on the core asymmetry: even if a departing member tried to
        // rotate, peers reject a rotation whose signer is not an admin of the group. The
        // leaver is not an admin (their row is gone), so the op is refused — they cannot
        // mint themselves a fresh key and stay in.
        let store = test_store();
        let ns_id = [0xB2u8; 32];
        let ns_gid = ContextGroupId::from(ns_id);
        let (_admin_sk, _admin) = bootstrap_namespace_with_admin(&store, ns_id);

        let leaver_sk = PrivateKey::random(&mut OsRng);
        let leaver = leaver_sk.public_key();
        MembershipRepository::new(&store)
            .add_member(&ns_gid, &leaver, GroupMemberRole::Member)
            .unwrap();

        let leave = SignedGroupOp::sign(
            &leaver_sk,
            ns_gid.to_bytes().into(),
            vec![],
            1,
            calimero_governance_types::GroupOp::MemberLeft {
                member: leaver,
                expected_group_state_hash: [0u8; 32],
                expected_context_state_hashes: Vec::new(),
            },
        )
        .expect("sign MemberLeft");
        apply_local_signed_group_op(&store, &leave).expect("apply MemberLeft");

        let self_rotation = SignedGroupOp::sign(
            &leaver_sk,
            ns_gid.to_bytes().into(),
            vec![],
            2,
            calimero_governance_types::GroupOp::GroupKeyRotated { departed: leaver },
        )
        .expect("sign GroupKeyRotated");

        let _ = apply_local_signed_group_op(&store, &self_rotation).expect_err(
            "a departed member must not be able to rotate the key they are being cut off from",
        );
    }
}

// -----------------------------------------------------------------------
// A parked op must RETRY TO SUCCESS once the missing history arrives.
//
// Refusing to decide is only the right answer if the refusal is recoverable. If an
// `AuthorityUndecidable` left any residue — a burned nonce, a half-applied mutation,
// an advanced head — then "park" would really mean "drop forever", which is worse
// than the live guess it replaces: the op would never apply on this replica even
// after it caught up.
//
// So this drives the real local apply path (nonce window and all): the same signed op
// is applied twice, first against a projection that cannot resolve its cut, then
// against one that can. The first must leave NOTHING behind; the second must succeed.
mod parked_op_retries_to_success {
    use super::*;
    use crate::test_fixtures::{FixedAuthorizer, UnresolvableAuthorizer};
    use calimero_context_config::types::AppKey;
    use calimero_governance_types::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    #[test]
    fn an_undecidable_op_applies_cleanly_once_the_cut_becomes_resolvable() {
        let store = test_store();
        let gid = test_group_id();

        let admin_sk = PrivateKey::random(&mut OsRng);
        let admin = admin_sk.public_key();
        let mut meta = test_meta();
        meta.admin_identity = admin;
        meta.owner_identity = admin;
        MetaRepository::new(&store).save(&gid, &meta).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &admin, GroupMemberRole::Admin)
            .unwrap();

        // A real cut: the op cites a parent, so abstention means "I haven't folded this
        // op's ancestry", not "there is no ancestry".
        let op = SignedGroupOp::sign(
            &admin_sk,
            gid.to_bytes().into(),
            vec![[0xAB; 32]],
            1,
            GroupOp::TargetApplicationSet {
                app_key: AppKey::from([0x5A; 32]),
                target_application_id: ApplicationId::from([0x5B; 32]),
            },
        )
        .expect("sign TargetApplicationSet");

        // --- Arrival 1: this replica is mid-backfill and cannot resolve the cut. ---
        let err = apply_local_signed_group_op_at_cut(&store, &op, &UnresolvableAuthorizer)
            .expect_err("an unresolvable cut must not be decided");
        assert!(
            format!("{err:#}").contains("authority undecidable"),
            "expected a park, got: {err:#}"
        );

        // The park must be inert. Any residue here turns "retry later" into "never".
        let meta_after_park = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
        assert_ne!(
            meta_after_park.app_key, [0x5A; 32],
            "a parked op must not half-apply"
        );
        assert_eq!(
            get_local_gov_nonce(&store, &gid, &admin).unwrap(),
            None,
            "a parked op must NOT burn its nonce — if it did, the retry would be rejected \
             as stale and the op could never apply on this replica"
        );

        // --- Arrival 2: backfill completed, the cut resolves, the op is re-delivered. ---
        apply_local_signed_group_op_at_cut(&store, &op, &FixedAuthorizer(true)).expect(
            "the very same op must apply once the cut is resolvable — otherwise \
                     parking is just a slower way of dropping it",
        );

        let meta_after_retry = MetaRepository::new(&store).load(&gid).unwrap().unwrap();
        assert_eq!(
            meta_after_retry.app_key, [0x5A; 32],
            "the retried op must actually take effect"
        );
        assert_eq!(
            meta_after_retry.target_application_id,
            ApplicationId::from([0x5B; 32]),
            "the retried op's full mutation must land, not just part of it"
        );
    }
}
