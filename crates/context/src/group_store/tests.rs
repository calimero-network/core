use std::sync::Arc;

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
use calimero_store::Store;

use super::*;

fn test_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn test_group_id() -> ContextGroupId {
    ContextGroupId::from([0xAA; 32])
}

fn test_meta() -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: PublicKey::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

// -----------------------------------------------------------------------
// Group meta tests
// -----------------------------------------------------------------------

#[test]
fn save_load_delete_group_meta() {
    let store = test_store();
    let gid = test_group_id();
    let meta = test_meta();

    assert!(load_group_meta(&store, &gid).unwrap().is_none());

    save_group_meta(&store, &gid, &meta).unwrap();
    let loaded = load_group_meta(&store, &gid).unwrap().unwrap();
    assert_eq!(loaded.app_key, meta.app_key);
    assert_eq!(loaded.target_application_id, meta.target_application_id);

    delete_group_meta(&store, &gid).unwrap();
    assert!(load_group_meta(&store, &gid).unwrap().is_none());
}

// -----------------------------------------------------------------------
// Member tests
// -----------------------------------------------------------------------

#[test]
fn add_and_check_membership() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x01; 32]);

    assert!(!check_group_membership(&store, &gid, &pk).unwrap());

    add_group_member(&store, &gid, &pk, GroupMemberRole::Admin).unwrap();
    assert!(check_group_membership(&store, &gid, &pk).unwrap());
    assert!(is_group_admin(&store, &gid, &pk).unwrap());
}

#[test]
fn remove_member() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x02; 32]);

    add_group_member(&store, &gid, &pk, GroupMemberRole::Member).unwrap();
    assert!(check_group_membership(&store, &gid, &pk).unwrap());

    remove_group_member(&store, &gid, &pk).unwrap();
    assert!(!check_group_membership(&store, &gid, &pk).unwrap());
}

#[test]
fn get_member_role() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    assert_eq!(
        get_group_member_role(&store, &gid, &admin).unwrap(),
        Some(GroupMemberRole::Admin)
    );
    assert_eq!(
        get_group_member_role(&store, &gid, &member).unwrap(),
        Some(GroupMemberRole::Member)
    );
    assert!(!is_group_admin(&store, &gid, &member).unwrap());
}

#[test]
fn require_group_admin_rejects_non_admin() {
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x03; 32]);

    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    assert!(require_group_admin(&store, &gid, &member).is_err());
}

#[test]
fn permission_checker_enforces_admin_and_capability_rules() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x10; 32]);
    let member = PublicKey::from([0x11; 32]);
    let outsider = PublicKey::from([0x12; 32]);

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    let checker = PermissionChecker::new(&store, gid);
    assert!(checker.require_admin(&admin).is_ok());
    assert!(checker.require_admin(&member).is_err());

    assert!(checker
        .require_manage_members(&admin, "manage members")
        .is_ok());
    assert!(checker
        .require_manage_members(&member, "manage members")
        .is_err());
    set_member_capability(
        &store,
        &gid,
        &member,
        calimero_context_config::MemberCapabilities::MANAGE_MEMBERS,
    )
    .unwrap();
    assert!(checker
        .require_manage_members(&member, "manage members")
        .is_ok());

    assert!(checker.require_can_create_context(&admin).is_ok());
    assert!(checker.require_can_create_context(&member).is_err());
    set_member_capability(
        &store,
        &gid,
        &member,
        calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT,
    )
    .unwrap();
    assert!(checker.require_can_create_context(&member).is_ok());

    assert!(checker.require_admin_or_self(&member, &member).is_ok());
    assert!(checker.require_admin_or_self(&member, &outsider).is_err());
    assert!(checker.require_admin_or_self(&admin, &outsider).is_ok());
}

#[test]
fn membership_policy_guards_last_admin_and_tee_paths() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin = PrivateKey::random(&mut rng).public_key();
    let admin2 = PrivateKey::random(&mut rng).public_key();
    let member = PrivateKey::random(&mut rng).public_key();
    let outsider = PrivateKey::random(&mut rng).public_key();

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    let membership = MembershipPolicy::new(&store, gid);
    assert!(membership.ensure_not_last_admin_removal(&admin).is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Member)
        .is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Admin)
        .is_ok());

    add_group_member(&store, &gid, &admin2, GroupMemberRole::Admin).unwrap();
    assert!(membership.ensure_not_last_admin_removal(&admin).is_ok());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Member)
        .is_ok());

    assert!(membership
        .require_tee_attestation_verifier_membership(&member)
        .is_ok());
    assert!(membership
        .require_tee_attestation_verifier_membership(&outsider)
        .is_err());
    assert!(membership.read_required_tee_admission_policy().is_err());

    let signer_sk = PrivateKey::random(&mut rng);
    let policy_op = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m1".to_owned()],
            allowed_rtmr0: vec!["r0".to_owned()],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: false,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 1, &borsh::to_vec(&policy_op).unwrap()).unwrap();

    let policy = membership.read_required_tee_admission_policy().unwrap();
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "m1", "r0", "x", "y", "z", "ok")
        .is_ok());
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "wrong", "r0", "x", "y", "z", "ok")
        .is_err());
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "m1", "wrong", "x", "y", "z", "ok")
        .is_err());

    let tee_joined = PrivateKey::random(&mut rng).public_key();
    assert!(!check_group_membership(&store, &gid, &tee_joined).unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(check_group_membership(&store, &gid, &tee_joined).unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(check_group_membership(&store, &gid, &tee_joined).unwrap());
}

#[test]
fn membership_policy_rules_report_rejection_reasons() {
    use super::membership_policy_rules::{
        validate_tee_attestation_allowlists, MembershipPolicyRejection, TeeAllowlistPolicy,
        TeeAttestationClaims,
    };

    let policy = TeeAllowlistPolicy {
        allowed_mrtd: vec!["m-ok".to_owned()],
        allowed_rtmr0: vec!["r0-ok".to_owned()],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec!["ok".to_owned()],
    };
    let claims = TeeAttestationClaims {
        mrtd: "m-ok",
        rtmr0: "r0-ok",
        rtmr1: "anything",
        rtmr2: "anything",
        rtmr3: "anything",
        tcb_status: "ok",
    };

    assert!(validate_tee_attestation_allowlists(&policy, &claims).is_ok());

    let bad_mrtd = TeeAttestationClaims {
        mrtd: "m-bad",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_mrtd).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::MrtdNotAllowed);

    let bad_rtmr0 = TeeAttestationClaims {
        rtmr0: "r0-bad",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_rtmr0).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr0NotAllowed);

    let bad_tcb = TeeAttestationClaims {
        tcb_status: "warn",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_tcb).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::TcbStatusNotAllowed);
}

