//! A collection value that isn't `Mergeable` must point the author at a CRDT.
//!
//! `Plain` is borsh-serializable but not a CRDT, so it isolates the `Mergeable`
//! `on_unimplemented` diagnostic (no borsh cascade).

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::UnorderedMap;

#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Plain {
    x: u64,
}

#[app::state]
struct S {
    items: UnorderedMap<String, Plain>,
}

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S {
            items: UnorderedMap::new(),
        }
    }
}

fn main() {}
