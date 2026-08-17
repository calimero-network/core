//! The op-local admission predicates, exercised directly.
//!
//! The apply path runs the same functions, so a gap here is a gap there — and a
//! credential this half waved through while the apply path refused it would be a
//! device folded on one plane and absent from the other.

use calimero_account::{sign_device_cert, AccountGenesis, AccountId, DeviceId, KemPublicKey};
use calimero_primitives::identity::{PrivateKey, PublicKey};

use crate::join_credential_binds;
use crate::tests::support::test_join_account_for;

/// The shared predicate must refuse a credential that does not VERIFY, not
/// only one certified for the wrong key.
#[test]
fn the_shared_predicate_refuses_an_unverifiable_credential() {
    let sign_pk = PublicKey::from([7u8; 32]);

    // Certified for a real account, but the signature is filler, so
    // `verify_device_cert` refuses it.
    let filler = test_join_account_for(sign_pk);
    assert!(
        !join_credential_binds(
            &filler.cert.account,
            &filler.genesis,
            &filler.chain,
            &filler.cert
        ),
        "a certificate that does not verify is not admissible, whoever it names"
    );

    // A genuinely signed credential binds the account it certifies.
    let root_sk = PrivateKey::from([0x91; 32]);
    let genesis = AccountGenesis::new(root_sk.public_key());
    let account = genesis.account_id();
    let cert = sign_device_cert(
        &root_sk,
        account,
        DeviceId::from([0x3E; 32]),
        &sign_pk,
        &KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign cert");
    assert!(join_credential_binds(&account, &genesis, &[], &cert));

    // ...and binds nobody else's account. This is the ownership check now
    // that a join op names an account: a credential lifted from somebody
    // else's join certifies THEIR account and simply fails to match.
    assert!(!join_credential_binds(
        &AccountId::from([8u8; 32]),
        &genesis,
        &[],
        &cert
    ));
}
