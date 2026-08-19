//! The two halves of pairing safety: the statement refuses a partial key
//! substitution, the code refuses a wholesale one.

use calimero_primitives::identity::{DeviceId, PrivateKey};

use super::support::pairing_fixture;
use crate::account::AccountGenesis;
use crate::device::KemPublicKey;
use crate::domain::PAIRING_CONFIRMATION_HEX_LEN;
use crate::error::AccountError;
use crate::pairing::PairingOffer;

#[test]
fn a_pairing_statement_verifies_against_the_key_that_signed_it() {
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let (offer, statement) =
        PairingOffer::signed(&device_sk, account, device, kem_pk).expect("sign");

    assert!(offer.verify_statement(&statement).is_ok());
}

#[test]
fn substituted_key_material_under_a_valid_device_id_is_refused() {
    // The attack the statement exists to refuse. An attacker cannot mint a
    // DeviceId, so it captures a real one in transit and offers its OWN KEM
    // key beneath it — which is what the certificate would then name as the
    // recipient of every scope key the account can read.
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let (_, statement) = PairingOffer::signed(&device_sk, account, device, kem_pk).expect("sign");

    let attacker_kem = KemPublicKey::from([0xAA; 32]);
    assert!(matches!(
        PairingOffer::new(account, device, attacker_kem, device_sk.public_key())
            .verify_statement(&statement),
        Err(AccountError::PairingStatementInvalid),
    ));

    // Nor can it keep the honest KEM key and slip its own signing key in,
    // which would make it the author of the device's ops.
    let attacker_sk = PrivateKey::from([0xBB; 32]);
    assert!(matches!(
        PairingOffer::new(account, device, kem_pk, attacker_sk.public_key())
            .verify_statement(&statement),
        Err(AccountError::PairingStatementInvalid),
    ));
}

#[test]
fn a_statement_does_not_carry_to_another_account() {
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let (_, statement) = PairingOffer::signed(&device_sk, account, device, kem_pk).expect("sign");

    let other = AccountGenesis::new(PrivateKey::from([8u8; 32]).public_key()).account_id();
    assert!(matches!(
        PairingOffer::new(other, device, kem_pk, device_sk.public_key())
            .verify_statement(&statement),
        Err(AccountError::PairingStatementInvalid),
    ));
}

#[test]
fn the_confirmation_code_changes_when_any_certified_value_does() {
    // What the two humans compare. A wholesale substitution re-signs its own
    // statement and passes verification, so the code is the only thing left
    // that distinguishes it — it has to move when anything the certificate
    // names moves.
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let sign_pk = device_sk.public_key();
    let honest = PairingOffer::new(account, device, kem_pk, sign_pk).confirmation_code();

    let attacker_sk = PrivateKey::from([0xBB; 32]);
    for (label, code) in [
        (
            "substituted KEM key",
            PairingOffer::new(account, device, KemPublicKey::from([0xAA; 32]), sign_pk)
                .confirmation_code(),
        ),
        (
            "substituted signing key",
            PairingOffer::new(account, device, kem_pk, attacker_sk.public_key())
                .confirmation_code(),
        ),
        (
            "different device",
            PairingOffer::new(
                account,
                DeviceId::mint(account, [0x44; 16]),
                kem_pk,
                sign_pk,
            )
            .confirmation_code(),
        ),
    ] {
        assert_ne!(honest, code, "code must move: {label}");
    }

    // Same inputs on both ends must agree, or there is nothing to compare.
    assert_eq!(
        honest,
        PairingOffer::new(account, device, kem_pk, sign_pk).confirmation_code()
    );
}

#[test]
fn the_confirmation_code_is_wide_enough_to_resist_grinding() {
    // The attacker knows the honest code and can grind keypairs offline until
    // one matches, so the code's width IS the work factor. Anything short
    // enough to be comfortable to read aloud (six digits, say) is instant.
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let code =
        PairingOffer::new(account, device, kem_pk, device_sk.public_key()).confirmation_code();

    let hex: String = code.chars().filter(|c| *c != '-').collect();
    assert_eq!(
        hex.len(),
        PAIRING_CONFIRMATION_HEX_LEN,
        "64 bits of digest, or the comparison stops being worth making"
    );
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    // Pinned: both ends of a pairing must derive the same code, so a change
    // to the derivation is a change to the wire and has to be deliberate.
    // It moved once, when the genesis dropped its nonce: the code covers the
    // account id, and the account id is a different value under a preimage
    // that no longer has a nonce in it. The derivation itself is untouched.
    assert_eq!(code, "F3B3-B5FB-450D-DF9E", "code derivation is stable");
}

#[test]
fn the_confirmation_code_matches_however_a_person_typed_it() {
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let sign_pk = device_sk.public_key();
    let code = PairingOffer::new(account, device, kem_pk, sign_pk).confirmation_code();

    for variant in [
        code.clone(),
        code.to_lowercase(),
        code.replace('-', ""),
        code.replace('-', " "),
        format!("  {code}  "),
    ] {
        assert!(
            PairingOffer::new(account, device, kem_pk, sign_pk).code_matches(&variant),
            "grouping and case must not decide this: {variant}"
        );
    }
}

#[test]
fn a_code_for_other_key_material_is_refused() {
    // The wholesale substitution: the attacker replaced both keys and
    // re-signed, so the statement verifies — and this is what still refuses
    // it, because the code the account holder was read came from the real
    // device and does not describe the attacker's keys.
    let (account, device, device_sk, kem_pk) = pairing_fixture();
    let honest_code =
        PairingOffer::new(account, device, kem_pk, device_sk.public_key()).confirmation_code();

    let attacker_sk = PrivateKey::from([0xBB; 32]);
    let attacker_kem = KemPublicKey::from([0xAA; 32]);
    assert!(
        !PairingOffer::new(account, device, attacker_kem, attacker_sk.public_key())
            .code_matches(&honest_code),
        "a code that describes the honest keys must not match substituted ones"
    );

    // And nothing empty-ish slips through.
    for junk in ["", "   ", "----", "not-hex-at-all"] {
        assert!(
            !PairingOffer::new(account, device, kem_pk, device_sk.public_key()).code_matches(junk),
            "must refuse: {junk:?}"
        );
    }
}
