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
        // Reads a value and throws it away — the change-less footgun the guard
        // closes. This is a `compile_fail` fixture: it is only ever compiled,
        // never run, so the `unwrap`s can't actually panic. The discarded
        // `ValueRef` is what `#[must_use]` flags (turned into an error by the
        // crate-level `#![deny(unused_must_use)]` above so trybuild can assert
        // it; `must_use` is warn-by-default and a compile_fail test needs an
        // error).
        self.items.get(&key).unwrap().unwrap();
    }
}

fn main() {}
