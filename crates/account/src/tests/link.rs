//! An account link must be provable from the account id alone, bound to the one
//! verifier it was minted for, and impossible to forge out of a device
//! certificate.

use calimero_primitives::identity::{DeviceId, PrivateKey};

use super::support::{key, rotated, sign_cert};
use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::link::{verify_account_link, AccountLink};
use crate::login::Audience;

const CHALLENGE: [u8; 32] = [0x5C; 32];
const ISSUED: u64 = 1_700_000_000;
const EXPIRES: u64 = 1_700_000_300;

fn cloud() -> Audience {
    Audience::WebOrigin("https://cloud.calimero.network".to_owned())
}

/// Mint a link for the account `root` owns, at epoch 0.
fn link_from(root: &PrivateKey) -> (AccountGenesis, AccountLink) {
    let genesis = AccountGenesis::new(root.public_key());
    let link = AccountLink::sign(
        root,
        genesis.account_id(),
        cloud(),
        CHALLENGE,
        ISSUED,
        EXPIRES,
        0,
    )
    .expect("sign");
    (genesis, link)
}

#[test]
fn a_root_signed_link_verifies_from_the_account_id_alone() {
    // The property the whole credential exists for: a verifier holding nothing
    // but the account id can check this. No node, no membership, no cut.
    let root = key(7);
    let (genesis, link) = link_from(&root);

    assert!(verify_account_link(genesis.account_id(), &genesis, &[], &link).is_ok());
}

#[test]
fn a_link_signed_by_a_stranger_is_refused() {
    let root = key(7);
    let stranger = key(8);
    let genesis = AccountGenesis::new(root.public_key());

    // The stranger signs a link naming somebody else's account.
    let forged = AccountLink::sign(
        &stranger,
        genesis.account_id(),
        cloud(),
        CHALLENGE,
        ISSUED,
        EXPIRES,
        0,
    )
    .expect("sign");

    assert!(matches!(
        verify_account_link(genesis.account_id(), &genesis, &[], &forged),
        Err(AccountError::LinkSignatureInvalid),
    ));
}

#[test]
fn a_link_for_one_account_does_not_verify_as_another() {
    let root = key(7);
    let (_genesis, link) = link_from(&root);
    let other = AccountGenesis::new(key(8).public_key());

    assert!(matches!(
        verify_account_link(other.account_id(), &other, &[], &link),
        Err(AccountError::LinkAccountMismatch),
    ));
}

/// The field that stops one verifier filing a link minted for another.
#[test]
fn the_audience_is_covered_by_the_signature() {
    let root = key(7);
    let (genesis, link) = link_from(&root);

    let mut elsewhere = link.clone();
    elsewhere.audience = Audience::WebOrigin("https://attacker.example".to_owned());

    assert!(
        matches!(
            verify_account_link(genesis.account_id(), &genesis, &[], &elsewhere),
            Err(AccountError::LinkSignatureInvalid),
        ),
        "re-pointing a link at another verifier must not survive verification",
    );
}

/// The variant tag, not just the string. Without it a link minted for the web
/// origin `"x"` would verify as one minted for the code-signing id `"x"`.
#[test]
fn the_audience_variant_is_covered_too() {
    let root = key(7);
    let genesis = AccountGenesis::new(root.public_key());
    let name = "https://cloud.calimero.network".to_owned();

    let as_origin = AccountLink::sign(
        &root,
        genesis.account_id(),
        Audience::WebOrigin(name.clone()),
        CHALLENGE,
        ISSUED,
        EXPIRES,
        0,
    )
    .expect("sign");

    let mut as_code_signing = as_origin.clone();
    as_code_signing.audience = Audience::CodeSigningId(name);

    assert!(matches!(
        verify_account_link(genesis.account_id(), &genesis, &[], &as_code_signing),
        Err(AccountError::LinkSignatureInvalid),
    ));
}

#[test]
fn the_challenge_is_covered_by_the_signature() {
    let root = key(7);
    let (genesis, link) = link_from(&root);

    let mut replayed = link.clone();
    replayed.challenge = [0xAA; 32];

    assert!(
        matches!(
            verify_account_link(genesis.account_id(), &genesis, &[], &replayed),
            Err(AccountError::LinkSignatureInvalid),
        ),
        "a link must not be re-pointed at a fresh challenge",
    );
}

#[test]
fn the_validity_window_is_covered_by_the_signature() {
    let root = key(7);
    let (genesis, link) = link_from(&root);

    let mut extended = link.clone();
    extended.expires_at = EXPIRES + 86_400;

    assert!(
        matches!(
            verify_account_link(genesis.account_id(), &genesis, &[], &extended),
            Err(AccountError::LinkSignatureInvalid),
        ),
        "an expiry a holder can extend is no expiry at all",
    );
}

#[test]
fn the_rotated_root_may_link_at_its_own_epoch() {
    let root = key(7);
    let next = key(8);
    let (genesis, handoff) = rotated(&root, &next);

    let link = AccountLink::sign(
        &next,
        genesis.account_id(),
        cloud(),
        CHALLENGE,
        ISSUED,
        EXPIRES,
        1,
    )
    .expect("sign");

    assert!(verify_account_link(genesis.account_id(), &genesis, &[handoff], &link).is_ok());
}

/// The epoch is in the preimage, so a signature made at one epoch cannot be
/// re-labelled as another — which is what would let a retired root key pass as
/// the current one.
#[test]
fn an_epoch_a_signature_was_not_made_at_is_refused() {
    let root = key(7);
    let next = key(8);
    let (genesis, handoff) = rotated(&root, &next);

    // Signed by the OLD root, at epoch 0, then relabelled as epoch 1.
    let mut relabelled = AccountLink::sign(
        &root,
        genesis.account_id(),
        cloud(),
        CHALLENGE,
        ISSUED,
        EXPIRES,
        0,
    )
    .expect("sign");
    relabelled.key_epoch = 1;

    assert!(matches!(
        verify_account_link(genesis.account_id(), &genesis, &[handoff], &relabelled),
        Err(AccountError::LinkSignatureInvalid),
    ));
}

/// THE attack this statement exists to stop.
///
/// A device certificate is also a signature by the account root, so it is the
/// credential an outside verifier is most likely to be offered as proof of
/// ownership — and it is a static blob anyone who has seen it can present. The
/// separate signing domain is what stops the substitution: a certificate's
/// signature does not verify over a link's preimage, so a link cannot be
/// assembled out of one.
#[test]
fn a_device_certificate_cannot_be_presented_as_a_link() {
    let root = key(7);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);

    let cert = sign_cert(&root, account, device, &key(9), 0, 0);

    // Everything the account holder genuinely signed, wearing a link's clothes.
    let forged = AccountLink {
        account,
        audience: cloud(),
        challenge: CHALLENGE,
        issued_at: ISSUED,
        expires_at: EXPIRES,
        key_epoch: 0,
        signature: cert.signature,
    };

    assert!(
        matches!(
            verify_account_link(account, &genesis, &[], &forged),
            Err(AccountError::LinkSignatureInvalid),
        ),
        "a root signature over a certificate must not read as a live, addressed link",
    );
}
