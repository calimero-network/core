#![no_main]
//! Session tokens: every request with a passkey session carries one, and the
//! node reads it before it knows who sent it.
//!
//! A token is either refused or verifies under exactly the key that minted
//! it: one that verifies under the node's key never verifies under another.

use calimero_identity::{peek_session, verify_session};
use libfuzzer_sys::fuzz_target;

const KEY: [u8; 32] = [7; 32];
const OTHER: [u8; 32] = [8; 32];

fuzz_target!(|data: &[u8]| {
    let token = String::from_utf8_lossy(data);
    let _ = peek_session(&token);
    for now in [0, u64::MAX] {
        if verify_session(&KEY, &token, now).is_ok() {
            assert!(
                verify_session(&OTHER, &token, now).is_err(),
                "a token verified under two keys"
            );
        }
    }
});
