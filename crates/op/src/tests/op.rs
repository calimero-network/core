//! The envelope: what the id covers, and what a signature is worth.

use calimero_account::AccountId;
use calimero_storage::address::Id;

use crate::tests::support::{hlc0, key, real_authorship};
use crate::{Op, OpPayload, ScopeId};

#[test]
fn compute_id_is_parent_order_invariant() {
    let scope = ScopeId::from([7u8; 32]);
    let author = real_authorship(1, 2);
    let hlc = hlc0();
    let payload = OpPayload::Delete {
        entity: Id::new([2u8; 32]),
    };
    let a = Op::compute_id(scope, &[[3u8; 32], [4u8; 32]], &author, &hlc, &payload);
    let b = Op::compute_id(scope, &[[4u8; 32], [3u8; 32]], &author, &hlc, &payload);
    assert_eq!(a, b, "id must not depend on parent ordering");
}

#[test]
fn compute_id_distinguishes_payload() {
    let scope = ScopeId::from([7u8; 32]);
    let author = real_authorship(1, 2);
    let hlc = hlc0();
    let put = OpPayload::Put {
        entity: Id::new([2u8; 32]),
        value: vec![1, 2, 3],
    };
    let del = OpPayload::Delete {
        entity: Id::new([2u8; 32]),
    };
    assert_ne!(
        Op::compute_id(scope, &[], &author, &hlc, &put),
        Op::compute_id(scope, &[], &author, &hlc, &del),
    );
}

#[test]
fn verify_checks_the_signature_against_the_device_key() {
    // An account has no key of its own; the device key is what signs.
    let device_sk = key(2);
    let authorship = real_authorship(1, 2);
    let scope = ScopeId::from([7u8; 32]);
    let payload = OpPayload::Put {
        entity: Id::new([2u8; 32]),
        value: vec![1],
    };
    let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
    let op = Op::new(
        scope,
        vec![],
        authorship,
        hlc0(),
        payload,
        [0u8; 32],
        device_sk.sign(&id).expect("sign").to_bytes(),
    );
    assert!(op.verify());
    assert_eq!(op.author(), authorship.account);
    assert_eq!(op.device(), authorship.device);
    assert_eq!(*op.device_key(), device_sk.public_key());
}

#[test]
fn verify_rejects_an_op_signed_by_a_different_device_key() {
    let authorship = real_authorship(1, 2);
    let scope = ScopeId::from([7u8; 32]);
    let payload = OpPayload::Put {
        entity: Id::new([2u8; 32]),
        value: vec![1],
    };
    let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
    // Signed by key 9 while claiming device_key of key 2.
    let op = Op::new(
        scope,
        vec![],
        authorship,
        hlc0(),
        payload,
        [0u8; 32],
        key(9).sign(&id).expect("sign").to_bytes(),
    );
    assert!(!op.verify());
}

#[test]
fn verify_rejects_a_swapped_account_after_signing() {
    // The account is in the id preimage, so re-pointing a validly signed op
    // at another account breaks the id/content match before the signature
    // check is even reached.
    let device_sk = key(2);
    let authorship = real_authorship(1, 2);
    let scope = ScopeId::from([7u8; 32]);
    let payload = OpPayload::Put {
        entity: Id::new([2u8; 32]),
        value: vec![1],
    };
    let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
    let mut op = Op::new(
        scope,
        vec![],
        authorship,
        hlc0(),
        payload,
        [0u8; 32],
        device_sk.sign(&id).expect("sign").to_bytes(),
    );
    assert!(op.verify());
    op.authorship.account = AccountId::from([99u8; 32]);
    assert!(!op.verify());
}
