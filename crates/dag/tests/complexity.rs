//! Scaling guards for the pending-set paths a peer can drive.
//!
//! Each test times one operation at a small and a large size and asserts the
//! large run grew by no more than a linear algorithm could explain. Wall-clock
//! numbers are machine-dependent, but their *ratio* is not: an O(n) path grows
//! ~`LARGE / SMALL` times, an O(n^2) path ~`(LARGE / SMALL)^2` times, and the
//! bound sits between the two with room on both sides. Taking the fastest of a
//! few runs keeps a scheduler hiccup from reading as a regression.
//!
//! These caught two quadratic paths that the Criterion trend in
//! `benches/pending.rs` only reported as "slower": evicting the waiters of a
//! parent many pending deltas share, and finding an origin's oldest pending
//! delta once it is at [`calimero_dag::MAX_PENDING_PER_ORIGIN`].

use std::time::{Duration, Instant};

use calimero_dag::{AddDeltaOutcome, ApplyError, CausalDelta, DagStore, DeltaApplier};
use calimero_storage::logical_clock::HybridTimestamp;
use tokio::sync::Mutex;

const SMALL: usize = 500;
const LARGE: usize = 8 * SMALL;

/// Linear growth is `LARGE / SMALL` = 8x and quadratic 64x. Allocation and
/// cache effects push a linear path to ~10x, so the bound is set well clear
/// of that and well below the quadratic figure.
const MAX_GROWTH: f64 = 24.0;

/// Runs per size; the fastest is kept.
const RUNS: usize = 3;

/// Charges every pending delta to the origin named by its payload, and records
/// the order deltas are applied in.
#[derive(Default)]
struct Recorder {
    applied: Mutex<Vec<[u8; 32]>>,
}

#[async_trait::async_trait]
impl DeltaApplier<[u8; 32]> for Recorder {
    async fn apply(&self, delta: &CausalDelta<[u8; 32]>) -> Result<(), ApplyError> {
        self.applied.lock().await.push(delta.id);
        Ok(())
    }

    fn admit_pending(&self, delta: &CausalDelta<[u8; 32]>) -> Result<Option<[u8; 32]>, ApplyError> {
        Ok((delta.payload != [0; 32]).then_some(delta.payload))
    }
}

fn id(tag: u8, i: usize) -> [u8; 32] {
    let mut id = [tag; 32];
    id[..8].copy_from_slice(&(i as u64).to_le_bytes());
    id
}

fn delta(id: [u8; 32], parents: Vec<[u8; 32]>, origin: [u8; 32]) -> CausalDelta<[u8; 32]> {
    CausalDelta::new(id, parents, origin, HybridTimestamp::default())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("building a current-thread runtime cannot fail")
}

fn add(
    rt: &tokio::runtime::Runtime,
    dag: &mut DagStore<[u8; 32]>,
    delta: CausalDelta<[u8; 32]>,
    applier: &Recorder,
) -> AddDeltaOutcome {
    rt.block_on(dag.add_delta_with_outcome(delta, applier))
        .expect("the recorder never refuses a delta")
}

/// Fastest of [`RUNS`] timings of `measure(n)`, which builds its own fixture
/// and returns only the time of the operation under test.
fn fastest(n: usize, measure: &mut impl FnMut(usize) -> Duration) -> Duration {
    (0..RUNS)
        .map(|_| measure(n))
        .min()
        .expect("RUNS is not zero")
}

fn assert_scales_linearly(what: &str, mut measure: impl FnMut(usize) -> Duration) {
    // Warm the allocator and caches so the small run is not charged for them.
    let _ = measure(SMALL);
    let small = fastest(SMALL, &mut measure);
    let large = fastest(LARGE, &mut measure);
    let growth = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        growth <= MAX_GROWTH,
        "{what}: {SMALL} -> {LARGE} took {small:?} -> {large:?}, {growth:.1}x; \
         linear is ~{}x, quadratic ~{}x, the bound is {MAX_GROWTH}x",
        LARGE / SMALL,
        (LARGE / SMALL).pow(2),
    );
}

/// `n` pending deltas that all wait on one parent that never arrives.
fn waiting_on_one_parent(
    rt: &tokio::runtime::Runtime,
    n: usize,
    origin: [u8; 32],
) -> (DagStore<[u8; 32]>, Recorder) {
    let mut dag = DagStore::new([0; 32]);
    dag.set_max_pending(usize::MAX);
    let applier = Recorder::default();
    for i in 0..n {
        let outcome = add(
            rt,
            &mut dag,
            delta(id(1, i), vec![id(0xEE, 0)], origin),
            &applier,
        );
        assert!(outcome.is_pending(), "fixture delta {i} did not pend");
    }
    (dag, applier)
}

#[test]
fn evicting_the_waiters_of_one_shared_parent_is_linear() {
    let rt = runtime();
    assert_scales_linearly("cleanup_stale over one shared parent", |n| {
        let (mut dag, _) = waiting_on_one_parent(&rt, n, [0; 32]);
        let start = Instant::now();
        let evicted = dag.cleanup_stale(Duration::ZERO);
        let took = start.elapsed();
        assert_eq!(evicted, n);
        assert_eq!(dag.pending_stats().count, 0);
        took
    });
}

