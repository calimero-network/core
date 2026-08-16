//! The device id's stability, and every field a certificate has to commit to.

use std::collections::HashSet;

use calimero_primitives::identity::DeviceId;

use super::support::{genesis_for, key, sign_cert, sign_handoff};
use crate::account::AccountGenesis;
use crate::device::{sign_device_cert, verify_device_cert, KemPublicKey};
use crate::error::AccountError;
use crate::revocation::{sign_device_revocation, verify_device_revocation};
use crate::root_key::sign_root_key_handoff;

// ---- device ids ----

#[test]
fn device_id_is_stable_across_device_key_rotation() {
    // The reason DeviceId is minted from a nonce rather than from the
    // device's keys: rotating keys must not orphan the replica's CRDT state.
    let account = genesis_for(&key(1)).account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    assert_eq!(device, DeviceId::mint(account, [3u8; 16]));
}

#[test]
fn device_ids_differ_by_account_and_by_nonce() {
    let a = genesis_for(&key(1)).account_id();
    let b = genesis_for(&key(2)).account_id();
    assert_ne!(DeviceId::mint(a, [1u8; 16]), DeviceId::mint(a, [2u8; 16]));
    assert_ne!(DeviceId::mint(a, [1u8; 16]), DeviceId::mint(b, [1u8; 16]));
}

#[test]
fn hlc_seed_is_the_device_id_prefix() {
    let account = genesis_for(&key(1)).account_id();
    let device = DeviceId::mint(account, [4u8; 16]);
    assert_eq!(&device.hlc_seed()[..], &device.as_bytes()[..16]);
}

#[test]
fn distinct_devices_get_distinct_hlc_seeds() {
    // Not a proof of uniqueness — that is enforced at link time by the
    // projection. This only guards against a derivation that collapses.
    let account = genesis_for(&key(1)).account_id();
    let seeds: HashSet<[u8; 16]> = (0..64u8)
        .map(|n| DeviceId::mint(account, [n; 16]).hlc_seed())
        .collect();
    assert_eq!(seeds.len(), 64);
}

// ---- certificates ----

#[test]
fn valid_cert_verifies_and_reports_its_fields() {
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let cert = sign_cert(&root, account, device, &dev, 0, 0);

    let verified = verify_device_cert(account, &g, &[], &cert).expect("valid");
    assert_eq!(verified.account, account);
    assert_eq!(verified.device, device);
    assert_eq!(verified.sign_pk, dev.public_key());
    assert_eq!(verified.key_epoch, 0);
    assert_eq!(verified.device_epoch, 0);
}

#[test]
fn cert_signed_by_a_rotated_key_verifies_against_the_chain() {
    let (r0, r1, dev) = (key(1), key(2), key(5));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let chain = [sign_handoff(&r0, account, 0, &r1)];
    let cert = sign_cert(&r1, account, device, &dev, 1, 0);
    assert!(verify_device_cert(account, &g, &chain, &cert).is_ok());
}

#[test]
fn genesis_must_address_the_claimed_account() {
    // The anchor check: a well-formed credential for account X must not
    // verify when the caller asked about account Y.
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let other = AccountGenesis::new(key(2).public_key()).account_id();
    let cert = sign_cert(
        &root,
        account,
        DeviceId::mint(account, [3u8; 16]),
        &dev,
        0,
        0,
    );

    assert_eq!(
        verify_device_cert(other, &g, &[], &cert),
        Err(AccountError::GenesisMismatch {
            claimed: other,
            actual: account
        })
    );
}

#[test]
fn cert_for_a_different_account_than_the_genesis_is_rejected() {
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let foreign = AccountGenesis::new(key(8).public_key()).account_id();
    let cert = sign_cert(
        &root,
        foreign,
        DeviceId::mint(foreign, [3u8; 16]),
        &dev,
        0,
        0,
    );
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::CertAccountMismatch)
    );
}

#[test]
fn cert_claiming_an_epoch_beyond_the_chain_is_rejected() {
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let cert = sign_cert(&root, account, device, &dev, 3, 0);
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::EpochOutOfRange {
            key_epoch: 3,
            reachable: 0
        })
    );
}

