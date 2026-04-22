//! Merge-time authorization tests for `AuthoredMap` and `AuthoredVector`.
//!
//! Unit tests inside each primitive's module cover the single-node API (owner
//! stamping, local owner-check short-circuits). These tests cross the merge
//! boundary: they insert via the primitive, then feed a hand-crafted delta
//! into `Interface::apply_action` to verify the storage layer rejects
//! tampered writes — the central security guarantee of these primitives.

use borsh::to_vec;
use calimero_primitives::identity::PublicKey;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::collections::{AuthoredMap, AuthoredVector, Root};
use crate::entities::{Metadata, SignatureData, StorageType};
use crate::env;
use crate::error::StorageError;
use crate::interface::Interface;
use crate::store::MainStorage;
use crate::tests::common::{create_test_keypair, sign_action};

type MainInterface = Interface<MainStorage>;

/// Build an `Action::Update` stamped with `claimed_owner`, signed by
/// `signing_key`. If `claimed_owner` doesn't match `signing_key`'s pubkey,
/// the signature won't verify against the claim — the merge path rejects it.
fn build_signed_update_for(
    id: crate::address::Id,
    data: Vec<u8>,
    claimed_owner: PublicKey,
    signing_key: &SigningKey,
    nonce: u64,
) -> Action {
    let timestamp = env::time_now();
    let metadata = Metadata {
        created_at: timestamp,
        updated_at: timestamp.into(),
        storage_type: StorageType::User {
            owner: claimed_owner,
            signature_data: Some(SignatureData {
                signature: [0; 64],
                nonce,
            }),
        },
        crdt_type: None,
        field_name: None,
    };

    let mut action = Action::Update {
        id,
        data,
        ancestors: vec![],
        metadata,
    };

    let signature = sign_action(&action, signing_key);
    if let Action::Update {
        ref mut metadata, ..
    } = action
    {
        if let StorageType::User {
            ref mut signature_data,
            ..
        } = metadata.storage_type
        {
            *signature_data = Some(SignatureData { signature, nonce });
        }
    }
    action
}

/// Build an `Action::DeleteRef` claiming `claimed_owner`, signed by
/// `signing_key`. The interface verifies `claimed_owner` matches the entry's
/// stored owner, then checks the signature against `claimed_owner`.
fn build_signed_delete_for(
    id: crate::address::Id,
    claimed_owner: PublicKey,
    signing_key: &SigningKey,
    deleted_at: u64,
) -> Action {
    let metadata = Metadata {
        created_at: env::time_now(),
        updated_at: deleted_at.into(),
        storage_type: StorageType::User {
            owner: claimed_owner,
            signature_data: Some(SignatureData {
                signature: [0; 64],
                nonce: deleted_at,
            }),
        },
        crdt_type: None,
        field_name: None,
    };

    let mut action = Action::DeleteRef {
        id,
        deleted_at,
        metadata,
    };

    let signature = sign_action(&action, signing_key);
    if let Action::DeleteRef {
        ref mut metadata, ..
    } = action
    {
        if let StorageType::User {
            ref mut signature_data,
            ..
        } = metadata.storage_type
        {
            *signature_data = Some(SignatureData {
                signature,
                nonce: deleted_at,
            });
        }
    }
    action
}

/// An `Update` claiming ownership by Alice but signed with Bob's key must be
/// rejected — the signature will not verify against Alice's pubkey.
#[test]
#[serial]
fn authored_map_update_with_forged_owner_claim_is_rejected() {
    env::reset_for_testing();

    let (alice_sk, alice_pk) = create_test_keypair();
    let (bob_sk, _bob_pk) = create_test_keypair();

    // Alice inserts the entry (stamped owner = Alice).
    env::set_executor_id(*alice_pk.digest());
    let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
    map.insert("apple".to_owned(), 1).expect("alice insert");

    let entry_id = map.entry_id(&"apple".to_owned());

    // Bob forges an Update claiming to be Alice. He has to sign with Alice's
    // identity as declared in metadata — but he doesn't hold Alice's key, so
    // he signs with his own. The signature verification against alice_pk fails.
    let forged = build_signed_update_for(
        entry_id,
        to_vec(&("apple".to_owned(), 99u64)).unwrap(),
        alice_pk,
        &bob_sk,
        env::time_now(),
    );

    match MainInterface::apply_action(forged) {
        Err(StorageError::InvalidSignature) => {}
        other => panic!("expected InvalidSignature, got {:?}", other),
    }

    // Silence unused_variables warning on alice_sk (not used in this path).
    let _ = alice_sk;
}

/// A `DeleteRef` signed by Bob but claiming Bob as owner, targeting an entry
/// whose stored owner is Alice, must be rejected — the interface verifies
/// `claimed_owner == existing_owner` before the signature check.
#[test]
#[serial]
fn authored_map_delete_by_non_owner_is_rejected() {
    env::reset_for_testing();

    let (_alice_sk, alice_pk) = create_test_keypair();
    let (bob_sk, bob_pk) = create_test_keypair();

    env::set_executor_id(*alice_pk.digest());
    let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
    map.insert("apple".to_owned(), 1).expect("alice insert");

    let entry_id = map.entry_id(&"apple".to_owned());

    // Bob legitimately signs a delete claiming to be Bob. His signature is
    // valid against his own pubkey, but the stored owner is Alice — merge
    // path rejects before even trying the signature check.
    let forged = build_signed_delete_for(entry_id, bob_pk, &bob_sk, env::time_now());

    match MainInterface::apply_action(forged) {
        Err(StorageError::InvalidSignature) => {}
        other => panic!("expected InvalidSignature, got {:?}", other),
    }
}

/// Same forgery, but against an `AuthoredVector` position.
#[test]
#[serial]
fn authored_vector_update_with_forged_owner_claim_is_rejected() {
    env::reset_for_testing();

    let (_alice_sk, alice_pk) = create_test_keypair();
    let (bob_sk, _bob_pk) = create_test_keypair();

    env::set_executor_id(*alice_pk.digest());
    let mut v = Root::new(|| AuthoredVector::<u64>::new());
    v.push(7).expect("alice push");

    let entry_id = v
        .entry_id_at(0)
        .expect("entry id lookup")
        .expect("entry exists");

    let forged = build_signed_update_for(
        entry_id,
        to_vec(&99u64).unwrap(),
        alice_pk,
        &bob_sk,
        env::time_now(),
    );

    match MainInterface::apply_action(forged) {
        Err(StorageError::InvalidSignature) => {}
        other => panic!("expected InvalidSignature, got {:?}", other),
    }
}

/// Non-owner deletion of an `AuthoredVector` slot is rejected.
#[test]
#[serial]
fn authored_vector_delete_by_non_owner_is_rejected() {
    env::reset_for_testing();

    let (_alice_sk, alice_pk) = create_test_keypair();
    let (bob_sk, bob_pk) = create_test_keypair();

    env::set_executor_id(*alice_pk.digest());
    let mut v = Root::new(|| AuthoredVector::<u64>::new());
    v.push(7).expect("alice push");

    let entry_id = v
        .entry_id_at(0)
        .expect("entry id lookup")
        .expect("entry exists");

    let forged = build_signed_delete_for(entry_id, bob_pk, &bob_sk, env::time_now());

    match MainInterface::apply_action(forged) {
        Err(StorageError::InvalidSignature) => {}
        other => panic!("expected InvalidSignature, got {:?}", other),
    }
}
