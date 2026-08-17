//! Fixtures shared by the test files beside this one.

use core::num::NonZeroU128;

use calimero_account::{
    sign_device_cert, AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey,
};
use calimero_governance_types::JoinAccountCredential;
use calimero_op::Authorship;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

/// The [`Authorship`] for an op whose author is named directly.
///
/// An unattributed authorship names no account at all,
/// which couples a test to the stand-in bridge even when all it wanted was "some
/// author". This states the principal and the key separately, which is what the
/// two fields actually mean.
pub(crate) fn authorship_of(account: AccountId, device_key: PublicKey) -> Authorship {
    Authorship {
        account,
        device: DeviceId::from(*account.as_bytes()),
        device_key,
    }
}

pub(crate) fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

/// A credential that actually VERIFIES for `sign_pk`. The filler fixture below
/// cannot fold a device now that the shared predicate checks the certificate, so
/// any test asserting the device half needs this one.
pub(crate) fn real_join_account_for(sign_pk: PublicKey, seed: u8) -> Box<JoinAccountCredential> {
    let root_sk = PrivateKey::from([seed; 32]);
    let genesis = AccountGenesis::new(root_sk.public_key());
    let cert = sign_device_cert(
        &root_sk,
        genesis.account_id(),
        DeviceId::from([0x3E; 32]),
        &sign_pk,
        &KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign the device cert");
    Box::new(JoinAccountCredential {
        genesis,
        chain: vec![],
        cert,
    })
}

/// A joiner credential whose signature is filler, so `verify_device_cert`
/// refuses it. Tests that assert a credential folds NO device use this: it is
/// present and well-shaped, and still not admissible.
pub(crate) fn test_join_account_for(sign_pk: PublicKey) -> Box<JoinAccountCredential> {
    let root = PublicKey::from([0x7A; 32]);
    let genesis = AccountGenesis::new(root);
    Box::new(JoinAccountCredential {
        cert: DeviceCert {
            account: genesis.account_id(),
            device: DeviceId::from([0x3E; 32]),
            sign_pk,
            kem_pk: KemPublicKey::from([0x2B; 32]),
            key_epoch: 0,
            device_epoch: 0,
            signature: [0x11; 64],
        },
        genesis,
        chain: vec![],
    })
}
