//! The shared credential shape: that one verifier really does serve both
//! statement kinds, that a proof and the loose-argument form cannot disagree, and
//! that a proof for the wrong device is refused before anything else is checked.

use calimero_primitives::identity::DeviceId;

use super::support::{genesis_for, key, rotated};
use crate::account::AccountMemberEndorsement;
use crate::device::{verify_device_cert, DeviceCert, KemPublicKey};
use crate::error::AccountError;
use crate::revocation::{verify_device_revocation, DeviceRevocation};
use crate::signed::{AccountProof, RootSigned};

fn cert_for(root: &calimero_primitives::identity::PrivateKey, key_epoch: u32) -> DeviceCert {
    let genesis = genesis_for(root);
    let account = genesis.account_id();
    DeviceCert::sign(
        root,
        account,
        DeviceId::mint(account, [3u8; 16]),
        &key(5).public_key(),
        &KemPublicKey::from([9u8; 32]),
        key_epoch,
        0,
    )
    .expect("sign")
}

#[test]
fn a_proof_and_the_loose_arguments_reach_the_same_verdict() {
    // The two entry points exist because the apply paths hold a borrowed chain
    // and the wire holds an owned one. They must never disagree: a credential
    // admitted through one and refused through the other is a split-brain in
    // whichever plane happened to pick the other door.
    let root = key(1);
    let cert = cert_for(&root, 0);
    let genesis = genesis_for(&root);
    let account = genesis.account_id();
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: cert,
    };

    assert_eq!(
        proof.verify(account).map(|v| *v.get()),
        verify_device_cert(account, &genesis, &[], &cert).map(|v| *v.get()),
    );

    // And on the failing side too, error variant included.
    let stranger = genesis_for(&key(2)).account_id();
    assert_eq!(
        proof.verify(stranger).err(),
        verify_device_cert(stranger, &genesis, &[], &cert).err(),
    );
}

#[test]
fn each_statement_kind_reports_its_own_errors() {
    // What the trait's two associated constants buy: one verifier body, but a
    // certificate failure and a revocation failure still name themselves. A
    // shared body that reported one variant for both would send whoever reads
    // the log to the wrong credential.
    let root = key(1);
    let genesis = genesis_for(&root);
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);

    let mut cert = cert_for(&root, 0);
    cert.signature[0] ^= 0xFF;
    assert_eq!(
        verify_device_cert(account, &genesis, &[], &cert).err(),
        Some(DeviceCert::SIGNATURE_INVALID),
    );
    assert_eq!(
        DeviceCert::SIGNATURE_INVALID,
        AccountError::CertSignatureInvalid
    );

    let mut revocation = DeviceRevocation::sign(&root, account, device, 0).expect("sign");
    revocation.signature[0] ^= 0xFF;
    assert_eq!(
        verify_device_revocation(account, &genesis, &[], &revocation).err(),
        Some(DeviceRevocation::SIGNATURE_INVALID),
    );
    assert_eq!(
        DeviceRevocation::SIGNATURE_INVALID,
        AccountError::RevocationSignatureInvalid
    );
}

#[test]
fn a_proof_for_another_device_is_refused_as_a_device_mismatch() {
    // A valid proof presented against the wrong subject. It must not report an
    // ACCOUNT mismatch: the account matched, and sending a reader to the wrong
    // question is the whole reason this variant exists separately.
    let root = key(1);
    let genesis = genesis_for(&root);
    let account = genesis.account_id();
    let mine = DeviceId::mint(account, [3u8; 16]);
    let theirs = DeviceId::mint(account, [4u8; 16]);

    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: DeviceRevocation::sign(&root, account, mine, 0).expect("sign"),
    };

    assert!(proof.authorises(account, mine).is_ok());
    assert_eq!(
        proof.authorises(account, theirs).err(),
        Some(AccountError::RevocationDeviceMismatch {
            named: mine,
            expected: theirs,
        }),
    );
}

#[test]
fn the_device_check_runs_before_the_signature_is_verified() {
    // Ordering matters for more than tidiness: the device comparison is free and
    // signature verification is not, so a proof aimed at the wrong device must
    // not buy an Ed25519 verification off any caller that can supply one.
    let root = key(1);
    let genesis = genesis_for(&root);
    let account = genesis.account_id();
    let mine = DeviceId::mint(account, [3u8; 16]);
    let theirs = DeviceId::mint(account, [4u8; 16]);

    let mut revocation = DeviceRevocation::sign(&root, account, mine, 0).expect("sign");
    revocation.signature = [0u8; 64];
    let proof = AccountProof {
        genesis,
        chain: vec![],
        statement: revocation,
    };

    // Unsignable garbage, yet the DEVICE mismatch is what comes back — proof the
    // cheap check short-circuited before the expensive one.
    assert!(matches!(
        proof.authorises(account, theirs),
        Err(AccountError::RevocationDeviceMismatch { .. }),
    ));
}

#[test]
fn a_verified_statement_carries_the_fields_the_check_covered() {
    // `Verified<T>` derefs to the statement, so a caller reads the same fields it
    // would off the unchecked value — the difference is that it cannot reach them
    // without a check having happened. The endorsement is the case that had no
    // verified form at all: its gate needs the endorser's key, and getting it from
    // the wrapper is what stops the key being read from an unchecked struct.
    let member = key(1);
    let account = genesis_for(&key(9)).account_id();
    let endorsement = AccountMemberEndorsement::sign(&member, account).expect("sign");

    let verified = endorsement.verify().expect("valid");
    assert_eq!(verified.member, member.public_key());
    assert_eq!(verified.account, account);
    assert_eq!(*verified.get(), endorsement);
    assert_eq!(verified.into_inner(), endorsement);
}

#[test]
fn a_proof_verifies_across_a_rotation_through_its_carried_chain() {
    // The chain is part of the bundle for a reason: a receiver that folded none of
    // the account's rotations still resolves the epoch-1 key from the proof alone.
    let (root, next) = (key(1), key(2));
    let (genesis, handoff) = rotated(&root, &next);
    let account = genesis.account_id();
    let cert = DeviceCert::sign(
        &next,
        account,
        DeviceId::mint(account, [3u8; 16]),
        &key(5).public_key(),
        &KemPublicKey::from([9u8; 32]),
        1,
        0,
    )
    .expect("sign");

    let proof = AccountProof {
        genesis,
        chain: vec![handoff],
        statement: cert,
    };
    assert!(proof.verify(account).is_ok());

    // Drop the chain and the same certificate becomes unverifiable — the epoch it
    // claims is no longer reachable.
    let orphaned = AccountProof {
        genesis,
        chain: vec![],
        statement: cert,
    };
    assert_eq!(
        orphaned.verify(account).err(),
        Some(AccountError::EpochOutOfRange {
            key_epoch: 1,
            reachable: 0,
        }),
    );
}
