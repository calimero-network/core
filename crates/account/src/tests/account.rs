//! The anchor's two properties: the id commits to exactly the genesis, and an
//! endorsement commits to exactly the (account, member) pair.

use super::support::{genesis_for, key};
use crate::account::{
    sign_account_endorsement, verify_account_endorsement, AccountGenesis, ACCOUNT_GENESIS_VERSION,
};
use crate::error::AccountError;

// ---- the account id ----

#[test]
fn account_id_is_the_content_address_of_its_genesis() {
    let root = key(1);
    let g = genesis_for(&root);
    assert_eq!(
        g.account_id(),
        g.account_id(),
        "derivation is deterministic"
    );

    // The whole point of the anchor: the id commits to the epoch-0 key.
    let mut other = g;
    other.root_sign_pk = key(2).public_key();
    assert_ne!(g.account_id(), other.account_id());
}

#[test]
fn genesis_version_is_part_of_the_account_id() {
    let root = key(1);
    let mut v1 = genesis_for(&root);
    let id_v1 = v1.account_id();
    v1.version = ACCOUNT_GENESIS_VERSION + 1;
    assert_ne!(id_v1, v1.account_id());
}

#[test]
fn one_root_key_is_one_account_everywhere() {
    // The account is the root key's content address and nothing else, so a
    // node re-derives the same account in every scope it joins, and a
    // recovered root names the account it always named.
    let root = key(1);
    assert_eq!(
        AccountGenesis::new(root.public_key()).account_id(),
        AccountGenesis::new(root.public_key()).account_id(),
        "derivation must be deterministic or a recovered node names a stranger"
    );
    assert_ne!(
        AccountGenesis::new(root.public_key()).account_id(),
        AccountGenesis::new(key(2).public_key()).account_id(),
        "two roots must be two accounts"
    );
}

#[test]
fn the_version_separates_this_genesis_from_the_nonce_bearing_one() {
    // The nonce's removal changes the preimage on its own, but only the
    // version makes the two provably distinct rather than two encodings a
    // reader might try to reconcile. Pinned so a revert of the bump is
    // caught here rather than by ids that silently collide with v1's.
    assert_eq!(ACCOUNT_GENESIS_VERSION, 2);
    assert_eq!(AccountGenesis::new(key(1).public_key()).version, 2);
}

// ---- endorsements ----

#[test]
fn an_endorsement_round_trips_and_rejects_forgery() {
    let member = key(1);
    let account = genesis_for(&key(9)).account_id();

    let endorsement = sign_account_endorsement(&member, account).expect("sign");
    assert_eq!(verify_account_endorsement(&endorsement), Ok(()));

    // A flipped signature byte fails.
    let mut tampered = endorsement;
    tampered.signature[0] ^= 0xFF;
    assert_eq!(
        verify_account_endorsement(&tampered),
        Err(AccountError::EndorsementSignatureInvalid)
    );

    // Naming a different account fails: the account is inside the payload.
    let mut moved = endorsement;
    moved.account = genesis_for(&key(8)).account_id();
    assert_eq!(
        verify_account_endorsement(&moved),
        Err(AccountError::EndorsementSignatureInvalid)
    );
}

#[test]
fn an_endorsement_cannot_be_re_presented_as_another_members() {
    // Why the endorser is inside the signed payload. Without it, swapping the
    // `member` field would leave a signature that verifies against a key which
    // never signed anything — a member could be shown to have endorsed an
    // account it never touched.
    let real = key(1);
    let other = key(2);
    let account = genesis_for(&key(9)).account_id();

    let mut stolen = sign_account_endorsement(&real, account).expect("sign");
    stolen.member = other.public_key();
    assert_eq!(
        verify_account_endorsement(&stolen),
        Err(AccountError::EndorsementSignatureInvalid)
    );
}

#[test]
fn endorsing_someone_elses_account_is_harmless() {
    // Account ids are public, so anyone can endorse one. It grants nothing:
    // enrolling a device still needs the ROOT's signature, which an endorser
    // does not hold. Pinned so the gate is never "tightened" into rejecting a
    // valid endorsement on the mistaken grounds that endorsement implies
    // ownership.
    let stranger = key(7);
    let someone_elses = genesis_for(&key(9)).account_id();

    let endorsement = sign_account_endorsement(&stranger, someone_elses).expect("sign");
    assert_eq!(
        verify_account_endorsement(&endorsement),
        Ok(()),
        "an endorsement is internally valid regardless of who made it; whether \
         the endorser is a member is a separate at-cut question"
    );
}
