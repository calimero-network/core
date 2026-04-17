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

    assert!(settings.set_default_visibility(&member, 1).is_err());
    assert!(settings.set_default_visibility(&admin, 2).is_err());
    settings.set_default_visibility(&admin, 1).unwrap();
    assert_eq!(get_default_visibility(&store, &gid).unwrap(), Some(1));

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
    assert_eq!(caps & (1 << 2), 0); // CAN_JOIN_OPEN_CONTEXTS
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
fn set_and_get_default_visibility() {
    let store = test_store();
    let gid = test_group_id();

    assert!(get_default_visibility(&store, &gid).unwrap().is_none());

    // Open = 0
    set_default_visibility(&store, &gid, 0).unwrap();
    assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 0);

    // Restricted = 1
    set_default_visibility(&store, &gid, 1).unwrap();
    assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 1);
}

#[test]
fn defaults_isolated_per_group() {
    let store = test_store();
    let g1 = ContextGroupId::from([0x40; 32]);
    let g2 = ContextGroupId::from([0x41; 32]);

    set_default_capabilities(&store, &g1, 0b001).unwrap();
    set_default_capabilities(&store, &g2, 0b110).unwrap();
    set_default_visibility(&store, &g1, 0).unwrap();
    set_default_visibility(&store, &g2, 1).unwrap();

    assert_eq!(
        get_default_capabilities(&store, &g1).unwrap().unwrap(),
        0b001
    );
    assert_eq!(
        get_default_capabilities(&store, &g2).unwrap().unwrap(),
        0b110
    );
    assert_eq!(get_default_visibility(&store, &g1).unwrap().unwrap(), 0);
    assert_eq!(get_default_visibility(&store, &g2).unwrap().unwrap(), 1);
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

    set_default_capabilities(&store, &gid, 0b101).unwrap();
    set_default_visibility(&store, &gid, 1).unwrap();
    set_member_capability(&store, &gid, &alice, 0b001).unwrap();
    set_member_capability(&store, &gid, &bob, 0b010).unwrap();
    assert_eq!(
        enumerate_member_capabilities(&store, &gid).unwrap().len(),
        2
    );

    delete_default_capabilities(&store, &gid).unwrap();
    delete_default_visibility(&store, &gid).unwrap();
    delete_all_member_capabilities(&store, &gid).unwrap();

    assert!(get_default_capabilities(&store, &gid).unwrap().is_none());
    assert!(get_default_visibility(&store, &gid).unwrap().is_none());
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
    set_default_visibility(&store, &gid, 1).unwrap();
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
    assert!(get_default_visibility(&store, &gid).unwrap().is_none());
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
    // resolve_group_signing_key checks self + walks up to MAX_NAMESPACE_DEPTH
    // ancestors, but the root is exactly at the boundary. It should still be
    // reachable since the loop runs MAX_NAMESPACE_DEPTH iterations checking
    // self first, then walking up.
    let at_boundary = resolve_group_signing_key(&store, &groups[MAX_NAMESPACE_DEPTH], &pk).unwrap();
    // At depth 16 from root: self (depth 16) + walk 15 ancestors = checks
    // depths 16,15,14,...,1 but NOT depth 0 (root). So the key is missed.
    assert_eq!(
        at_boundary, None,
        "key at root should be unreachable at max depth"
    );

    // One level shallower should still find it
    let within_limit =
        resolve_group_signing_key(&store, &groups[MAX_NAMESPACE_DEPTH - 1], &pk).unwrap();
    assert_eq!(
        within_limit,
        Some(sk),
        "key should be reachable within depth limit"
    );
}
