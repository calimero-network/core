//! Concurrent coverage for the production merge-registry backend.
//!
//! Same rationale as `merge_registry_integration.rs`: this test binary
//! links the library without `#[cfg(test)]`, so it exercises the real
//! `LazyLock<RwLock<HashMap<TypeId, MergeFn>>>` path — the one unit
//! tests can't reach.
//!
//! This file specifically stresses *concurrent* register + dispatch to
//! prove the `RwLock` backend is contention-safe (no deadlock, no lost
//! registrations) under the kind of parallel load the production
//! runtime can generate when several async workers process sync traffic
//! simultaneously. Single-threaded happy-path coverage lives in
//! `merge_registry_integration.rs`.
//!
//! Same one-test-per-file convention applies: the production registry
//! has no cleanup hook here, so adding a second `#[test]` in this file
//! would leak state into it.

use std::sync::{Arc, Barrier};
use std::thread;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::Mergeable;
use calimero_storage::merge::{register_crdt_merge, try_merge_registered, MergeRegistryResult};

#[derive(BorshSerialize, BorshDeserialize)]
struct ConcurrentState {
    values: Vec<u32>,
}

impl Mergeable for ConcurrentState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.values.extend_from_slice(&other.values);
        self.values.sort_unstable();
        self.values.dedup();
        Ok(())
    }
}

#[test]
fn concurrent_register_and_dispatch_against_production_registry() {
    const THREADS: usize = 16;
    const ITERATIONS_PER_THREAD: usize = 50;

    // Barrier forces all threads to reach the race window at the same
    // time rather than serialising one-after-another — that's what
    // actually stresses the RwLock.
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();

                for i in 0..ITERATIONS_PER_THREAD {
                    // Odd threads mostly write (register), even threads
                    // mostly read (dispatch). Both code paths hit the
                    // lock so readers and writers interleave.
                    if t % 2 == 1 {
                        register_crdt_merge::<ConcurrentState>();
                    }

                    let a = ConcurrentState {
                        values: vec![t as u32, i as u32],
                    };
                    let b = ConcurrentState {
                        values: vec![(t * 100 + i) as u32],
                    };
                    let bytes_a = borsh::to_vec(&a).unwrap();
                    let bytes_b = borsh::to_vec(&b).unwrap();

                    match try_merge_registered(&bytes_a, &bytes_b, 1, 2) {
                        MergeRegistryResult::Success(merged_bytes) => {
                            let merged: ConcurrentState = borsh::from_slice(&merged_bytes).unwrap();
                            // Merge must be sorted + deduped, containing
                            // every input value exactly once.
                            for v in &a.values {
                                assert!(merged.values.contains(v));
                            }
                            for v in &b.values {
                                assert!(merged.values.contains(v));
                            }
                            assert!(merged.values.windows(2).all(|w| w[0] < w[1]));
                        }
                        MergeRegistryResult::NoFunctionsRegistered => {
                            // Acceptable early in the run — before any
                            // writer has landed its first register call.
                            // Writers are always running, so we'll
                            // catch up.
                        }
                        MergeRegistryResult::AllFunctionsFailed => {
                            panic!(
                                "thread {t} iter {i}: dispatch failed despite \
                                 registration being in flight"
                            );
                        }
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("no worker thread should panic");
    }

    // Final check: after all threads have finished registering, a
    // single-threaded dispatch must still succeed. Proves the last
    // writer didn't leave the map in a weird state.
    let a = ConcurrentState {
        values: vec![100, 200],
    };
    let b = ConcurrentState {
        values: vec![150, 250],
    };
    let bytes_a = borsh::to_vec(&a).unwrap();
    let bytes_b = borsh::to_vec(&b).unwrap();
    match try_merge_registered(&bytes_a, &bytes_b, 1, 2) {
        MergeRegistryResult::Success(merged_bytes) => {
            let merged: ConcurrentState = borsh::from_slice(&merged_bytes).unwrap();
            assert_eq!(merged.values, vec![100, 150, 200, 250]);
        }
        other => panic!("post-contention dispatch failed: {other:?}"),
    }
}
