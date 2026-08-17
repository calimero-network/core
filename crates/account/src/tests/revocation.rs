//! A revocation must be terminal, bound to one (account, device), and provable
//! from the account id alone.

use calimero_primitives::identity::{DeviceId, PrivateKey};

use super::support::{key, rotated};
use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::revocation::{sign_device_revocation, verify_device_revocation};

#[test]
fn a_root_signed_revocation_verifies_from_the_account_id_alone() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let revocation = sign_device_revocation(&root, account, device, 0).expect("sign");
    assert!(verify_device_revocation(account, &genesis, &[], &revocation).is_ok());
}

#[test]
fn a_revocation_survives_a_later_root_key_rotation() {
    // The asymmetry with `verify_device_cert`, and the reason it exists.
    // Superseded epochs are filtered for certificates when the view is read;
    // applying that rule here would mean rotating the root silently
    // UN-revokes every device the account had withdrawn. Revocation is
    // terminal by design — a spent DeviceId must never come back.
    let root = PrivateKey::from([7u8; 32]);
    let next = PrivateKey::from([8u8; 32]);
    let (genesis, handoff) = rotated(&root, &next);
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    // Signed by the OLD root, before the rotation.
    let revocation = sign_device_revocation(&root, account, device, 0).expect("sign");

    assert!(
        verify_device_revocation(account, &genesis, &[handoff], &revocation).is_ok(),
        "a rotation must not resurrect a revoked device"
    );
}

#[test]
fn the_new_root_may_also_revoke() {
    let root = PrivateKey::from([7u8; 32]);
    let next = PrivateKey::from([8u8; 32]);
    let (genesis, handoff) = rotated(&root, &next);
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let revocation = sign_device_revocation(&next, account, device, 1).expect("sign");
    assert!(verify_device_revocation(account, &genesis, &[handoff], &revocation).is_ok());
}

#[test]
fn a_revocation_signed_by_a_stranger_is_refused() {
    // The whole point of the proof: without it, "may this signer revoke" would
    // have to be answered from folded state, and two replicas would disagree.
    let root = PrivateKey::from([7u8; 32]);
    let stranger = PrivateKey::from([9u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let forged = sign_device_revocation(&stranger, account, device, 0).expect("sign");
    assert!(matches!(
        verify_device_revocation(account, &genesis, &[], &forged),
        Err(AccountError::RevocationSignatureInvalid)
    ));
}

#[test]
fn a_revocation_cannot_be_replayed_onto_another_device_or_account() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    let other_device = DeviceId::mint(account, [0x23; 16]);

    let mut revocation = sign_device_revocation(&root, account, device, 0).expect("sign");
    revocation.device = other_device;
    assert!(
        matches!(
            verify_device_revocation(account, &genesis, &[], &revocation),
            Err(AccountError::RevocationSignatureInvalid)
        ),
        "the device is inside the signed payload"
    );

    let elsewhere = AccountGenesis::new(key(2).public_key());
    let honest = sign_device_revocation(&root, account, device, 0).expect("sign");
    assert!(
        matches!(
            verify_device_revocation(elsewhere.account_id(), &elsewhere, &[], &honest),
            Err(AccountError::RevocationAccountMismatch)
        ),
        "a revocation is bound to the account that minted it"
    );
}

#[test]
fn a_revocation_claiming_an_unreachable_epoch_is_refused() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let revocation = sign_device_revocation(&root, account, device, 5).expect("sign");
    assert!(matches!(
        verify_device_revocation(account, &genesis, &[], &revocation),
        Err(AccountError::EpochOutOfRange { .. })
    ));
}
