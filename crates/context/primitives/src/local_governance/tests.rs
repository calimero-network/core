use super::*;

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
        [0u8; 32],
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
        [0u8; 32],
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
        [0u8; 32],
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
        [0u8; 32],
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
        [0u8; 32],
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
        state_hash: [0u8; 32],
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
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
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
        [0u8; 32],
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
        [0u8; 32],
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
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
        }),
    )
    .expect("sign");

    let op2 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
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
        state_hash: [0u8; 32],
        signer: pk,
        nonce: 42,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
        }),
    };
    let a = namespace_signable_bytes(&s).expect("bytes");
    let b = namespace_signable_bytes(&s).expect("bytes");
    assert_eq!(a, b);
    assert!(a.starts_with(NAMESPACE_GOVERNANCE_SIGN_DOMAIN));
}
