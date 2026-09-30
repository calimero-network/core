//! The byte contract a non-Rust creation-warrant signer must reproduce.
//!
//! Same purpose as `warrant_wire_fixture`: the signer of a creation warrant is
//! typically a browser holding a device key and no node, so the init hash, the
//! signing preimage and the borsh encoding are a cross-language contract. The
//! vectors are pinned here, in the repository that can break them, and are the
//! fixture the JS implementation is tested against. Regenerate only when the
//! format is deliberately changing.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::AccountId;

use super::support::key;
use crate::account::borsh_bytes;
use crate::creation::{ContextCreationTerms, ContextCreationWarrant};

const INIT_ARGS: &[u8] = br#"{"name":"general"}"#;

fn terms() -> ContextCreationTerms {
    ContextCreationTerms {
        group: [0x11; 32],
        seed: [0x12; 32],
        author_account: AccountId::from([0x22; 32]),
        executor: AccountId::from([0x33; 32]),
        application_id: ApplicationId::from([0x44; 32]),
        service_name: None,
        name: Some("general".to_owned()),
        init_hash: ContextCreationWarrant::init_hash(INIT_ARGS),
        account_heads: vec![[0x55; 32]],
        governance_floor: vec![[0x66; 32]],
        nonce: 42,
        not_after: 1_700_000_000,
    }
}

fn fixture() -> ContextCreationWarrant {
    ContextCreationWarrant::sign(&key(7), terms()).expect("sign")
}

#[test]
fn the_init_hash_is_stable() {
    assert_eq!(
        hex::encode(ContextCreationWarrant::init_hash(INIT_ARGS)),
        "074dc4be8c7abe685532a48947430edd0301b42f60222e715a834e723d2a055e",
        "the init hash changed; a JS signer computing the old one produces \
         creation warrants refused as not covering their init arguments",
    );
}

#[test]
fn the_signing_preimage_is_stable() {
    assert_eq!(
        hex::encode(fixture().signing_payload()),
        "f838cd94995573a7fea9768a82b6207632c4a89ce37c860e0d05febf5609644f",
        "the signing preimage changed; every creation warrant signed elsewhere \
         now verifies nowhere",
    );
}

#[test]
fn the_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&fixture());

    assert_eq!(
        bytes.len(),
        389,
        "32*6 ids + 1 (service None) + (1 + 4 + 7) name + 32 init_hash + \
         2*(4 + 32) cited heads + 8 + 8 + 64",
    );
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // group
            "1111111111111111111111111111111111111111111111111111111111111111",
            // seed
            "1212121212121212121212121212121212121212121212121212121212121212",
            // author_account
            "2222222222222222222222222222222222222222222222222222222222222222",
            // author_device_key (derived from key(7))
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c",
            // executor
            "3333333333333333333333333333333333333333333333333333333333333333",
            // application_id
            "4444444444444444444444444444444444444444444444444444444444444444",
            // service_name: None
            "00",
            // name: Some, u32 LE length 7, "general"
            "01",
            "07000000",
            "67656e6572616c",
            // init_hash
            "074dc4be8c7abe685532a48947430edd0301b42f60222e715a834e723d2a055e",
            // account_heads: count 1, one head
            "01000000",
            "5555555555555555555555555555555555555555555555555555555555555555",
            // governance_floor: count 1, one head
            "01000000",
            "6666666666666666666666666666666666666666666666666666666666666666",
            // nonce 42, u64 LE
            "2a00000000000000",
            // not_after 1_700_000_000, u64 LE
            "00f1536500000000",
            // signature
            "7e47c0655b1b0ef65a420ef301f6888328579abc153b122d9e7f6396c8757aa5",
            "14d8a4e65ff85912dfb20f6858fcdbfc60818214ca2e6adcbfbf4dae3d9fd906",
        ),
    );
}
