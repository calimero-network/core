//! Integration coverage for the production merge-registry backend.
//!
//! Unit tests inside `src/merge/registry.rs` compile under `#[cfg(test)]`
//! and therefore exercise the thread-local `RefCell` backend. That backend
//! exists only to give parallel test runners isolation — it is NOT what
//! production uses.
//!
//! This file lives under `tests/` and is compiled as a separate integration
//! test binary WITHOUT `#[cfg(test)]`, so it links against the real
//! `LazyLock<RwLock<HashMap<TypeId, MergeFn>>>` backend. Its job is to
//! prove that register + dispatch plumb through correctly against the
//! production code path.
//!
//! Intentionally not tested here: the abort-on-poison branch. Exercising
//! it requires panicking inside the lock's critical section, and the
//! response is `std::process::abort()` — which would tear down the test
//! runner along with the lock. That branch is small enough to verify by
//! review.

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