#[test]
fn inserting_into_a_full_pending_map_does_not_grow_with_it() {
    let rt = runtime();
    // The whole cost of `n` inserts, each of which evicts the oldest waiter of
    // the one shared parent. Linear in `n` only if each eviction is O(1)-ish.
    assert_scales_linearly("inserts into a full map of shared-parent waiters", |n| {
        let mut dag = DagStore::new([0; 32]);
        dag.set_max_pending(n);
        let applier = Recorder::default();
        for i in 0..n {
            let _ = add(
                &rt,
                &mut dag,
                delta(id(1, i), vec![id(0xEE, 0)], [0; 32]),
                &applier,
            );
        }
        let start = Instant::now();
        for i in 0..n {
            let _ = add(
                &rt,
                &mut dag,
                delta(id(2, i), vec![id(0xEE, 0)], [0; 32]),
                &applier,
            );
        }
        let took = start.elapsed();
        assert_eq!(dag.pending_stats().count, n, "the cap did not hold");
        took
    });
}

#[test]
fn an_origin_at_its_cap_evicts_its_own_oldest_without_scanning_the_others() {
    let rt = runtime();
    let flooder = [0xF0; 32];
    // `n` older deltas from other origins sit ahead of the flooder's in arrival
    // order, then the flooder fills its quota and keeps sending: each further
    // insert evicts the flooder's oldest. The cost of `n` such inserts must
    // grow with `n` alone, not with `n` times the deltas ahead of it.
    assert_scales_linearly("inserts from an origin at its pending cap", |n| {
        let mut dag = DagStore::new([0; 32]);
        dag.set_max_pending(usize::MAX);
        let applier = Recorder::default();
        for i in 0..n {
            let other = id(0xA0, i % 64);
            let _ = add(
                &rt,
                &mut dag,
                delta(id(1, i), vec![id(0xEE, i)], other),
                &applier,
            );
        }
        for i in 0..calimero_dag::MAX_PENDING_PER_ORIGIN {
            let _ = add(
                &rt,
                &mut dag,
                delta(id(2, i), vec![id(0xED, i)], flooder),
                &applier,
            );
        }
        let start = Instant::now();
        for i in 0..n {
            let _ = add(
                &rt,
                &mut dag,
                delta(id(3, i), vec![id(0xEC, i)], flooder),
                &applier,
            );
        }
        let took = start.elapsed();
        assert_eq!(
            dag.pending_stats().count,
            n + calimero_dag::MAX_PENDING_PER_ORIGIN,
            "the flooder grew past its quota, or evicted another origin's delta"
        );
        took
    });
}

#[test]
fn a_reverse_ordered_chain_cascades_in_linear_time_and_in_order() {
    let rt = runtime();
    assert_scales_linearly("cascade of a reverse-delivered chain", |n| {
        let mut dag = DagStore::new([0; 32]);
        dag.set_max_pending(usize::MAX);
        let applier = Recorder::default();
        for i in (1..n).rev() {
            let _ = add(
                &rt,
                &mut dag,
                delta(id(1, i), vec![id(1, i - 1)], [0; 32]),
                &applier,
            );
        }
        let start = Instant::now();
        let outcome = add(
            &rt,
            &mut dag,
            delta(id(1, 0), vec![[0; 32]], [0; 32]),
            &applier,
        );
        let took = start.elapsed();

        let AddDeltaOutcome::Applied { cascaded } = outcome else {
            panic!("the chain's root did not apply: {outcome:?}");
        };
        assert_eq!(cascaded.len(), n - 1, "the cascade stopped short");
        let order = rt.block_on(applier.applied.lock()).clone();
        let expected: Vec<_> = (0..n).map(|i| id(1, i)).collect();
        assert_eq!(order, expected, "the chain applied out of causal order");
        assert_eq!(dag.get_heads(), vec![id(1, n - 1)]);
        took
    });
}

#[test]
fn a_wide_fan_in_applies_every_waiter_exactly_once() {
    let rt = runtime();
    assert_scales_linearly("cascade of a parent's many waiters", |n| {
        let (mut dag, applier) = waiting_on_one_parent(&rt, n, [0; 32]);
        let start = Instant::now();
        let outcome = add(
            &rt,
            &mut dag,
            delta(id(0xEE, 0), vec![[0; 32]], [0; 32]),
            &applier,
        );
        let took = start.elapsed();

        let AddDeltaOutcome::Applied { cascaded } = outcome else {
            panic!("the shared parent did not apply: {outcome:?}");
        };
        assert_eq!(cascaded.len(), n);
        let mut order = rt.block_on(applier.applied.lock()).clone();
        assert_eq!(
            order.remove(0),
            id(0xEE, 0),
            "a waiter applied before its parent"
        );
        order.sort_unstable();
        order.dedup();
        assert_eq!(order.len(), n, "a waiter applied twice, or not at all");
        assert_eq!(dag.pending_stats().count, 0);
        took
    });
}
