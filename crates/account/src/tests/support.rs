//! Fixtures shared by the test files beside this one.
//!
//! `sign_handoff` and `sign_cert` assemble their preimages through the same
//! `signing_payload` the verifier uses, but without going through the public
//! minters — so a test can mint a credential the minters would refuse (a handoff
//! signed by the wrong key, a cert at an unreachable epoch) and check that
//! verification is what catches it.

use calimero_primitives::identity::{AccountId, DeviceId, PrivateKey};

use crate::account::AccountGenesis;
use crate::device::{DeviceCert, KemPublicKey};
use crate::root_key::RootKeyHandoff;

/// Deterministic keypair, so failures reproduce exactly.
pub(crate) fn key(seed: u8) -> PrivateKey {
    PrivateKey::from([seed; 32])
}

pub(crate) fn genesis_for(root: &PrivateKey) -> AccountGenesis {
    AccountGenesis::new(root.public_key())
}

/// An account rooted at `root`, plus a handoff rolling it onto `next`.
pub(crate) fn rotated(root: &PrivateKey, next: &PrivateKey) -> (AccountGenesis, RootKeyHandoff) {
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let payload = RootKeyHandoff::signing_payload(account, 0, &next.public_key());
    let handoff = RootKeyHandoff {
        account,
        from_epoch: 0,
        new_root_sign_pk: next.public_key(),
        signature: root.sign(&payload).expect("sign").to_bytes(),
    };
    (genesis, handoff)
}

/// The account, device and honest key material a pairing produces.
pub(crate) fn pairing_fixture() -> (AccountId, DeviceId, PrivateKey, KemPublicKey) {
    let root = PrivateKey::from([7u8; 32]);
    let account = AccountGenesis::new(root.public_key()).account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    (
        account,
        device,
        PrivateKey::from([9u8; 32]),
        KemPublicKey::from([0x33; 32]),
    )
}

pub(crate) fn sign_handoff(
    signer: &PrivateKey,
    account: AccountId,
    from_epoch: u32,
    new_root: &PrivateKey,
) -> RootKeyHandoff {
    let new_root_sign_pk = new_root.public_key();
    let payload = RootKeyHandoff::signing_payload(account, from_epoch, &new_root_sign_pk);
    RootKeyHandoff {
        account,
        from_epoch,
        new_root_sign_pk,
        signature: signer.sign(&payload).expect("sign").to_bytes(),
    }
}

pub(crate) fn sign_cert(
    signer: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    device_sign: &PrivateKey,
    key_epoch: u32,
    device_epoch: u32,
) -> DeviceCert {
    let sign_pk = device_sign.public_key();
    let kem_pk = KemPublicKey::from([9u8; 32]);
    let payload =
        DeviceCert::signing_payload(account, device, &sign_pk, &kem_pk, key_epoch, device_epoch);
    DeviceCert {
        account,
        device,
        sign_pk,
        kem_pk,
        key_epoch,
        device_epoch,
        signature: signer.sign(&payload).expect("sign").to_bytes(),
    }
}
