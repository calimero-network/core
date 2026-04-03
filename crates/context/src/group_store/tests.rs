use super::*;

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
        use calimero_context_config::MemberCapabilities;
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
