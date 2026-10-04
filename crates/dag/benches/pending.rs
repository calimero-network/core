//! What do the pending-set walks cost as the pending set grows, and what does
//! a peer pay to make them grow?
//!
//! `pending` times the read-side walks over a set of unrelated waiters. The
//! `adversarial` group times the shapes a peer controls: many waiters on one
//! missing parent, an origin flooding at its quota behind older deltas, and a
//! chain delivered newest-first. A quadratic regression shows here as a sweep
//! whose per-element time grows with `n`; `tests/complexity.rs` is the gate.

use std::hint::black_box;
use std::time::Duration;

use calimero_dag::{AddDeltaOutcome, ApplyError, CausalDelta, DagStore};
use calimero_storage::logical_clock::HybridTimestamp;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

struct NeverApplied;

/// Applies everything, and charges a pending delta to the origin its first
/// payload byte names (0 = uncharged).
struct ChargedApplier;

#[async_trait::async_trait]
impl calimero_dag::DeltaApplier<Vec<u8>> for ChargedApplier {
    async fn apply(&self, _delta: &CausalDelta<Vec<u8>>) -> Result<(), ApplyError> {
        Ok(())
    }

    fn admit_pending(&self, delta: &CausalDelta<Vec<u8>>) -> Result<Option<[u8; 32]>, ApplyError> {
        Ok(delta.payload.first().filter(|b| **b != 0).map(|b| [*b; 32]))
    }
}

fn id(tag: u8, i: usize) -> [u8; 32] {
    let mut id = [tag; 32];
    id[..8].copy_from_slice(&(i as u64).to_le_bytes());
    id
}

fn delta(id: [u8; 32], parents: Vec<[u8; 32]>, origin: u8) -> CausalDelta<Vec<u8>> {
    CausalDelta::new(id, parents, vec![origin], HybridTimestamp::default())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("building a current-thread runtime cannot fail")
}

fn add(rt: &tokio::runtime::Runtime, dag: &mut DagStore<Vec<u8>>, delta: CausalDelta<Vec<u8>>) {
    let _ = rt
        .block_on(dag.add_delta_with_outcome(delta, &ChargedApplier))
        .expect("the applier refuses nothing");
}

/// `n` uncharged waiters on one parent that has not arrived.
fn shared_parent_dag(rt: &tokio::runtime::Runtime, n: usize) -> DagStore<Vec<u8>> {
    let mut dag = DagStore::new([0_u8; 32]);
    dag.set_max_pending(usize::MAX);
    for i in 0..n {
        add(rt, &mut dag, delta(id(1, i), vec![id(0xEE, 0)], 0));
    }
    dag
}

/// `n` deltas from 64 other origins, then one origin at its full quota.
fn flooded_origin_dag(rt: &tokio::runtime::Runtime, n: usize) -> DagStore<Vec<u8>> {
    let mut dag = DagStore::new([0_u8; 32]);
    dag.set_max_pending(usize::MAX);
    for i in 0..n {
        add(
            rt,
            &mut dag,
            delta(id(1, i), vec![id(0xEE, i)], 1 + (i % 64) as u8),
        );
    }
    for i in 0..calimero_dag::MAX_PENDING_PER_ORIGIN {
        add(rt, &mut dag, delta(id(2, i), vec![id(0xED, i)], 0xF0));
    }
    dag
}

/// A chain of `n` deltas delivered newest-first, all but the root pending.
fn reverse_chain_dag(rt: &tokio::runtime::Runtime, n: usize) -> DagStore<Vec<u8>> {
    let mut dag = DagStore::new([0_u8; 32]);
    dag.set_max_pending(usize::MAX);
    for i in (1..n).rev() {
        add(rt, &mut dag, delta(id(1, i), vec![id(1, i - 1)], 0));
    }
    dag
}

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

fn adversarial(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag_adversarial");
    let rt = runtime();

    for n in [1_000_usize, 4_000, 10_000] {
        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(
            BenchmarkId::new("cleanup_stale_shared_parent", n),
            &n,
            |b, &n| {
                b.iter_batched(
                    || shared_parent_dag(&rt, n),
                    |mut dag| black_box(dag.cleanup_stale(Duration::ZERO)),
                    BatchSize::LargeInput,
                );
            },
        );

        // One insert from the flooding origin, which evicts its own oldest.
        // Flat in `n` when that lookup does not walk the older deltas.
        group.bench_with_input(BenchmarkId::new("insert_at_origin_cap", n), &n, |b, &n| {
            let mut dag = flooded_origin_dag(&rt, n);
            let mut i = 0;
            b.iter(|| {
                i += 1;
                add(&rt, &mut dag, delta(id(3, i), vec![id(0xEC, i)], 0xF0));
            });
        });

        group.bench_with_input(BenchmarkId::new("cascade_reverse_chain", n), &n, |b, &n| {
            b.iter_batched(
                || reverse_chain_dag(&rt, n),
                |mut dag| add(&rt, &mut dag, delta(id(1, 0), vec![[0; 32]], 0)),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, pending, adversarial);
criterion_main!(benches);
