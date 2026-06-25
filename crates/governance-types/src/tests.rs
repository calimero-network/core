use super::*;

use calimero_primitives::identity::PrivateKey;
use rand::rngs::OsRng;

fn sample_group_id() -> [u8; 32] {
    let mut g = [0u8; 32];
    g[0] = 7;
    g[31] = 3;
    g
}

#[test]
fn sign_and_verify_round_trip() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
}

#[test]
fn wrong_key_fails() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let other = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

    let mut op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Admin,
        },
    )
    .expect("sign");

    // Swap signer to another key without re-signing
    op.signer = other.public_key();

    assert!(op.verify_signature().is_err());
}

#[test]
fn tampered_op_fails() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

    let mut op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    op.nonce = 2;
    assert!(op.verify_signature().is_err());
}

#[test]
fn replay_distinct_content_hash() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

    let op1 = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    let op2 = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        2,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    let h1 = op1.content_hash().expect("hash");
    let h2 = op2.content_hash().expect("hash");
    assert_ne!(
        h1, h2,
        "different nonces must yield different content hashes"
    );
}

#[test]
fn signable_bytes_deterministic() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let pk = sk.public_key();
    let s = SignableGroupOp {
        version: SIGNED_GROUP_OP_SCHEMA_VERSION,
        group_id: [1u8; 32],
        parent_op_hashes: vec![],
        signer: pk,
        nonce: 42,
        op: GroupOp::Noop,
    };
    let a = signable_bytes(&s).expect("bytes");
    let b = signable_bytes(&s).expect("bytes");
    assert_eq!(a, b);
    assert!(a.starts_with(GROUP_GOVERNANCE_SIGN_DOMAIN));
}

// --- Namespace op tests ---

fn sample_namespace_id() -> [u8; 32] {
    let mut ns = [0u8; 32];
    ns[0] = 0xAA;
    ns[31] = 0xBB;
    ns
}

#[test]
fn namespace_op_sign_verify_root() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id(),
            restricted: true,
        }),
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert!(op.group_id().is_none());
}

#[test]
fn namespace_op_sign_verify_group() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let encrypted = EncryptedGroupOp {
        nonce: [42u8; 12],
        ciphertext: vec![1, 2, 3, 4],
    };

    let op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Group {
            group_id: sample_group_id(),
            key_id: [0u8; 32],
            encrypted,
            key_rotation: None,
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(op.group_id(), Some(sample_group_id()));
}

#[test]
fn namespace_op_tampered_fails() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let mut op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: sk.public_key(),
        }),
    )
    .expect("sign");

    op.nonce = 999;
    assert!(op.verify_signature().is_err());
}

#[test]
fn namespace_op_content_hash_distinct() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op1 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id(),
            restricted: true,
        }),
    )
    .expect("sign");

    let op2 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id(),
            restricted: true,
        }),
    )
    .expect("sign");

    assert_ne!(
        op1.content_hash().unwrap(),
        op2.content_hash().unwrap(),
        "different nonces must yield different content hashes"
    );
}

#[test]
fn namespace_signable_bytes_deterministic() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let pk = sk.public_key();
    let s = SignableNamespaceOp {
        version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
        namespace_id: sample_namespace_id(),
        parent_op_hashes: vec![],
        signer: pk,
        nonce: 42,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id(),
            restricted: true,
        }),
    };
    let a = namespace_signable_bytes(&s).expect("bytes");
    let b = namespace_signable_bytes(&s).expect("bytes");
    assert_eq!(a, b);
    assert!(a.starts_with(NAMESPACE_GOVERNANCE_SIGN_DOMAIN));
}

// --- Cascade op variants (Option C in cascade design doc) ---

fn sample_application_id(seed: u8) -> ApplicationId {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[31] = !seed;
    ApplicationId::from(bytes)
}

