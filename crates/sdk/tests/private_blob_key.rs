#![allow(
    dead_code,
    reason = "Only the generated keys are read; the blobs are never built"
)]

//! An `#[app::private]` blob's key must fit the node's private-state key: the
//! node refuses any other width, and the blob would silently not persist.

use calimero_prelude::constants::{PRIVATE_BLOB_KEY_TAG, STATE_KEY_LEN};
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
struct Secrets {
    count: u32,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private(key = b"short")]
struct Named {
    count: u32,
}

#[test]
fn private_blob_keys_have_the_state_key_width_and_the_blob_tag() {
    for key in [SECRETS_KEY, NAMED_KEY] {
        assert_eq!(key.len(), STATE_KEY_LEN);
        assert_eq!(key[0], PRIVATE_BLOB_KEY_TAG);
    }
    assert_ne!(SECRETS_KEY, NAMED_KEY);
}
