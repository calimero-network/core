//! Integration coverage for the production merge-registry backend.
//!
//! Unit tests inside `src/merge/registry.rs` compile under `#[cfg(test)]`
//! and therefore exercise the thread-local `RefCell` backend. That backend
//! exists only to give parallel test runners isolation — it is NOT what
//! production uses.
//!
//! This file lives under `tests/` and is compiled as a separate integration
//! test binary. The binary itself IS built with `#[cfg(test)]` (so `#[test]`
//! attributes work), but the `calimero-storage` *library* it links against
//! is compiled WITHOUT `#[cfg(test)]` — which is what matters here. The
//! library side is where the registry backend selection happens, so this
//! test exercises the real `LazyLock<RwLock<HashMap<TypeId, MergeFn>>>`
//! path. Its job is to prove that register + dispatch plumb through
//! correctly against the production code.
//!
//! Intentionally not tested here: the abort-on-poison branch. Exercising
//! it requires panicking inside the lock's critical section, and the
//! response is `std::process::abort()` — which would tear down the test
//! runner along with the lock. That branch is small enough to verify by
//! review.
//!
//! # Only one test per file
//!
//! The production registry is a process-global `RwLock`; there is no
//! `clear_merge_registry` available here because it's gated behind
//! `#[cfg(test)]` on the library side. Every `#[test]` in this binary
//! runs against the same registry and cannot clean up after itself. If
//! two tests register different types with overlapping borsh layouts,
//! `try_merge_registered`'s HashMap-order iteration could dispatch to
//! the wrong one — a flake we just eliminated in the unit tests.
//!
//! Convention for this crate: **one registering test per `tests/*.rs`
//! file**. Each file gets its own test binary (Cargo default), so
//! process-global state doesn't leak between them. If you need another
//! integration scenario, add a new `tests/merge_registry_<scenario>.rs`
//! file rather than another `#[test]` here.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::Mergeable;
use calimero_storage::merge::{register_crdt_merge, try_merge_registered, MergeRegistryResult};

#[derive(BorshSerialize, BorshDeserialize)]
struct IntegrationState {
    values: Vec<u32>,
}

impl Mergeable for IntegrationState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.values.extend_from_slice(&other.values);
        self.values.sort_unstable();
        self.values.dedup();
        Ok(())
    }
}

#[test]
fn register_and_dispatch_against_production_registry() {
    // Registering against the real RwLock-backed global registry.
    register_crdt_merge::<IntegrationState>();

    let a = IntegrationState {
        values: vec![1, 3, 5],
    };
    let b = IntegrationState {
        values: vec![2, 3, 4],
    };

    let bytes_a = borsh::to_vec(&a).unwrap();
    let bytes_b = borsh::to_vec(&b).unwrap();

    let merged_bytes = match try_merge_registered(&bytes_a, &bytes_b, 100, 200) {
        MergeRegistryResult::Success(bytes) => bytes,
        other => panic!("expected Success from production registry, got {:?}", other),
    };

    let merged: IntegrationState = borsh::from_slice(&merged_bytes).unwrap();
    assert_eq!(merged.values, vec![1, 2, 3, 4, 5]);
}
