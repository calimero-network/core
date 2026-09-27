//! What does each declared index cost a write?
//!
//! Inserts the same entries into an `UnorderedMap` and into `IndexedMap`s
//! declaring one, three and five indexes, with the indexes warm so every
//! insert is maintained incrementally. Reporting only, never a merge gate: the
//! row counts behind these timings are gated by `indexed_map_insert` and
//! `unordered_map_insert_issues` in the snapshot.

use std::hint::black_box;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{
    IndexValue, Indexed, IndexedMap, LwwRegister, Root, UnorderedMap,
};
use calimero_storage::store::MainStorage;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, SamplingMode, Throughput};
use storage_cost::measure;

const SAMPLE_SIZE: usize = 10;

const ENTRIES: usize = 1_000;

/// Five independent fields, so an index count is the only thing that varies.
#[derive(BorshSerialize, BorshDeserialize)]
struct Row<const INDEXES: usize> {
    fields: [LwwRegister<u64>; 5],
}

const NAMES: [&str; 5] = ["f0", "f1", "f2", "f3", "f4"];

impl<const INDEXES: usize> Indexed for Row<INDEXES> {
    const INDEXES: &'static [&'static str] = NAMES.split_at(INDEXES).0;

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        self.fields[index].encode_index(out);
    }
}

fn row<const INDEXES: usize>(i: usize) -> Row<INDEXES> {
    let i = i as u64;
    Row {
        fields: [i, i % 7, i % 13, i % 101, i % 1_009].map(LwwRegister::new),
    }
}

fn insert_indexed<const INDEXES: usize>(n: usize) {
    let mut map =
        Root::new(|| IndexedMap::<String, Row<INDEXES>, MainStorage>::new_with_field_name("rows"));
    if INDEXES > 0 {
        // Warm: a stamped marker is what makes each insert maintain the
        // indexes instead of leaving them to a rebuild.
        let _ignored = map.query(NAMES[0]).count().expect("warm query");
    }
    for i in 0..n {
        let _ignored = map.insert(key(i), row(i)).expect("insert");
    }
}

fn insert_plain(n: usize) {
    let mut map =
        Root::new(|| UnorderedMap::<String, Row<0>, MainStorage>::new_with_field_name("rows"));
    for i in 0..n {
        let _ignored = map.insert(key(i), row(i)).expect("insert");
    }
}

fn key(i: usize) -> String {
    format!("row{i:08}")
}

/// A bench case: its name and the build it times.
type Case = (&'static str, fn(usize));

fn indexes(c: &mut Criterion) {
    let mut group = c.benchmark_group("indexed_map_indexes");
    let _ignored = group.sample_size(SAMPLE_SIZE);
    let _ignored = group.sampling_mode(SamplingMode::Flat);
    let _ignored = group.throughput(Throughput::Elements(ENTRIES as u64));

    let cases: [Case; 4] = [
        ("unordered_map", insert_plain),
        ("indexes_1", insert_indexed::<1>),
        ("indexes_3", insert_indexed::<3>),
        ("indexes_5", insert_indexed::<5>),
    ];
    for (name, run) in cases {
        let _ignored = group.bench_function(format!("insert/{name}/{ENTRIES}"), |b| {
            b.iter_batched(
                || ENTRIES,
                |n| black_box(measure(|| run(n))),
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, indexes);
criterion_main!(benches);