#[test]
fn cascade_target_application_set_sign_verify() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32],
            app_key: [10u8; 32],
            target_application_id: sample_application_id(0x42),
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(
        op.op.op_kind_label(),
        "cascade_target_application_set",
        "op_kind_label must distinguish cascade variant for metrics"
    );
}

#[test]
fn cascade_group_migration_set_sign_verify() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeGroupMigrationSet {
            from_app_key: [9u8; 32],
            migration: Some(b"migrate_v1_to_v2".to_vec()),
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(
        op.op.op_kind_label(),
        "cascade_group_migration_set",
        "op_kind_label must distinguish cascade migration variant for metrics"
    );
}

#[test]
fn cascade_target_distinct_from_single_group_target() {
    // A cascade op and a non-cascade op with the same new app_key/target
    // must produce DIFFERENT content hashes -- otherwise replay/dedup
    // would conflate the two distinct governance intents.
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let new_app_key = [11u8; 32];
    let target = sample_application_id(0x77);

    let single = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::TargetApplicationSet {
            app_key: new_app_key,
            target_application_id: target,
        },
    )
    .expect("sign");

    let cascade = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32],
            app_key: new_app_key,
            target_application_id: target,
        },
    )
    .expect("sign");

    assert_ne!(
        single.content_hash().expect("hash single"),
        cascade.content_hash().expect("hash cascade"),
        "cascade and single-group target ops must hash distinctly"
    );
}

#[test]
fn cascade_target_from_app_key_changes_hash() {
    // The Borsh-discriminant guarantees distinctness from the
    // single-group variant (covered by
    // `cascade_target_distinct_from_single_group_target`). This test
    // covers the stronger invariant: `from_app_key` is itself part of
    // the signed bytes, so two cascade ops that agree on every field
    // EXCEPT `from_app_key` must still hash differently. Otherwise a
    // refactor that accidentally collapses `from_app_key` (e.g. by
    // defaulting it or excluding it from signable bytes) would silently
    // break dedup of intent-different cascades.
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let new_app_key = [11u8; 32];
    let target = sample_application_id(0x77);

    let a = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32],
            app_key: new_app_key,
            target_application_id: target,
        },
    )
    .expect("sign");

    let b = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [8u8; 32], // only this differs
            app_key: new_app_key,
            target_application_id: target,
        },
    )
    .expect("sign");

    assert_ne!(
        a.content_hash().expect("hash a"),
        b.content_hash().expect("hash b"),
        "from_app_key must be covered by the signed content hash"
    );
}

#[test]
fn cascade_target_application_set_borsh_round_trip() {
    // Explicit wire-format round-trip for the new variant. The
    // sign/verify tests above implicitly exercise serialization (sign
    // hashes the Borsh bytes; verify rebuilds them) but do not assert
    // that field values survive a standalone serialize -> deserialize
    // round trip on the GroupOp itself. A future enum reordering that
    // shifts variant tags would silently change which variant a stored
    // op decodes as; this guards against that by asserting field
    // equality after a round trip.
    let original = GroupOp::CascadeTargetApplicationSet {
        from_app_key: [9u8; 32],
        app_key: [10u8; 32],
        target_application_id: sample_application_id(0x42),
    };

    let bytes = borsh::to_vec(&original).expect("serialize");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("deserialize");

    match decoded {
        GroupOp::CascadeTargetApplicationSet {
            from_app_key,
            app_key,
            target_application_id,
        } => {
            assert_eq!(from_app_key, [9u8; 32]);
            assert_eq!(app_key, [10u8; 32]);
            assert_eq!(target_application_id, sample_application_id(0x42));
        }
        other => panic!("expected CascadeTargetApplicationSet, got {other:?}"),
    }
}