#[test]
fn membership_view_reports_admin_and_member_counts() {
    let store = test_store();
    let gid = test_group_id();
    let admin1 = PublicKey::from([0xD1; 32]);
    let admin2 = PublicKey::from([0xD2; 32]);
    let member = PublicKey::from([0xD3; 32]);

    add_group_member(&store, &gid, &admin1, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &admin2, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    let view = GroupMembershipView::new(&store, gid);
    assert!(view.is_admin(&admin1).unwrap());
    assert!(!view.is_admin(&member).unwrap());
    assert_eq!(view.admin_count().unwrap(), 2);
    assert_eq!(view.member_count().unwrap(), 3);
}

#[test]
fn group_settings_service_enforces_permissions_and_persists_values() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x21; 32]);
    let member = PublicKey::from([0x22; 32]);
    let app_id = ApplicationId::from([0x23; 32]);

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    save_group_meta(&store, &gid, &test_meta()).unwrap();

    let settings = GroupSettingsService::new(&store, gid);

    assert!(settings
        .set_subgroup_visibility(&member, calimero_context_config::VisibilityMode::Restricted)
        .is_err());
    settings
        .set_subgroup_visibility(&admin, calimero_context_config::VisibilityMode::Restricted)
        .unwrap();
    assert_eq!(
        get_subgroup_visibility(&store, &gid).unwrap(),
        calimero_context_config::VisibilityMode::Restricted
    );

    settings.set_default_capabilities(&admin, 0b101).unwrap();
    assert_eq!(get_default_capabilities(&store, &gid).unwrap(), Some(0b101));

    assert!(settings
        .set_group_migration(&member, &Some(vec![1, 2, 3]))
        .is_err());
    set_member_capability(
        &store,
        &gid,
        &member,
        calimero_context_config::MemberCapabilities::MANAGE_APPLICATION,
    )
    .unwrap();
    settings
        .set_group_migration(&member, &Some(vec![1, 2, 3]))
        .unwrap();
    assert_eq!(
        load_group_meta(&store, &gid).unwrap().unwrap().migration,
        Some(vec![1, 2, 3])
    );

    settings
        .set_target_application(&member, &[0xAB; 32], &app_id)
        .unwrap();
    let meta = load_group_meta(&store, &gid).unwrap().unwrap();
    assert_eq!(meta.app_key, [0xAB; 32]);
    assert_eq!(meta.target_application_id, app_id);

    settings.set_group_alias(&admin, "group-main").unwrap();
    assert_eq!(
        get_group_alias(&store, &gid).unwrap().as_deref(),
        Some("group-main")
    );
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

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &creator, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &gid,
        &creator,
        calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT,
    )
    .unwrap();

    let mut meta = test_meta();
    meta.target_application_id = calimero_primitives::application::ZERO_APPLICATION_ID;
    save_group_meta(&store, &gid, &meta).unwrap();

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
        load_group_meta(&store, &gid)
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
            &calimero_store::key::ContextIdentity::new(context, member.into()),
            &calimero_store::types::ContextIdentity {
                private_key: None,
                sender_key: Some([0u8; 32]),
            },
        )
        .unwrap();
    drop(handle);

    tree_b.cascade_remove_member(&member).unwrap();
    let handle = store.handle();
    let identity_key = calimero_store::key::ContextIdentity::new(context, member.into());
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

    add_group_member(&store, &gid, &creator, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &gid,
        &creator,
        calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT,
    )
    .unwrap();
    save_group_meta(&store, &gid, &test_meta()).unwrap();

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
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member_pk = PrivateKey::random(&mut rng).public_key();

    let op1 = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAdded {
            member: member_pk,
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op1).unwrap();
    assert!(check_group_membership(&store, &gid, &member_pk).unwrap());

    let op_dup_nonce =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], [0u8; 32], 1, GroupOp::Noop).unwrap();
    assert!(
        apply_local_signed_group_op(&store, &op_dup_nonce).is_ok(),
        "duplicate nonce should be silently accepted (idempotent)"
    );

    let op2 =
        SignedGroupOp::sign(&admin_sk, gid_bytes, vec![], [0u8; 32], 2, GroupOp::Noop).unwrap();
    apply_local_signed_group_op(&store, &op2).unwrap();

    let non_admin_sk = PrivateKey::random(&mut rng);
    add_group_member(
        &store,
        &gid,
        &non_admin_sk.public_key(),
        GroupMemberRole::Member,
    )
    .unwrap();
    let op_bad = SignedGroupOp::sign(
        &non_admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAdded {
            member: PrivateKey::random(&mut rng).public_key(),
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
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
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAdded {
            member: tee_pk,
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        err.to_string().contains("ReadOnlyTee"),
        "expected ReadOnlyTee rejection, got: {err}"
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
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    add_group_member(&store, &gid, &member_pk, GroupMemberRole::Member).unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberRoleSet {
            member: member_pk,
            role: GroupMemberRole::ReadOnlyTee,
        },
    )
    .unwrap();
    let err = apply_local_signed_group_op(&store, &op).unwrap_err();
    assert!(
        err.to_string().contains("ReadOnlyTee"),
        "expected ReadOnlyTee rejection, got: {err}"
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
    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    add_group_member(&store, &gid, &member_pk, GroupMemberRole::Member).unwrap();

    let op = SignedGroupOp::sign(
        &member_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAliasSet {
            member: member_pk,
            alias: "alice".to_owned(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op).unwrap();
    assert_eq!(
        get_member_alias(&store, &gid, &member_pk)
            .unwrap()
            .as_deref(),
        Some("alice")
    );

    let other_sk = PrivateKey::random(&mut rng);
    let op_bad = SignedGroupOp::sign(
        &other_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAliasSet {
            member: member_pk,
            alias: "bob".to_owned(),
        },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());

    let admin_op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAliasSet {
            member: member_pk,
            alias: "carol".to_owned(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &admin_op).unwrap();
    assert_eq!(
        get_member_alias(&store, &gid, &member_pk)
            .unwrap()
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
    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let creator_sk = PrivateKey::random(&mut rng);
    let creator_pk = creator_sk.public_key();
    add_group_member(&store, &gid, &creator_pk, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &gid,
        &creator_pk,
        MemberCapabilities::CAN_CREATE_CONTEXT,
    )
    .unwrap();

    let context_id = ContextId::from([0x33; 32]);

    let op_reg = SignedGroupOp::sign(
        &creator_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
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
        gid_bytes,
        vec![],
        [0u8; 32],
        2,
        GroupOp::ContextAliasSet {
            context_id,
            alias: "from-creator".to_owned(),
        },
    )
    .unwrap();
    assert!(
        apply_local_signed_group_op(&store, &op_creator_alias).is_err(),
        "non-admin creator should be rejected"
    );

    let op_admin = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::ContextAliasSet {
            context_id,
            alias: "from-admin".to_owned(),
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_admin).unwrap();
    assert_eq!(
        get_context_alias(&store, &gid, &context_id)
            .unwrap()
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

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let member_m = PrivateKey::random(&mut rng).public_key();
    add_group_member(&store, &gid, &member_m, GroupMemberRole::Member).unwrap();

    let op_caps = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberCapabilitySet {
            member: member_m,
            capabilities: 0x7,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_caps).unwrap();
    assert_eq!(
        get_member_capability(&store, &gid, &member_m)
            .unwrap()
            .unwrap(),
        0x7
    );

    let op_policy = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        2,
        GroupOp::UpgradePolicySet {
            policy: UpgradePolicy::Automatic,
        },
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_policy).unwrap();
    assert_eq!(
        load_group_meta(&store, &gid)
            .unwrap()
            .unwrap()
            .upgrade_policy,
        UpgradePolicy::Automatic
    );

    let op_del = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        3,
        GroupOp::GroupDelete,
    )
    .unwrap();
    apply_local_signed_group_op(&store, &op_del).unwrap();
    assert!(load_group_meta(&store, &gid).unwrap().is_none());
}

#[test]
fn apply_local_signed_group_op_rejects_last_admin_removal() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let gid_bytes = gid.to_bytes();
    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).unwrap();

    let op_bad = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberRemoved { member: admin_pk },
    )
    .unwrap();
    assert!(apply_local_signed_group_op(&store, &op_bad).is_err());
}

#[test]
fn count_members_and_admins() {
    let store = test_store();
    let gid = test_group_id();

    assert_eq!(count_group_members(&store, &gid).unwrap(), 0);
    assert_eq!(count_group_admins(&store, &gid).unwrap(), 0);

    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x01; 32]),
        GroupMemberRole::Admin,
    )
    .unwrap();
    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x02; 32]),
        GroupMemberRole::Member,
    )
    .unwrap();
    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x03; 32]),
        GroupMemberRole::Admin,
    )
    .unwrap();

    assert_eq!(count_group_members(&store, &gid).unwrap(), 3);
    assert_eq!(count_group_admins(&store, &gid).unwrap(), 2);
}

