//! What makes a chain an *authorization* chain: it starts at epoch 0, steps by
//! one, is signed by the outgoing key at every step, and is bounded in length
//! before any of that is checked.

use super::support::{genesis_for, key, sign_handoff};
use crate::account::{AccountGenesis, ACCOUNT_GENESIS_VERSION};
use crate::error::AccountError;
use crate::root_key::{root_key_at_epoch, RootKeyHandoff, MAX_ROOT_KEY_HANDOFFS};

#[test]
fn empty_chain_resolves_to_the_genesis_key() {
    let root = key(1);
    let g = genesis_for(&root);
    assert_eq!(
        root_key_at_epoch(&g, &[], 0).expect("valid"),
        root.public_key()
    );
}

#[test]
fn chain_resolves_each_epoch_in_order() {
    let (r0, r1, r2) = (key(1), key(2), key(3));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [
        sign_handoff(&r0, account, 0, &r1),
        sign_handoff(&r1, account, 1, &r2),
    ];
    for (epoch, expected) in [&r0, &r1, &r2].into_iter().enumerate() {
        assert_eq!(
            root_key_at_epoch(&g, &chain, epoch as u32).expect("valid"),
            expected.public_key(),
            "epoch {epoch}"
        );
    }
}

#[test]
fn handoff_must_be_signed_by_the_outgoing_key() {
    let (r0, r1, imposter) = (key(1), key(2), key(9));
    let g = genesis_for(&r0);
    let account = g.account_id();
    // Signed by a key that was never the account's root.
    let chain = [sign_handoff(&imposter, account, 0, &r1)];
    assert_eq!(
        root_key_at_epoch(&g, &chain, 1),
        Err(AccountError::HandoffSignatureInvalid { epoch: 0 })
    );
}

#[test]
fn a_superseded_key_cannot_re_sign_a_later_handoff() {
    // Epoch 0 authorizes epoch 1; epoch 0 must not then authorize epoch 2.
    let (r0, r1, r2) = (key(1), key(2), key(3));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [
        sign_handoff(&r0, account, 0, &r1),
        sign_handoff(&r0, account, 1, &r2), // wrong signer for this position
    ];
    assert_eq!(
        root_key_at_epoch(&g, &chain, 2),
        Err(AccountError::HandoffSignatureInvalid { epoch: 1 })
    );
}

#[test]
fn chain_must_start_at_epoch_zero() {
    let (r0, r1) = (key(1), key(2));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [sign_handoff(&r0, account, 1, &r1)];
    assert_eq!(
        root_key_at_epoch(&g, &chain, 1),
        Err(AccountError::ChainNotContiguous {
            expected: 0,
            found: 1
        })
    );
}

#[test]
fn chain_must_not_skip_an_epoch() {
    let (r0, r1, r2) = (key(1), key(2), key(3));
    let g = genesis_for(&r0);
    let account = g.account_id();
    let chain = [
        sign_handoff(&r0, account, 0, &r1),
        sign_handoff(&r1, account, 2, &r2),
    ];
    assert_eq!(
        root_key_at_epoch(&g, &chain, 2),
        Err(AccountError::ChainNotContiguous {
            expected: 1,
            found: 2
        })
    );
}

#[test]
fn handoff_cannot_be_replayed_onto_another_account() {
    let (r0, r1) = (key(1), key(2));
    let g = genesis_for(&r0);
    let other = AccountGenesis::new(key(3).public_key());
    // Validly signed by r0, but minted for a different account id.
    let stolen = sign_handoff(&r0, other.account_id(), 0, &r1);
    assert_eq!(
        root_key_at_epoch(&g, &[stolen], 1),
        Err(AccountError::HandoffAccountMismatch { epoch: 0 })
    );
}

#[test]
fn unknown_genesis_version_is_rejected() {
    let mut g = genesis_for(&key(1));
    g.version = 200;
    assert_eq!(
        root_key_at_epoch(&g, &[], 0),
        Err(AccountError::UnsupportedVersion {
            found: 200,
            supported: ACCOUNT_GENESIS_VERSION
        })
    );
}

#[test]
fn an_overlong_handoff_chain_is_refused_before_any_verification() {
    // Each entry costs an Ed25519 verification, and this is reachable from
    // untrusted bytes, so the cap has to be checked before the walk rather
    // than relying on every caller to bound the field first.
    let g = genesis_for(&key(1));
    let bogus =
        RootKeyHandoff::sign(&key(1), g.account_id(), 0, &key(2).public_key()).expect("sign");
    let chain = vec![bogus; MAX_ROOT_KEY_HANDOFFS + 1];
    assert_eq!(
        root_key_at_epoch(&g, &chain, 0),
        Err(AccountError::ChainTooLong {
            found: MAX_ROOT_KEY_HANDOFFS + 1,
            limit: MAX_ROOT_KEY_HANDOFFS,
        }),
        "an overlong chain must be refused by length, not by the first bad link"
    );

    // A chain exactly at the cap is still refused on its merits (this one is
    // not contiguous), not by the length gate.
    let at_cap = vec![bogus; MAX_ROOT_KEY_HANDOFFS];
    assert!(!matches!(
        root_key_at_epoch(&g, &at_cap, MAX_ROOT_KEY_HANDOFFS as u32),
        Err(AccountError::ChainTooLong { .. })
    ));
}

#[test]
fn a_minted_handoff_chain_verifies() {
    let (r0, r1, r2) = (key(1), key(2), key(3));
    let g = genesis_for(&r0);
    let account = g.account_id();

    let chain = [
        RootKeyHandoff::sign(&r0, account, 0, &r1.public_key()).expect("sign"),
        RootKeyHandoff::sign(&r1, account, 1, &r2.public_key()).expect("sign"),
    ];

    for (epoch, expected) in [&r0, &r1, &r2].into_iter().enumerate() {
        assert_eq!(
            root_key_at_epoch(&g, &chain, epoch as u32).expect("resolve"),
            expected.public_key(),
            "epoch {epoch}"
        );
    }
}
