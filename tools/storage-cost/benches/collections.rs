//! Wall-clock throughput per workload. Reporting only, never a merge gate.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, SamplingMode, Throughput};
use storage_cost::measure;
use storage_cost::workloads::all;

const SAMPLE_SIZE: usize = 10; // criterion's floor; the top sizes run for seconds per iteration

fn collections(c: &mut Criterion) {
    let mut group = c.benchmark_group("collections");
    let _ignored = group.sample_size(SAMPLE_SIZE);
    let _ignored = group.sampling_mode(SamplingMode::Flat);

    for workload in all() {
        let _ignored = group.throughput(Throughput::Elements(workload.n as u64));
        let _ignored = group.bench_function(format!("{}/{}", workload.name, workload.n), |b| {
            b.iter_batched(
                || workload.n,
                |n| {
                    let (result, _costs) = measure(|| (workload.run)(n));
                    black_box(result)
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, collections);
criterion_main!(benches);