#[test]
fn cascade_group_migration_set_borsh_round_trip() {
    // Symmetric round-trip guard for the migration variant.
    let original = GroupOp::CascadeGroupMigrationSet {
        from_app_key: [9u8; 32],
        migration: Some(b"migrate_v1_to_v2".to_vec()),
    };

    let bytes = borsh::to_vec(&original).expect("serialize");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("deserialize");

    match decoded {
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => {
            assert_eq!(from_app_key, [9u8; 32]);
            assert_eq!(migration.as_deref(), Some(b"migrate_v1_to_v2".as_ref()));
        }
        other => panic!("expected CascadeGroupMigrationSet, got {other:?}"),
    }

    // Also cover migration = None.
    let original_none = GroupOp::CascadeGroupMigrationSet {
        from_app_key: [0u8; 32],
        migration: None,
    };
    let bytes_none = borsh::to_vec(&original_none).expect("serialize none");
    let decoded_none: GroupOp = borsh::from_slice(&bytes_none).expect("deserialize none");
    match decoded_none {
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => {
            assert_eq!(from_app_key, [0u8; 32]);
            assert!(migration.is_none());
        }
        other => panic!("expected CascadeGroupMigrationSet, got {other:?}"),
    }
}

// --- CascadeUpgrade wire-format back-compat (schema v7) ---

#[test]
fn cascade_upgrade_back_compat_discriminant_fixed() {
    // `CascadeUpgrade` is the LAST variant of `GroupOp`, so its Borsh
    // discriminant must stay fixed at ordinal 25. This is a GOLDEN
    // byte-vector guard: the bytes below were produced by the enum at the
    // v7 layout (CascadeUpgrade at ordinal 25, its leading discriminant
    // byte). We decode these EXTERNALLY-FIXED bytes with the CURRENT enum —
    // we never re-encode them here. A same-binary serialize -> deserialize
    // round-trip would NOT catch a mid-enum insertion, because both sides
    // would use the shifted layout and still agree. Decoding frozen bytes is
    // what actually catches it: insert a variant in the MIDDLE of `GroupOp`
    // and CascadeUpgrade's ordinal shifts off 25, so byte `25` here decodes
    // as a DIFFERENT variant (or fails).
    //
    // Golden encoding of:
    //   GroupOp::CascadeUpgrade {
    //       from_app_key: [3u8; 32],
    //       app_key: [4u8; 32],
    //       target_application_id: sample_application_id(5),
    //       migration: Some(b"migrate".to_vec()),
    //       cascade_hlc: HybridTimestamp::zero(),
    //   }
    const GOLDEN_CASCADE_UPGRADE: &[u8] = &[
        25, // <- CascadeUpgrade's fixed Borsh discriminant (ordinal 25)
        3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
        3, 3, // from_app_key
        4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, // app_key
        5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 250, // target_application_id = sample_application_id(5)
        1, 7, 0, 0, 0, 109, 105, 103, 114, 97, 116, 101, // migration = Some("migrate")
        0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, // cascade_hlc = HybridTimestamp::zero()
    ];

    // Up-front: the leading discriminant byte must equal CascadeUpgrade's
    // known ordinal, so a mid-enum insertion (which shifts it) is caught.
    assert_eq!(
        GOLDEN_CASCADE_UPGRADE[0], 25,
        "CascadeUpgrade's Borsh discriminant must stay at ordinal 25; a \
         changed leading byte means a prior variant moved"
    );

    let decoded: GroupOp =
        borsh::from_slice(GOLDEN_CASCADE_UPGRADE).expect("decode frozen CascadeUpgrade bytes");
    match decoded {
        GroupOp::CascadeUpgrade {
            from_app_key,
            app_key,
            target_application_id,
            migration,
            cascade_hlc,
        } => {
            assert_eq!(from_app_key, [3u8; 32]);
            assert_eq!(app_key, [4u8; 32]);
            assert_eq!(target_application_id, sample_application_id(5));
            assert_eq!(migration, Some(b"migrate".to_vec()));
            assert_eq!(cascade_hlc, HybridTimestamp::zero());
        }
        other => panic!(
            "frozen CascadeUpgrade bytes (discriminant 25) decoded as {other:?}; a \
             variant was inserted mid-enum, shifting prior variant tags"
        ),
    }
}
