//! A hand-written `Mergeable` must say HOW it merges.
//!
//! Before this was required, such an impl compiled and — stored as a collection
//! value — resolved last-write-wins with `merge` never called. It type-checked,
//! passed convergence tests, and decided nothing. The reference app shipped in
//! that state for months.
//!
//! `#[app::mergeable]` (dispatched) and `#[derive(Mergeable)]` (structural) are
//! the two answers; this pins that giving neither is a compile error rather
//! than a silent default.

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable};

#[derive(Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Undeclared {
    hits: Counter,
}

impl Mergeable for Undeclared {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.hits.merge(&other.hits)
    }
}

fn main() {}
