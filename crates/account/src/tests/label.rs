//! A label must be bound to one (account, device), ordered by its own epoch, and
//! provable from the account id alone.

use calimero_primitives::identity::{DeviceId, PrivateKey};

use super::support::key;
use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::label::{DeviceLabel, SignedDeviceLabel};
use crate::signed::AccountProof;

#[test]
fn a_root_signed_label_verifies_from_the_account_id_alone() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let label = DeviceLabel::sign(&root, account, device, "Work laptop".to_owned(), 3, 0)
        .expect("the account root signs its own device label");
    let proof = SignedDeviceLabel {
        genesis,
        chain: vec![],
        statement: label,
    };

    let verified = proof.authorises(account, device).expect("verifies");
    assert_eq!(verified.label, "Work laptop");
    assert_eq!(verified.label_epoch, 3);
}

#[test]
fn a_label_cannot_be_replayed_onto_another_account_device_text_or_epoch() {
    let root = PrivateKey::from([7u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    let other_device = DeviceId::mint(account, [0x23; 16]);

    let honest = DeviceLabel::sign(&root, account, device, "Phone".to_owned(), 1, 0).expect("sign");

    // Named in the payload AND checked before verification, so a proof for one
    // device can never be presented as another's.
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: honest.clone(),
    };
    assert!(matches!(
        proof.authorises(account, other_device),
        Err(AccountError::LabelDeviceMismatch { .. })
    ));

    // Rewriting the text is a different payload - the whole point, since the
    // text is the only thing this statement carries.
    let mut renamed = honest.clone();
    renamed.label = "Not your phone".to_owned();
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: renamed,
    };
    assert!(matches!(
        proof.authorises(account, device),
        Err(AccountError::LabelSignatureInvalid)
    ));

    // Bumping the epoch must not be free: the higher one wins, so a forgeable
    // epoch would be a way to pin a name nobody can replace.
    let mut bumped = honest.clone();
    bumped.label_epoch = 9;
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: bumped,
    };
    assert!(matches!(
        proof.authorises(account, device),
        Err(AccountError::LabelSignatureInvalid)
    ));

    // And it is bound to the account that minted it.
    let elsewhere = AccountGenesis::new(key(2).public_key());
    let proof = AccountProof {
        genesis: elsewhere,
        chain: vec![],
        statement: honest.clone(),
    };
    assert!(matches!(
        proof.authorises(elsewhere.account_id(), device),
        Err(AccountError::LabelAccountMismatch)
    ));

    // The root is the only key that can state any of it.
    let stranger = PrivateKey::from([9u8; 32]);
    let forged =
        DeviceLabel::sign(&stranger, account, device, "Phone".to_owned(), 1, 0).expect("sign");
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: forged,
    };
    assert!(matches!(
        proof.authorises(account, device),
        Err(AccountError::LabelSignatureInvalid)
    ));
}
