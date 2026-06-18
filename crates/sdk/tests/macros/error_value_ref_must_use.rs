//! A `get()` result (read-only `ValueRef`) that is discarded is a no-op read.
//! `#![deny(unused_must_use)]` turns the `#[must_use]` warning into an error so
//! the suite can assert it.

#![deny(unused_must_use)]

use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state]
struct S {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S {
            items: UnorderedMap::new(),
        }
    }

    pub fn peek(&self, key: String) {
        // Reads a value and throws it away — the change-less footgun the guard closes.
        self.items.get(&key).unwrap().unwrap();
    }
}

fn main() {}