#[test]
fn cert_signed_by_the_wrong_epoch_key_is_rejected() {
    // r0 signs but claims epoch 1, whose key is r1.
    let (r0, r1, dev) = (key(1), key(2), key(5));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let chain = [sign_handoff(&r0, account, 0, &r1)];
    let cert = sign_cert(&r0, account, device, &dev, 1, 0);
    assert_eq!(
        verify_device_cert(account, &g, &chain, &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn substituting_the_device_signing_key_invalidates_the_cert() {
    let (root, dev, attacker) = (key(1), key(5), key(6));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
    cert.sign_pk = attacker.public_key();
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn substituting_the_kem_key_invalidates_the_cert() {
    // Otherwise an attacker could redirect wrapped scope keys to themselves
    // while leaving a valid-looking signing binding in place.
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
    cert.kem_pk = KemPublicKey::from([0xAAu8; 32]);
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn substituting_the_device_id_invalidates_the_cert() {
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let mut cert = sign_cert(
        &root,
        account,
        DeviceId::mint(account, [3u8; 16]),
        &dev,
        0,
        0,
    );
    cert.device = DeviceId::mint(account, [4u8; 16]);
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn bumping_the_device_epoch_invalidates_the_cert() {
    // device_epoch drives supersession at the projection, so it must be
    // signed — otherwise anyone could replay an old cert at a higher epoch.
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let mut cert = sign_cert(&root, account, device, &dev, 0, 0);
    cert.device_epoch = 7;
    assert_eq!(
        verify_device_cert(account, &g, &[], &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn a_credential_ignores_a_handoff_beyond_the_epoch_it_needs() {
    // Only the key at the credential's own epoch decides anything. A handoff
    // *past* that epoch is not part of the authorization it rests on, so
    // refusing the whole credential over one made a garbage entry appended by
    // the carrier — or one the holder built wrong — invalidate a certificate
    // that verifies perfectly against a key the chain genuinely established.
    //
    // It is also the difference between one Ed25519 verification and up to
    // MAX_ROOT_KEY_HANDOFFS of them on a path any member can drive: the cap
    // bounds that work, it does not avoid doing it.
    let (r0, r1, r2, imposter) = (key(1), key(2), key(3), key(9));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    let chain = [
        sign_handoff(&r0, account, 0, &r1),
        // Never authorized: signed by a key that was never this account's root.
        sign_handoff(&imposter, account, 1, &r2),
    ];

    let cert = sign_cert(&r1, account, device, &key(5), 1, 0);
    assert!(
        verify_device_cert(account, &g, &chain, &cert).is_ok(),
        "epoch 1 is established by the first handoff alone"
    );

    let revocation = sign_device_revocation(&r1, account, device, 1).expect("sign");
    assert!(
        verify_device_revocation(account, &g, &chain, &revocation).is_ok(),
        "and the same holds for a revocation proof, which accepts any epoch \
         its chain resolves"
    );

    // The unauthorized handoff still buys nothing: the epoch it claims to
    // establish is unreachable.
    let forged = sign_cert(&r2, account, device, &key(5), 2, 0);
    assert!(matches!(
        verify_device_cert(account, &g, &chain, &forged),
        Err(AccountError::HandoffSignatureInvalid { epoch: 1 })
    ));
}

// ---- minting ----

#[test]
fn a_minted_cert_verifies() {
    // The round trip that matters: whatever the signer produces, the
    // verifier must accept. If the two ever assemble the preimage
    // differently, this is what catches it.
    let (root, dev) = (key(1), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);

    let cert = sign_device_cert(
        &root,
        account,
        device,
        &dev.public_key(),
        &KemPublicKey::from([9u8; 32]),
        0,
        0,
    )
    .expect("sign");

    let verified = verify_device_cert(account, &g, &[], &cert).expect("verify");
    assert_eq!(verified.device, device);
    assert_eq!(verified.sign_pk, dev.public_key());
}

#[test]
fn a_cert_minted_under_a_rotated_key_verifies_against_the_chain() {
    // End to end through both minters: rotate the root, then certify a
    // device with the new key.
    let (r0, r1, dev) = (key(1), key(2), key(5));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [sign_root_key_handoff(&r0, account, 0, &r1.public_key()).expect("sign")];

    let cert = sign_device_cert(
        &r1,
        account,
        DeviceId::mint(account, [3u8; 16]),
        &dev.public_key(),
        &KemPublicKey::from([9u8; 32]),
        1,
        0,
    )
    .expect("sign");

    assert!(verify_device_cert(account, &g, &chain, &cert).is_ok());
}

#[test]
fn minting_with_the_wrong_key_for_the_claimed_epoch_fails_verification() {
    // The minter does not check that the signer matches `key_epoch` — it
    // signs what it is told. The verifier is what enforces the pairing, so
    // a caller that passes the stale key gets a cert that simply does not
    // verify, rather than one that quietly works.
    let (r0, r1, dev) = (key(1), key(2), key(5));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [sign_root_key_handoff(&r0, account, 0, &r1.public_key()).expect("sign")];

    let cert = sign_device_cert(
        &r0, // superseded key...
        account,
        DeviceId::mint(account, [3u8; 16]),
        &dev.public_key(),
        &KemPublicKey::from([9u8; 32]),
        1, // ...claiming the new epoch
        0,
    )
    .expect("sign");

    assert_eq!(
        verify_device_cert(account, &g, &chain, &cert),
        Err(AccountError::CertSignatureInvalid)
    );
}

#[test]
fn minted_credentials_are_deterministic() {
    // Ed25519 signatures here are deterministic, so the same inputs give
    // byte-identical output. Worth pinning: a nondeterministic credential
    // would change an op's content address on every re-issue.
    let root = key(1);
    let g = genesis_for(&root);
    let account = g.account_id();
    let mint = || {
        sign_device_cert(
            &root,
            account,
            DeviceId::mint(account, [3u8; 16]),
            &key(5).public_key(),
            &KemPublicKey::from([9u8; 32]),
            0,
            0,
        )
        .expect("sign")
    };
    assert_eq!(mint(), mint());
}