#[test]
fn list_members_with_offset_and_limit() {
    let store = test_store();
    let gid = test_group_id();

    for i in 0u8..5 {
        let mut pk_bytes = [0u8; 32];
        pk_bytes[0] = i;
        add_group_member(
            &store,
            &gid,
            &PublicKey::from(pk_bytes),
            GroupMemberRole::Member,
        )
        .unwrap();
    }

    let all = list_group_members(&store, &gid, 0, 100).unwrap();
    assert_eq!(all.len(), 5);

    let page = list_group_members(&store, &gid, 1, 2).unwrap();
    assert_eq!(page.len(), 2);
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

    assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());

    store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
    let loaded = get_group_signing_key(&store, &gid, &pk).unwrap().unwrap();
    assert_eq!(loaded, sk);
}

#[test]
fn delete_signing_key() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
    delete_group_signing_key(&store, &gid, &pk).unwrap();
    assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());
}

#[test]
fn require_signing_key_fails_when_missing() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x10; 32]);

    assert!(require_group_signing_key(&store, &gid, &pk).is_err());
}

#[test]
fn delete_all_group_signing_keys_removes_all() {
    let store = test_store();
    let gid = test_group_id();
    let pk1 = PublicKey::from([0x10; 32]);
    let pk2 = PublicKey::from([0x11; 32]);

    store_group_signing_key(&store, &gid, &pk1, &[0xAA; 32]).unwrap();
    store_group_signing_key(&store, &gid, &pk2, &[0xBB; 32]).unwrap();

    delete_all_group_signing_keys(&store, &gid).unwrap();

    assert!(get_group_signing_key(&store, &gid, &pk1).unwrap().is_none());
    assert!(get_group_signing_key(&store, &gid, &pk2).unwrap().is_none());
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
    assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 1);

    register_context_in_group(&store, &gid2, &cid).unwrap();
    assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 0);
    assert_eq!(count_group_contexts(&store, &gid2).unwrap(), 1);
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

    assert_eq!(count_group_contexts(&store, &gid).unwrap(), 4);

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

    assert!(load_group_upgrade(&store, &gid).unwrap().is_none());

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
    };

    save_group_upgrade(&store, &gid, &upgrade).unwrap();
    let loaded = load_group_upgrade(&store, &gid).unwrap().unwrap();
    assert_eq!(loaded.from_version, "1.0.0");
    assert_eq!(loaded.to_version, "2.0.0");

    delete_group_upgrade(&store, &gid).unwrap();
    assert!(load_group_upgrade(&store, &gid).unwrap().is_none());
}

#[test]
fn enumerate_in_progress_upgrades_filters_completed() {
    let store = test_store();
    let gid_in_progress = ContextGroupId::from([0x01; 32]);
    let gid_completed = ContextGroupId::from([0x02; 32]);

    save_group_upgrade(
        &store,
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
        },
    )
    .unwrap();

    save_group_upgrade(
        &store,
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
        },
    )
    .unwrap();

    let results = enumerate_in_progress_upgrades(&store).unwrap();
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

    save_group_meta(&store, &gid, &meta).unwrap();
    // Add a group member — this writes a GroupMember key (prefix 0x21)
    // into the same column, right after GroupMeta keys (prefix 0x20).
    add_group_member(&store, &gid, &member, GroupMemberRole::Admin).unwrap();

    // Must return exactly one group without panicking.
    let groups = enumerate_all_groups(&store, 0, 100).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].0, gid.to_bytes());
}

#[test]
fn enumerate_all_groups_multiple_groups_with_members() {
    let store = test_store();
    let gid1 = ContextGroupId::from([0x01; 32]);
    let gid2 = ContextGroupId::from([0x02; 32]);
    let meta = test_meta();

    save_group_meta(&store, &gid1, &meta).unwrap();
    save_group_meta(&store, &gid2, &meta).unwrap();
    add_group_member(
        &store,
        &gid1,
        &PublicKey::from([0xAA; 32]),
        GroupMemberRole::Admin,
    )
    .unwrap();
    add_group_member(
        &store,
        &gid2,
        &PublicKey::from([0xBB; 32]),
        GroupMemberRole::Member,
    )
    .unwrap();

    let groups = enumerate_all_groups(&store, 0, 100).unwrap();
    assert_eq!(groups.len(), 2);

    // Pagination
    let page = enumerate_all_groups(&store, 1, 1).unwrap();
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
    assert!(get_member_capability(&store, &gid, &pk).unwrap().is_none());

    // Set capabilities
    set_member_capability(&store, &gid, &pk, 0b101).unwrap();
    let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
    assert_eq!(caps, 0b101);

    // Update capabilities
    set_member_capability(&store, &gid, &pk, 0b111).unwrap();
    let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
    assert_eq!(caps, 0b111);
}

#[test]
fn capability_zero_means_no_permissions() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x11; 32]);

    set_member_capability(&store, &gid, &pk, 0).unwrap();
    let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
    assert_eq!(caps, 0);
    // All capability bits are off
    assert_eq!(caps & (1 << 0), 0); // CAN_CREATE_CONTEXT
    assert_eq!(caps & (1 << 1), 0); // CAN_INVITE_MEMBERS
    assert_eq!(caps & (1 << 2), 0); // CAN_JOIN_OPEN_SUBGROUPS
}

