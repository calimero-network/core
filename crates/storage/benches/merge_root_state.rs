//! What does the merge framework cost around one `Mergeable::merge` call?
//!
//! Uses `merge_root_state_typed`: the registry path is compiled out on native.

use std::hint::black_box;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::{MergeError, MergeStrategy, Mergeable};
use calimero_storage::collections::rekey::RekeyTarget;
use calimero_storage::merge::merge_root_state_typed;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
struct BenchState(Vec<(String, Vec<u8>)>);

impl BenchState {
    fn new(items: usize, salt: u8) -> Self {
        Self(
            (0..items)
                .map(|i| (format!("key{i:08}"), vec![salt; 32]))
                .collect(),
        )
    }
}

impl RekeyTarget for BenchState {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

impl MergeStrategy for BenchState {
    const DISPATCHED: bool = false;
}

impl Mergeable for BenchState {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Both sides carry the same ordered key set, so the zip is O(n)-faithful.
        for (mine, theirs) in self.0.iter_mut().zip(other.0.iter()) {
            mine.1.clone_from(&theirs.1);
        }
        Ok(())
    }
}

fn payload(items: usize, salt: u8) -> Vec<u8> {
    to_vec(&BenchState::new(items, salt)).expect("borsh encode of BenchState cannot fail")
}

fn merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_root_state");

    for items in [10_usize, 100, 1_000, 10_000] {
        let existing = payload(items, 1);
        let incoming = payload(items, 2);

        group.throughput(Throughput::Elements(items as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(items),
            &(existing, incoming),
            |b, (existing, incoming)| {
                b.iter(|| {
                    black_box(merge_root_state_typed::<BenchState>(
                        black_box(existing),
                        black_box(incoming),
                        // created_at != existing_ts, so this is not the bootstrap path.
                        1,
                        2,
                        3,
                    ))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, merge);
criterion_main!(benches);
