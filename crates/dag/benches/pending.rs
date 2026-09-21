//! What do the pending-set walks cost as the pending set grows?

use std::hint::black_box;
use std::time::Duration;

use calimero_dag::{AddDeltaOutcome, ApplyError, CausalDelta, DagStore};
use calimero_storage::logical_clock::HybridTimestamp;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

struct NeverApplied;

#[async_trait::async_trait]
impl calimero_dag::DeltaApplier<Vec<u8>> for NeverApplied {
    async fn apply(&self, _delta: &CausalDelta<Vec<u8>>) -> Result<(), ApplyError> {
        unreachable!("fixture deltas never satisfy can_apply, so apply() is never reached")
    }
}

fn pending_dag(n: usize) -> DagStore<Vec<u8>> {
    let mut dag = DagStore::new([0_u8; 32]);
    let applier = NeverApplied;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("building a current-thread runtime cannot fail");

    for i in 0..n as u64 {
        let mut id = [0_u8; 32];
        id[..8].copy_from_slice(&i.to_le_bytes());
        let mut parent = [0xFF_u8; 32];
        parent[..8].copy_from_slice(&i.to_le_bytes());
        let delta = CausalDelta::new(id, vec![parent], vec![0_u8; 64], HybridTimestamp::default());
        let outcome = rt.block_on(dag.add_delta_with_outcome(delta, &applier));
        assert!(
            matches!(outcome, Ok(AddDeltaOutcome::Pending)),
            "fixture delta did not land in the pending set"
        );
    }
    dag
}

fn pending(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag");

    for n in [10_usize, 100, 1_000, 10_000] {
        let dag = pending_dag(n);

        assert_eq!(dag.pending_stats().count, n, "fixture pending count != n");

        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(
            BenchmarkId::new("get_missing_parents", n),
            &dag,
            |b, dag| {
                b.iter(|| black_box(dag.get_missing_parents(black_box(128)).len()));
            },
        );

        group.bench_with_input(BenchmarkId::new("pending_stats", n), &dag, |b, dag| {
            b.iter(|| black_box(dag.pending_stats()));
        });

        // `cleanup_stale` evicts, so rebuild per batch to keep the set at `n`.
        group.bench_with_input(BenchmarkId::new("cleanup_stale", n), &n, |b, &n| {
            b.iter_batched(
                || pending_dag(n),
                |mut dag| black_box(dag.cleanup_stale(Duration::from_secs(0))),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, pending);
criterion_main!(benches);
