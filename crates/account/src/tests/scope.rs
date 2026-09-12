//! A scope must be bound to one (account, device), ordered by its own epoch, and
//! provable from the account id alone.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{DeviceId, PrivateKey};

use super::support::{key, rotated};
use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::scope::{DeviceScope, SignedDeviceScope};
use crate::signed::AccountProof;

fn app(seed: u8) -> ApplicationId {
    ApplicationId::from([seed; 32])
}

#[test]
fn a_root_signed_scope_verifies_from_the_account_id_alone() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let scope = DeviceScope::sign(&root, account, device, vec![app(1)], 0, 0).expect("sign");
    let proof = SignedDeviceScope {
        genesis,
        chain: vec![],
        statement: scope,
    };

    let verified = proof.authorises(account, device).expect("verifies");
    assert_eq!(verified.applications, vec![app(1)]);
}

#[test]
fn a_scope_signed_by_a_stranger_is_refused() {
    let root = PrivateKey::from([7u8; 32]);
    let stranger = PrivateKey::from([9u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let forged = DeviceScope::sign(&stranger, account, device, vec![], 0, 0).expect("sign");
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: forged,
    };

    assert!(matches!(
        proof.authorises(account, device),
        Err(AccountError::ScopeSignatureInvalid)
    ));
}

#[test]
fn a_scope_cannot_be_replayed_onto_another_account_device_or_application_set() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    let other_device = DeviceId::mint(account, [0x23; 16]);

    let honest = DeviceScope::sign(&root, account, device, vec![app(1)], 0, 0).expect("sign");

    // The device is named in the payload AND checked before verification, so a
    // proof for one device can never authorise another.
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: honest.clone(),
    };
    assert!(matches!(
        proof.authorises(account, other_device),
        Err(AccountError::ScopeDeviceMismatch { .. })
    ));

    // A widened application list is a different payload.
    let mut widened = honest.clone();
    widened.applications.push(app(2));
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: widened,
    };
    assert!(
        matches!(
            proof.authorises(account, device),
            Err(AccountError::ScopeSignatureInvalid)
        ),
        "the applications are inside the signed payload"
    );

    // And it is bound to the account that minted it.
    let elsewhere = AccountGenesis::new(key(2).public_key());
    let proof = AccountProof {
        genesis: elsewhere,
        chain: vec![],
        statement: honest,
    };
    assert!(matches!(
        proof.authorises(elsewhere.account_id(), device),
        Err(AccountError::ScopeAccountMismatch)
    ));
}

#[test]
fn a_scope_epoch_is_part_of_what_is_signed() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    // Bumping the epoch on a signed statement must not be free: the registry
    // keeps the highest epoch, so a forgeable one would be a way to pin a scope.
    let mut scope = DeviceScope::sign(&root, account, device, vec![], 0, 0).expect("sign");
    scope.scope_epoch = 7;
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: scope,
    };

    assert!(matches!(
        proof.authorises(account, device),
        Err(AccountError::ScopeSignatureInvalid)
    ));
}

#[test]
fn the_new_root_may_also_scope_a_device() {
    let root = PrivateKey::from([7u8; 32]);
    let next = PrivateKey::from([8u8; 32]);
    let (genesis, handoff) = rotated(&root, &next);
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let scope = DeviceScope::sign(&next, account, device, vec![], 1, 1).expect("sign");
    let proof = AccountProof {
        genesis,
        chain: vec![handoff],
        statement: scope,
    };

    assert!(proof.authorises(account, device).is_ok());
}
