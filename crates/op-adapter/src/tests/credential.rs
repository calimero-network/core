//! The op-local admission predicates, exercised directly.
//!
//! The apply path runs the same functions, so a gap here is a gap there — and a
//! credential this half waved through while the apply path refused it would be a
//! device folded on one plane and absent from the other.

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey};
use calimero_primitives::identity::{PrivateKey, PublicKey};

use calimero_governance_types::JoinAccountCredential;

use crate::tests::support::{real_join_account_for, test_join_account_for};
use crate::{join_credential_binds, tee_admission_binding, tee_quote_binds_credential};

/// The shared predicate must refuse a credential that does not VERIFY, not
/// only one certified for the wrong key.
#[test]
fn the_shared_predicate_refuses_an_unverifiable_credential() {
    let sign_pk = PublicKey::from([7u8; 32]);

    // Certified for a real account, but the signature is filler, so
    // `verify_device_cert` refuses it.
    let filler = test_join_account_for(sign_pk);
    assert!(
        !join_credential_binds(&filler.statement.account, &filler),
        "a certificate that does not verify is not admissible, whoever it names"
    );

    // A genuinely signed credential binds the account it certifies.
    let root_sk = PrivateKey::from([0x91; 32]);
    let genesis = AccountGenesis::new(root_sk.public_key());
    let account = genesis.account_id();
    let cert = DeviceCert::sign(
        &root_sk,
        account,
        DeviceId::from([0x3E; 32]),
        &sign_pk,
        &KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign cert");
    let credential = JoinAccountCredential {
        genesis,
        chain: vec![],
        statement: cert,
    };
    assert!(join_credential_binds(&account, &credential));

    // ...and binds nobody else's account. This is the ownership check now
    // that a join op names an account: a credential lifted from somebody
    // else's join certifies THEIR account and simply fails to match.
    assert!(!join_credential_binds(
        &AccountId::from([8u8; 32]),
        &credential
    ));
}

/// A mock quote whose report data is `challenge` then the admission binding of
/// `credential` admitted as `member` into `group` of `namespace`.
fn admission_quote(
    challenge: &[u8; 32],
    namespace: &[u8; 32],
    group: &[u8; 32],
    member: &PublicKey,
    credential: &JoinAccountCredential,
) -> Vec<u8> {
    let binding = tee_admission_binding(namespace, group, member, credential);
    let report_data = calimero_tee_attestation::admission_report_data(challenge, &binding);
    calimero_tee_attestation::generate_mock_attestation(report_data).quote_bytes
}

/// A quote binds the credential it was made for, in the namespace and group it
/// was made for, and nothing else.
#[test]
fn a_quote_binds_only_the_credential_namespace_and_group_it_was_made_for() {
    let member = PublicKey::from([7u8; 32]);
    let credential = real_join_account_for(member, 0x11);
    let (namespace, group) = ([0xA1; 32], [0xA2; 32]);
    let quote = admission_quote(&[0x01; 32], &namespace, &group, &member, &credential);

    assert!(tee_quote_binds_credential(
        &namespace,
        &group,
        &member,
        &credential,
        &quote
    ));

    // Another namespace, another group, another identity key.
    assert!(!tee_quote_binds_credential(
        &[0xB1; 32],
        &group,
        &member,
        &credential,
        &quote
    ));
    assert!(!tee_quote_binds_credential(
        &namespace,
        &[0xB2; 32],
        &member,
        &credential,
        &quote
    ));
    assert!(!tee_quote_binds_credential(
        &namespace,
        &group,
        &PublicKey::from([8u8; 32]),
        &credential,
        &quote
    ));

    // The same account, key and device, certifying another delivery key, is
    // another credential.
    let root_sk = PrivateKey::from([0x11; 32]);
    let other_kem = {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            &root_sk,
            credential.statement.account,
            credential.statement.device,
            &member,
            &KemPublicKey::from([0x99; 32]),
            0,
            0,
        )
        .expect("the root signs its own device cert");
        JoinAccountCredential {
            genesis,
            chain: vec![],
            statement: cert,
        }
    };
    assert_eq!(other_kem.statement.account, credential.statement.account);
    assert!(!tee_quote_binds_credential(
        &namespace, &group, &member, &other_kem, &quote
    ));

    // Bytes that are not a quote bind nothing.
    assert!(!tee_quote_binds_credential(
        &namespace,
        &group,
        &member,
        &credential,
        &[0x42; 16]
    ));
}