#[test]
fn capabilities_isolated_per_member() {
    let store = test_store();
    let gid = test_group_id();
    let alice = PublicKey::from([0x12; 32]);
    let bob = PublicKey::from([0x13; 32]);

    set_member_capability(&store, &gid, &alice, 0b001).unwrap();
    set_member_capability(&store, &gid, &bob, 0b110).unwrap();

    assert_eq!(
        get_member_capability(&store, &gid, &alice)
            .unwrap()
            .unwrap(),
        0b001
    );
    assert_eq!(
        get_member_capability(&store, &gid, &bob).unwrap().unwrap(),
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

    assert!(get_default_capabilities(&store, &gid).unwrap().is_none());

    set_default_capabilities(&store, &gid, 0b100).unwrap();
    assert_eq!(
        get_default_capabilities(&store, &gid).unwrap().unwrap(),
        0b100
    );

    // Update
    set_default_capabilities(&store, &gid, 0b111).unwrap();
    assert_eq!(
        get_default_capabilities(&store, &gid).unwrap().unwrap(),
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
        get_subgroup_visibility(&store, &gid).unwrap(),
        VisibilityMode::Restricted
    );

    set_subgroup_visibility(&store, &gid, VisibilityMode::Open).unwrap();
    assert_eq!(
        get_subgroup_visibility(&store, &gid).unwrap(),
        VisibilityMode::Open
    );

    set_subgroup_visibility(&store, &gid, VisibilityMode::Restricted).unwrap();
    assert_eq!(
        get_subgroup_visibility(&store, &gid).unwrap(),
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
fn nest_for_test(store: &Store, parent: &ContextGroupId, child: &ContextGroupId) {
    nest_group(store, parent, child).unwrap();
}

#[test]
fn check_membership_open_subgroup_inherits_parent_with_default_cap() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let parent = ContextGroupId::from([0xB0; 32]);
    let child = ContextGroupId::from([0xB1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);

    // Alice is a direct member of the parent with the default cap set.
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // Child is `Open`. Alice should be inherited as a member.
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_restricted_subgroup_does_not_inherit() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let parent = ContextGroupId::from([0xB2; 32]);
    let child = ContextGroupId::from([0xB3; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // Restricted child blocks inheritance even when the cap is set.
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_restricted_wall_blocks_grandparent_inheritance() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> restricted_mid -> open_leaf
    let store = test_store();
    let namespace = ContextGroupId::from([0xC0; 32]);
    let mid = ContextGroupId::from([0xC1; 32]);
    let leaf = ContextGroupId::from([0xC2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Restricted).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    // The walk hits `mid` (Restricted) and stops before reaching the
    // namespace; alice is not inherited into `leaf`.
    assert!(!check_group_membership(&store, &leaf, &alice).unwrap());
}

#[test]
fn check_membership_open_chain_walks_to_root() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> open_mid -> open_leaf, member only at namespace
    let store = test_store();
    let namespace = ContextGroupId::from([0xD0; 32]);
    let mid = ContextGroupId::from([0xD1; 32]);
    let leaf = ContextGroupId::from([0xD2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    assert!(check_group_membership(&store, &leaf, &alice).unwrap());
}

#[test]
fn check_membership_unset_visibility_treated_as_restricted() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // No `subgroup_visibility` set on `child` — should behave as Restricted.
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_open_subgroup_blocked_when_cap_revoked() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    // Cap explicitly cleared (admin used the deny-list).
    set_member_capability(&store, &parent, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_open_subgroup_admin_inherits_without_cap() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0x10; 32]);
    let child = ContextGroupId::from([0x11; 32]);
    let admin = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &admin, GroupMemberRole::Admin).unwrap();
    // Cap cleared — but admin override kicks in.
    set_member_capability(&store, &parent, &admin, 0).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(check_group_membership(&store, &child, &admin).unwrap());
}

#[test]
fn check_membership_anchor_cap_check_uses_deepest_direct_membership() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> mid -> open_leaf
    // Alice is a direct member of BOTH `namespace` (cap set) and `mid`
    // (cap cleared). For `open_leaf`, the walk anchors at `mid` (the
    // deepest direct membership), where cap is cleared → false.
    let store = test_store();
    let namespace = ContextGroupId::from([0x20; 32]);
    let mid = ContextGroupId::from([0x21; 32]);
    let leaf = ContextGroupId::from([0x22; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    add_group_member(&store, &mid, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    assert!(!check_group_membership(&store, &leaf, &alice).unwrap());
}

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

    set_default_capabilities(&store, &gid, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS).unwrap();
    add_group_member(&store, &gid, &alice, GroupMemberRole::Member).unwrap();

    let caps = get_member_capability(&store, &gid, &alice)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
    );
}

#[test]
fn inherited_admin_walk_independent_of_direct_non_admin_membership() {
    use calimero_context_config::VisibilityMode;

    use super::is_inherited_admin;

    // Bugbot finding (PR #2261): the previous `is_admin` reused
    // `check_group_membership_path`, which short-circuits to `Direct`
    // when the identity has *any* direct membership row in the target
    // subgroup — even a non-admin `Member` row. That suppressed
    // inherited admin authority for parent admins who happened to
    // also be added as explicit subgroup members. The dedicated
    // `is_inherited_admin` walk is independent of non-admin direct
    // membership.
    let store = test_store();
    let parent = ContextGroupId::from([0x50; 32]);
    let child = ContextGroupId::from([0x51; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);

    // Alice is namespace admin AND a non-admin direct member of the
    // child subgroup (e.g. she opted into a subgroup-specific role
    // for visibility, but her parent admin authority should still
    // apply).
    add_group_member(&store, &parent, &alice, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Inherited admin authority must hold despite Alice's direct
    // non-admin membership in `child`.
    assert!(
        is_inherited_admin(&store, &child, &alice).unwrap(),
        "parent admin should retain admin authority in child subgroup \
         regardless of any direct non-admin membership row"
    );
}

#[test]
fn membership_path_inherited_admin_overrides_anchor_cap_denial() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // Bugbot finding (PR #2261, comment 3146210600): governance
    // (`is_inherited_admin`) and context-join
    // (`check_group_membership_path`) used to disagree when an
    // identity held admin authority at a higher ancestor but ALSO
    // happened to have a direct non-admin row at an intermediate
    // level with `CAN_JOIN_OPEN_SUBGROUPS` cleared. The old walk
    // anchored at the intermediate row and denied join — yielding
    // the confusing "can govern but cannot join" UX.
    //
    // After the fix, admin authority cascades: the walk keeps going
    // past a non-admin direct row and finds the parent admin
    // anchor, returning `Inherited { via_admin: true }`.
    let store = test_store();
    let ns = ContextGroupId::from([0xE0; 32]);
    let mid = ContextGroupId::from([0xE1; 32]);
    let leaf = ContextGroupId::from([0xE2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &ns, &mid);
    nest_for_test(&store, &mid, &leaf);

    // Alice: namespace admin AND a direct non-admin Member at `mid`
    // with the join cap explicitly cleared.
    add_group_member(&store, &ns, &alice, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &mid, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    // Both authorization surfaces must agree: Alice is authorized.
    assert!(super::is_inherited_admin(&store, &leaf, &alice).unwrap());
    let path = check_group_membership_path(&store, &leaf, &alice).unwrap();
    match path {
        super::MembershipPath::Inherited {
            anchor,
            via_admin: true,
        } => {
            assert_eq!(
                anchor, ns,
                "admin anchor should be the namespace, not the intermediate `mid` row"
            );
        }
        other => panic!(
            "expected Inherited{{ via_admin: true, anchor: ns }} for parent admin, got {:?}",
            other
        ),
    }

    // Sanity: a non-admin in the same shape must still be denied —
    // the fix does NOT widen authorization for non-admins, only
    // honors admin authority that already exists higher up.
    let bob = PublicKey::from([0x02; 32]);
    add_group_member(&store, &ns, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &ns,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    add_group_member(&store, &mid, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &bob, 0).unwrap();
    assert!(
        !check_group_membership(&store, &leaf, &bob).unwrap(),
        "non-admin with cleared cap at intermediate anchor must still be denied; \
         the fix only cascades *admin* authority, not arbitrary parent membership"
    );
}

#[test]
fn is_open_chain_to_namespace_walks_parent_chain_correctly() {
    use calimero_context_config::VisibilityMode;

    use super::is_open_chain_to_namespace;

    // Tree: ns -> mid -> leaf. This is the input shape the
    // visibility-flip encryption special-case in
    // `GroupGovernancePublisher` feeds into when it queries the
    // **parent chain** of a `SubgroupVisibilitySet` op (i.e. it
    // calls `is_open_chain_to_namespace(parent, ns)` instead of
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
    assert!(!is_open_chain_to_namespace(&store, &ns, &ns).unwrap());

    // Direct child of the namespace: parent chain trivially Open
    // when `mid` itself is Open.
    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    assert!(is_open_chain_to_namespace(&store, &mid, &ns).unwrap());

    // Two-hop chain, all Open → boundary is namespace-wide.
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();
    assert!(is_open_chain_to_namespace(&store, &leaf, &ns).unwrap());

    // Restricted wall at mid → boundary is NOT namespace-wide,
    // even if leaf itself is Open.
    set_subgroup_visibility(&store, &mid, VisibilityMode::Restricted).unwrap();
    assert!(!is_open_chain_to_namespace(&store, &leaf, &ns).unwrap());

    // The visibility-flip publisher special-case calls this with
    // the *parent* of the flipping group — `mid` here, walking up
    // to `ns`. With mid currently Restricted that returns false;
    // re-open mid and confirm we get true.
    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    assert!(is_open_chain_to_namespace(&store, &mid, &ns).unwrap());
}

#[test]
fn auth_and_crypto_walks_agree_at_max_namespace_depth_boundary() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    use super::namespace::MAX_NAMESPACE_DEPTH;
    use super::{is_inherited_admin, is_open_chain_to_namespace};

    // Bugbot finding (PR #2261, comment 3146841673): at chain length
    // exactly `MAX_NAMESPACE_DEPTH`, `is_open_chain_to_namespace`
    // succeeds (encrypt path selects namespace key) while the
    // membership walks bailed with a spurious cycle error (auth path
    // refused). The two layers must agree on the corruption signal:
    // either both succeed or both bail. The fix bumps the membership
    // walks to `MAX_NAMESPACE_DEPTH + 1` iterations so they have the
    // same effective reach as the chain check.
    let store = test_store();

    // Build chain of length MAX_NAMESPACE_DEPTH from leaf to ns:
    // ns -> g_1 -> g_2 -> ... -> g_{MAX-1} -> leaf
    // (i.e. MAX_NAMESPACE_DEPTH parent-edges separate `leaf` from `ns`.)
    let ns = ContextGroupId::from([0xF0; 32]);
    let mut nodes = vec![ns];
    for i in 1..=MAX_NAMESPACE_DEPTH {
        let g = ContextGroupId::from([0xF0u8.wrapping_add(i as u8); 32]);
        nest_for_test(&store, nodes.last().unwrap(), &g);
        // Mark every non-root link `Open` so the chain is fully open.
        set_subgroup_visibility(&store, &g, VisibilityMode::Open).unwrap();
        nodes.push(g);
    }
    let leaf = *nodes.last().unwrap();

    // Sanity: chain check succeeds at the boundary.
    assert!(
        is_open_chain_to_namespace(&store, &leaf, &ns).unwrap(),
        "is_open_chain_to_namespace should resolve at chain length MAX_NAMESPACE_DEPTH"
    );

    // The bug: membership walks used to bail here. After the fix,
    // they must resolve to a definite answer (no cycle error).
    let alice = PublicKey::from([0x01; 32]);
    add_group_member(&store, &ns, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &ns,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // is_inherited_admin: alice is not admin anywhere → should
    // resolve to false (NOT bail).
    assert!(
        matches!(is_inherited_admin(&store, &leaf, &alice), Ok(false)),
        "is_inherited_admin must terminate at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // check_group_membership: alice has CAN_JOIN_OPEN_SUBGROUPS at
    // the namespace, all intermediate links are Open → should
    // resolve to true via inheritance, not bail.
    assert!(
        matches!(check_group_membership(&store, &leaf, &alice), Ok(true)),
        "check_group_membership must resolve at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // Promoting alice to admin should also be observed (governance
    // surface in agreement).
    let bob = PublicKey::from([0x02; 32]);
    add_group_member(&store, &ns, &bob, GroupMemberRole::Admin).unwrap();
    assert!(
        matches!(is_inherited_admin(&store, &leaf, &bob), Ok(true)),
        "inherited admin authority must reach the leaf at chain length MAX_NAMESPACE_DEPTH"
    );
}

#[test]
fn is_open_chain_to_namespace_bails_on_depth_overflow() {
    use calimero_context_config::VisibilityMode;

    use super::is_open_chain_to_namespace;
    use super::namespace::MAX_NAMESPACE_DEPTH;

    // Build a chain longer than MAX_NAMESPACE_DEPTH so the walk
    // exhausts its bound without finding the namespace. This used
    // to silently return Ok(false); the fix bails so authorization
    // and crypto-key selection both surface the corruption signal.
    let store = test_store();
    let ns = ContextGroupId::from([0xC0; 32]);
    let mut prev = ns;
    for i in 0..(MAX_NAMESPACE_DEPTH + 2) {
        let next = ContextGroupId::from([0xD0u8.wrapping_add(i as u8); 32]);
        nest_for_test(&store, &prev, &next);
        set_subgroup_visibility(&store, &next, VisibilityMode::Open).unwrap();
        prev = next;
    }
    // Walking from the deepest node should hit the depth bound
    // before reaching `ns` and return an error rather than
    // Ok(false).
    let res = is_open_chain_to_namespace(&store, &prev, &ns);
    assert!(
        res.is_err(),
        "is_open_chain_to_namespace must bail on MAX_NAMESPACE_DEPTH overflow, \
         got {:?}",
        res
    );
}

#[test]
fn has_direct_group_member_ignores_open_chain_inheritance() {
    use calimero_context_config::VisibilityMode;

    use super::has_direct_group_member;

    // Bugbot finding (PR #2261): a previous version of the bootstrap /
    // dedup guards in `store_group_meta`, `apply_member_joined`, and
    // `admit_tee_node` used the inheritance-aware `check_group_membership`,
    // which would silently report `true` for an identity that holds
    // membership only via an Open parent — and skip writing the direct
    // row that those handlers exist to create. `has_direct_group_member`
    // is the direct-only counterpart that those guards must use.
    let store = test_store();
    let parent = ContextGroupId::from([0x60; 32]);
    let child = ContextGroupId::from([0x61; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Inheritance-aware path *should* see Alice (admin inheritance from parent).
    assert!(check_group_membership(&store, &child, &alice).unwrap());

    // Direct-only path *must not* see her — that's exactly the signal
    // the bootstrap/dedup guards need to know they still have to write
    // the direct row.
    assert!(
        !has_direct_group_member(&store, &child, &alice).unwrap(),
        "has_direct_group_member must ignore Open-chain inheritance and \
         report only on the direct membership row"
    );

    // After explicitly adding her to the child, both views agree.
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();
    assert!(has_direct_group_member(&store, &child, &alice).unwrap());
    assert!(check_group_membership(&store, &child, &alice).unwrap());
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
    set_default_capabilities(&store, &gid, 0).unwrap();
    add_group_member(&store, &gid, &alice, GroupMemberRole::Member).unwrap();

    // alice should NOT have any capability bits; in particular she
    // should NOT have CAN_JOIN_OPEN_SUBGROUPS just because a hard-coded
    // path snuck it in.
    let caps = get_member_capability(&store, &gid, &alice)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        caps, 0,
        "admin override default=0 should give member caps=0, got {caps}"
    );

    // Symmetric check with a non-zero non-default value.
    let bob = PublicKey::from([0x02; 32]);
    let custom = calimero_context_config::MemberCapabilities::CAN_CREATE_CONTEXT
        | calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS;
    set_default_capabilities(&store, &gid, custom).unwrap();
    add_group_member(&store, &gid, &bob, GroupMemberRole::Member).unwrap();
    let bob_caps = get_member_capability(&store, &gid, &bob)
        .unwrap()
        .unwrap_or(0);
    assert_eq!(
        bob_caps, custom,
        "admin override default={custom} should give member caps={custom}, got {bob_caps}"
    );
}

#[test]
fn check_membership_direct_member_of_subgroup_always_passes() {
    use calimero_context_config::VisibilityMode;

    // Direct membership short-circuits the walk regardless of visibility
    // setting on the subgroup.
    let store = test_store();
    let parent = ContextGroupId::from([0x30; 32]);
    let child = ContextGroupId::from([0x31; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // No parent membership; alice is added directly to the Restricted child.
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();
    assert!(check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn defaults_isolated_per_group() {
    let store = test_store();
    let g1 = ContextGroupId::from([0x40; 32]);
    let g2 = ContextGroupId::from([0x41; 32]);

    use calimero_context_config::VisibilityMode;

    set_default_capabilities(&store, &g1, 0b001).unwrap();
    set_default_capabilities(&store, &g2, 0b110).unwrap();
    set_subgroup_visibility(&store, &g1, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &g2, VisibilityMode::Restricted).unwrap();

    assert_eq!(
        get_default_capabilities(&store, &g1).unwrap().unwrap(),
        0b001
    );
    assert_eq!(
        get_default_capabilities(&store, &g2).unwrap().unwrap(),
        0b110
    );
    assert_eq!(
        get_subgroup_visibility(&store, &g1).unwrap(),
        VisibilityMode::Open
    );
    assert_eq!(
        get_subgroup_visibility(&store, &g2).unwrap(),
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

    assert!(
        get_context_member_capability(&store, &gid, &context_a, &alice)
            .unwrap()
            .is_none()
    );

    set_context_member_capability(&store, &gid, &context_a, &alice, 0b001).unwrap();
    set_context_member_capability(&store, &gid, &context_a, &bob, 0b010).unwrap();
    set_context_member_capability(&store, &gid, &context_b, &alice, 0b111).unwrap();

    assert_eq!(
        get_context_member_capability(&store, &gid, &context_a, &alice)
            .unwrap()
            .unwrap(),
        0b001
    );
    assert_eq!(
        get_context_member_capability(&store, &gid, &context_a, &bob)
            .unwrap()
            .unwrap(),
        0b010
    );
    assert_eq!(
        get_context_member_capability(&store, &gid, &context_b, &alice)
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

    set_default_capabilities(&store, &gid, 0b101).unwrap();
    set_subgroup_visibility(&store, &gid, VisibilityMode::Restricted).unwrap();
    set_member_capability(&store, &gid, &alice, 0b001).unwrap();
    set_member_capability(&store, &gid, &bob, 0b010).unwrap();
    assert_eq!(
        enumerate_member_capabilities(&store, &gid).unwrap().len(),
        2
    );

    delete_default_capabilities(&store, &gid).unwrap();
    delete_subgroup_visibility(&store, &gid).unwrap();
    delete_all_member_capabilities(&store, &gid).unwrap();

    assert!(get_default_capabilities(&store, &gid).unwrap().is_none());
    // Subgroup visibility's contract is "absent reads as Restricted",
    // so a successful delete is observed as the default value coming back.
    assert_eq!(
        get_subgroup_visibility(&store, &gid).unwrap(),
        VisibilityMode::Restricted
    );
    assert!(get_member_capability(&store, &gid, &alice)
        .unwrap()
        .is_none());
    assert!(get_member_capability(&store, &gid, &bob).unwrap().is_none());
    assert!(enumerate_member_capabilities(&store, &gid)
        .unwrap()
        .is_empty());
}

#[test]
fn migration_tracking_roundtrip_and_cleanup() {
    let store = test_store();
    let gid = test_group_id();
    let context_a = ContextId::from([0x51; 32]);
    let context_b = ContextId::from([0x52; 32]);

    assert!(get_context_last_migration(&store, &gid, &context_a)
        .unwrap()
        .is_none());

    set_context_last_migration(&store, &gid, &context_a, "migrate_v2").unwrap();
    set_context_last_migration(&store, &gid, &context_b, "migrate_v3").unwrap();

    assert_eq!(
        get_context_last_migration(&store, &gid, &context_a)
            .unwrap()
            .as_deref(),
        Some("migrate_v2")
    );
    assert_eq!(
        get_context_last_migration(&store, &gid, &context_b)
            .unwrap()
            .as_deref(),
        Some("migrate_v3")
    );

    delete_all_context_last_migrations(&store, &gid).unwrap();
    assert!(get_context_last_migration(&store, &gid, &context_a)
        .unwrap()
        .is_none());
    assert!(get_context_last_migration(&store, &gid, &context_b)
        .unwrap()
        .is_none());
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
    save_group_meta(
        &store,
        &auto_group_id,
        &GroupMetaValue {
            app_key: [0u8; 32],
            target_application_id: ApplicationId::from([0xCC; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: node_pk,
            migration: None,
            auto_join: true,
        },
    )
    .unwrap();

    // Add node identity as admin with keys (as create_context does)
    add_group_member_with_keys(
        &store,
        &auto_group_id,
        &node_pk,
        GroupMemberRole::Admin,
        Some(*node_sk),
        Some(*sender_key),
    )
    .unwrap();

    // Register the context in the group
    register_context_in_group(&store, &auto_group_id, &context_id).unwrap();

    // The node's identity should be recognized as a group member
    assert!(check_group_membership(&store, &auto_group_id, &node_pk).unwrap());
    assert!(is_group_admin(&store, &auto_group_id, &node_pk).unwrap());

    // The group should have exactly 1 member
    assert_eq!(count_group_members(&store, &auto_group_id).unwrap(), 1);

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
    add_group_member(&store, &auto_group_id, &random_pk, GroupMemberRole::Admin).unwrap();

    // The node's ACTUAL group identity is different
    let node_sk = PrivateKey::random(&mut OsRng);
    let node_pk = node_sk.public_key();

    // The random identity IS a member
    assert!(check_group_membership(&store, &auto_group_id, &random_pk).unwrap());

    // But the node's identity is NOT a member — this is the bug
    assert!(!check_group_membership(&store, &auto_group_id, &node_pk).unwrap());
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
fn local_state_join_tracking_and_delete_group_rows_cleanup() {
    let store = test_store();
    let gid = ContextGroupId::from([0xC1; 32]);
    let context = ContextId::from([0xC2; 32]);
    let member = PublicKey::from([0xC3; 32]);
    let member2 = PublicKey::from([0xC4; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    set_default_capabilities(&store, &gid, 0b111).unwrap();
    set_subgroup_visibility(
        &store,
        &gid,
        calimero_context_config::VisibilityMode::Restricted,
    )
    .unwrap();
    set_group_alias(&store, &gid, "g-alias").unwrap();
    set_context_last_migration(&store, &gid, &context, "v2").unwrap();

    add_group_member(&store, &gid, &member, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member2, GroupMemberRole::Member).unwrap();
    set_member_alias(&store, &gid, &member2, "member2").unwrap();
    set_member_capability(&store, &gid, &member2, 0b010).unwrap();
    set_local_gov_nonce(&store, &gid, &member, 7).unwrap();

    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let signer_sk = PrivateKey::random(&mut rng);
    let op = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::Noop,
    )
    .unwrap();
    let op_bytes = borsh::to_vec(&op).unwrap();
    append_op_log_entry(&store, &gid, 1, &op_bytes).unwrap();
    set_op_head(&store, &gid, 1, vec![[0x11; 32]]).unwrap();
    track_member_context_join(&store, &gid, &member2, &context, [0xAA; 32]).unwrap();

    assert_eq!(get_local_gov_nonce(&store, &gid, &member).unwrap(), Some(7));
    assert_eq!(read_op_log_after(&store, &gid, 0, 10).unwrap().len(), 1);
    assert_eq!(
        get_member_context_joins(&store, &gid, &member2)
            .unwrap()
            .len(),
        1
    );

    delete_group_local_rows(&store, &gid).unwrap();

    assert!(load_group_meta(&store, &gid).unwrap().is_none());
    assert!(get_group_alias(&store, &gid).unwrap().is_none());
    assert!(get_default_capabilities(&store, &gid).unwrap().is_none());
    // Subgroup visibility falls back to Restricted when the row is absent
    // — that's how a successful delete is observed by the typed API.
    assert_eq!(
        get_subgroup_visibility(&store, &gid).unwrap(),
        calimero_context_config::VisibilityMode::Restricted
    );
    assert!(enumerate_member_capabilities(&store, &gid)
        .unwrap()
        .is_empty());
    assert!(enumerate_member_aliases(&store, &gid).unwrap().is_empty());
    assert!(get_context_last_migration(&store, &gid, &context)
        .unwrap()
        .is_none());
    assert!(get_local_gov_nonce(&store, &gid, &member)
        .unwrap()
        .is_none());
    assert!(get_op_head(&store, &gid).unwrap().is_none());
    assert!(read_op_log_after(&store, &gid, 0, 10).unwrap().is_empty());
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
        gid.to_bytes(),
        vec![],
        [0u8; 32],
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
        gid.to_bytes(),
        vec![],
        [0u8; 32],
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
        gid.to_bytes(),
        vec![],
        [0u8; 32],
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

fn append_tee_policy_op(store: &Store, group: &ContextGroupId, seq: u64, mrtd: &str) {
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let signer_sk = PrivateKey::random(&mut rng);
    let op = SignedGroupOp::sign(
        &signer_sk,
        group.to_bytes(),
        vec![],
        [0u8; 32],
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

    nest_group(&store, &root, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();
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

    nest_group(&store, &root, &child).unwrap();
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

    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    add_group_member(&store, &root, &admin_pk, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &child, &admin_pk, GroupMemberRole::Admin).unwrap();
    nest_group(&store, &root, &child).unwrap();

    let op = SignedGroupOp::sign(
        &admin_sk,
        child.to_bytes(),
        vec![],
        [0u8; 32],
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

    store_group_signing_key(&store, &gid, &pk, &sk).unwrap();

    let found = resolve_group_signing_key(&store, &gid, &pk).unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_walks_to_parent() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    nest_group(&store, &root, &child).unwrap();
    store_group_signing_key(&store, &root, &pk, &sk).unwrap();

    // Child should find root's key via parent walk
    let found = resolve_group_signing_key(&store, &child, &pk).unwrap();
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

    nest_group(&store, &root, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();
    store_group_signing_key(&store, &root, &pk, &sk).unwrap();

    // Grandchild walks upward: grandchild -> child -> root, finds root's key
    let found = resolve_group_signing_key(&store, &grandchild, &pk).unwrap();
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

    nest_group(&store, &root, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();

    store_group_signing_key(&store, &root, &pk, &root_sk).unwrap();
    store_group_signing_key(&store, &child, &pk, &child_sk).unwrap();

    // Grandchild should find child's key (nearest), not root's
    let found = resolve_group_signing_key(&store, &grandchild, &pk).unwrap();
    assert_eq!(found, Some(child_sk));

    // Child should find its own key
    let found = resolve_group_signing_key(&store, &child, &pk).unwrap();
    assert_eq!(found, Some(child_sk));
}

#[test]
fn resolve_signing_key_none_for_orphan() {
    let store = test_store();
    let orphan = ContextGroupId::from([0xD0; 32]);
    let pk = PublicKey::from([0x10; 32]);

    // No parent, no key stored anywhere
    let found = resolve_group_signing_key(&store, &orphan, &pk).unwrap();
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

    nest_group(&store, &root, &child).unwrap();
    store_group_signing_key(&store, &root, &admin, &sk).unwrap();

    // Different identity should not find the key
    let found = resolve_group_signing_key(&store, &child, &other).unwrap();
    assert_eq!(found, None);

    // Correct identity should find it
    let found = resolve_group_signing_key(&store, &child, &admin).unwrap();
    assert_eq!(found, Some(sk));
}

#[test]
fn resolve_signing_key_broken_by_unnest() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let pk = PublicKey::from([0x10; 32]);
    let sk = [0xAA; 32];

    nest_group(&store, &root, &child).unwrap();
    store_group_signing_key(&store, &root, &pk, &sk).unwrap();

    // Before unnest: child can find root's key
    assert_eq!(
        resolve_group_signing_key(&store, &child, &pk).unwrap(),
        Some(sk)
    );

    // Unnest breaks the parent link
    unnest_group(&store, &root, &child).unwrap();

    // After unnest: child can no longer walk to root
    assert_eq!(
        resolve_group_signing_key(&store, &child, &pk).unwrap(),
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

    nest_group(&store, &root, &child).unwrap();
    store_group_signing_key(&store, &root, &pk, &sk).unwrap();

    // Unnest
    unnest_group(&store, &root, &child).unwrap();
    assert_eq!(
        resolve_group_signing_key(&store, &child, &pk).unwrap(),
        None
    );

    // Re-nest: key should be reachable again
    nest_group(&store, &root, &child).unwrap();
    assert_eq!(
        resolve_group_signing_key(&store, &child, &pk).unwrap(),
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
        nest_group(&store, &groups[i], &groups[i + 1]).unwrap();
    }

    // Store key only on the root
    store_group_signing_key(&store, &groups[0], &pk, &sk).unwrap();

    // The deepest group (index MAX_NAMESPACE_DEPTH) is 16 levels below root.
    // The loop traverses MAX_NAMESPACE_DEPTH parent edges (matching
    // resolve_namespace), then does a final check on the reached group.
    // This means self + 16 edges + final check = covers the full chain.
    let at_boundary = resolve_group_signing_key(&store, &groups[MAX_NAMESPACE_DEPTH], &pk).unwrap();
    assert_eq!(
        at_boundary,
        Some(sk),
        "key at root should be reachable at exactly MAX_NAMESPACE_DEPTH"
    );

    // One level shallower should also find it
    let within_limit =
        resolve_group_signing_key(&store, &groups[MAX_NAMESPACE_DEPTH - 1], &pk).unwrap();
    assert_eq!(
        within_limit,
        Some(sk),
        "key should be reachable within depth limit"
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

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    // Admin passes
    assert!(require_group_admin(&store, &gid, &admin).is_ok());
    // Non-admin fails
    assert!(require_group_admin(&store, &gid, &member).is_err());
    // Unknown identity fails
    let unknown = PublicKey::from([0x03; 32]);
    assert!(require_group_admin(&store, &gid, &unknown).is_err());
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
    save_group_meta(&store, &root, &test_meta()).unwrap();
    add_group_member(&store, &root, &admin, GroupMemberRole::Admin).unwrap();
    store_group_signing_key(&store, &root, &admin, &sk).unwrap();

    // Set up child nested under root, with meta + admin but NO signing key
    save_group_meta(&store, &child, &test_meta()).unwrap();
    add_group_member(&store, &child, &admin, GroupMemberRole::Admin).unwrap();
    nest_group(&store, &root, &child).unwrap();

    // Verify: group exists, admin check passes, signing key resolves via parent
    assert!(load_group_meta(&store, &child).unwrap().is_some());
    assert!(require_group_admin(&store, &child, &admin).is_ok());
    let resolved = resolve_group_signing_key(&store, &child, &admin).unwrap();
    assert_eq!(resolved, Some(sk), "signing key should resolve from root");
}

#[test]
fn preflight_fails_when_no_signing_key_in_hierarchy() {
    let store = test_store();
    let gid = ContextGroupId::from([0xF0; 32]);
    let admin = PublicKey::from([0x01; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    // No signing key stored anywhere

    let resolved = resolve_group_signing_key(&store, &gid, &admin).unwrap();
    assert_eq!(resolved, None, "no signing key should be found");
}

#[test]
fn preflight_fails_for_nonexistent_group() {
    let store = test_store();
    let gid = ContextGroupId::from([0xF0; 32]);

    // Group doesn't exist — load_group_meta returns None
    assert!(load_group_meta(&store, &gid).unwrap().is_none());
}

// -----------------------------------------------------------------------
// recursive_remove_member — cascade removal through group hierarchy
// -----------------------------------------------------------------------

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

// -----------------------------------------------------------------------
// NamespaceGovernance::apply_signed_op — governance state machine tests
// -----------------------------------------------------------------------

#[test]
fn governance_group_reparented_via_signed_op() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::namespace_governance::NamespaceGovernance;

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
fn governance_rejects_non_admin_signer() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::namespace_governance::NamespaceGovernance;

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

    use super::namespace_governance::NamespaceGovernance;

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

    use super::namespace_governance::NamespaceGovernance;

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

    use super::namespace_governance::NamespaceGovernance;

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

    use super::namespace_governance::NamespaceGovernance;

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

// Helper: create a GroupMetaValue with a specific admin
fn sample_meta_with_admin(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        migration: None,
        auto_join: true,
    }
}

// ---------------------------------------------------------------------------
// Strict-tree refactor — orphan state is structurally impossible.
// See spec: docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md
// ---------------------------------------------------------------------------

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
    store_namespace_identity(&store, &ns_id, &ns_pk, &[0x22; 32], &[0x33; 32]).unwrap();

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
    store_namespace_identity(&store, &other_ns_id, &other_pk, &[0x66; 32], &[0x77; 32]).unwrap();
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

    save_group_meta(&store, &ns_id, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    save_group_meta(&store, &grandchild, &test_meta()).unwrap();
    nest_group(&store, &ns_id, &child).unwrap();
    nest_group(&store, &child, &grandchild).unwrap();

    let ctx_root = ContextId::from([0x10; 32]);
    let ctx_child = ContextId::from([0x11; 32]);
    let ctx_gc = ContextId::from([0x12; 32]);
    register_context_in_group(&store, &ns_id, &ctx_root).unwrap();
    register_context_in_group(&store, &child, &ctx_child).unwrap();
    register_context_in_group(&store, &grandchild, &ctx_gc).unwrap();

    let admin_pk = PublicKey::from([0xAA; 32]);
    add_group_member(&store, &ns_id, &admin_pk, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &child, &admin_pk, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &grandchild, &admin_pk, GroupMemberRole::Admin).unwrap();

    let ns_bytes = ns_id.to_bytes();
    store_namespace_identity(&store, &ns_id, &admin_pk, &[0x22; 32], &[0x33; 32]).unwrap();
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
    let payload = collect_subtree_for_cascade(&store, &ns_id).unwrap();
    let all = payload
        .descendant_groups
        .iter()
        .copied()
        .chain(std::iter::once(ns_id));
    for gid in all {
        for ctx in enumerate_group_contexts(&store, &gid, 0, usize::MAX).unwrap() {
            unregister_context_from_group(&store, &gid, &ctx).unwrap();
        }
        let parent = get_parent_group(&store, &gid).unwrap();
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
            load_group_meta(&store, &gid).unwrap().is_none(),
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
    assert!(get_parent_group(&store, &child).unwrap().is_none());
    assert!(get_parent_group(&store, &grandchild).unwrap().is_none());
    assert!(list_child_groups(&store, &ns_id).unwrap().is_empty());
    assert!(list_child_groups(&store, &child).unwrap().is_empty());

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

#[cfg(test)]
mod auto_follow_tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::group_store::{
        add_group_member, apply_local_signed_group_op, get_group_member_value,
    };

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
        add_group_member(&store, &gid, &admin_sk.public_key(), GroupMemberRole::Admin).unwrap();
        add_group_member(
            &store,
            &gid,
            &member_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();
        (store, gid, gid_bytes, admin_sk, member_sk)
    }

    #[test]
    fn admin_can_set_member_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = get_group_member_value(&store, &gid, &member_sk.public_key())
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
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = get_group_member_value(&store, &gid, &member_sk.public_key())
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
        add_group_member(
            &store,
            &gid,
            &other_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: other_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(err.to_string().contains("auto-follow"));

        // Sanity: the target's flags were not mutated.
        let val = get_group_member_value(&store, &gid, &other_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(!val.auto_follow.contexts);
        assert!(!val.auto_follow.subgroups);
    }

    #[test]
    fn rejects_non_member_target() {
        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _member_sk) = seed(&mut rng);
        let stranger = PrivateKey::random(&mut rng).public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: stranger,
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(err.to_string().contains("not a member"));
    }

    #[test]
    fn default_flags_are_false_and_preserved_on_role_change() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        // Initial state: flags default to false
        let before = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(!before.auto_follow.contexts);
        assert!(!before.auto_follow.subgroups);

        // Member turns on contexts
        let op1 = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
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
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberRoleSet {
                member: member_sk.public_key(),
                role: GroupMemberRole::ReadOnly,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op2).unwrap();

        let after = get_group_member_value(&store, &gid, &member_sk.public_key())
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
            gid_bytes,
            vec![],
            [0u8; 32],
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
        let value = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(value.auto_follow.contexts);
        assert!(value.auto_follow.subgroups);

        // 2. ContextRegistered op (admin registers a new context).
        let context_id = ContextId::from([0x77; 32]);
        let register = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
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
}

// -----------------------------------------------------------------------
// namespace_member_pubkeys — regression for ack-verify identity set
// -----------------------------------------------------------------------

/// Regression: the namespace creator/root admin is recorded only in
/// `GroupMeta::admin_identity` at namespace genesis (no self-`MemberJoined`
/// op). `namespace_member_pubkeys` must include that identity so that
/// `verify_ack` accepts legitimate acks signed by the namespace creator.
#[test]
fn namespace_member_pubkeys_includes_meta_admin_without_member_row() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let admin = PublicKey::from([0x01; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert!(
        pks.contains(&admin),
        "meta admin must appear in namespace_member_pubkeys even without a self-row"
    );
}

/// `namespace_member_pubkeys` must not duplicate the meta admin when
/// the admin also has a `GroupMember` row (e.g. an explicit `MemberJoined`
/// op was emitted for them).
#[test]
fn namespace_member_pubkeys_dedups_admin_with_member_row() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let admin = PublicKey::from([0x01; 32]);
    let other = PublicKey::from([0x02; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &other, GroupMemberRole::Member).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert_eq!(pks.iter().filter(|p| **p == admin).count(), 1);
    assert!(pks.contains(&other));
}

/// Members added via `add_group_member` continue to appear (no regression
/// from the meta-admin enrichment).
#[test]
fn namespace_member_pubkeys_includes_member_rows() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let m1 = PublicKey::from([0x10; 32]);
    let m2 = PublicKey::from([0x20; 32]);

    add_group_member(&store, &gid, &m1, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &m2, GroupMemberRole::Admin).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert!(pks.contains(&m1));
    assert!(pks.contains(&m2));
}
