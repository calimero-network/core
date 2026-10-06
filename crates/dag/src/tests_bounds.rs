//! A peer-supplied delta cannot park an unbounded parent list in the pending buffer.

use super::*;

const PARENTS_IN_ONE_GOSSIP_MESSAGE: u32 = 30_000; // 960 KB of ids, under the 1 MiB gossip cap

struct Noop;

#[async_trait::async_trait]
impl DeltaApplier<()> for Noop {
    async fn apply(&self, _delta: &CausalDelta<()>) -> Result<(), ApplyError> {
        Ok(())
    }
}

/// `count` distinct parent ids no delta in the DAG has.
fn missing_parents(count: u32) -> Vec<[u8; 32]> {
    (1..=count)
        .map(|i| {
            let mut id = [0xAA; 32];
            id[..4].copy_from_slice(&i.to_be_bytes());
            id
        })
        .collect()
}

#[tokio::test]
async fn a_delta_naming_thousands_of_parents_is_not_buffered() {
    let mut dag = DagStore::<()>::new([0; 32]);
    let ordinary = CausalDelta::new_test([1; 32], missing_parents(2), ());
    assert!(
        dag.add_delta_with_outcome(ordinary, &Noop)
            .await
            .unwrap()
            .is_pending(),
        "control: a delta waits for its missing parents"
    );

    let flood = CausalDelta::new_test([2; 32], missing_parents(PARENTS_IN_ONE_GOSSIP_MESSAGE), ());
    let refused = dag.add_delta_with_outcome(flood, &Noop).await;
    assert!(matches!(refused, Err(DagError::TooManyParents { .. })));
    assert!(
        !dag.has_delta(&[2; 32]),
        "a delta's parent list must be bounded before it is buffered"
    );
}

#[tokio::test]
async fn a_store_can_allow_more_parents_than_the_default() {
    let mut dag = DagStore::<()>::new([0; 32]);
    dag.set_max_parents(MAX_DELTA_PARENTS * 2);
    let wide = CausalDelta::new_test([3; 32], missing_parents(300), ());

    assert!(dag
        .add_delta_with_outcome(wide, &Noop)
        .await
        .unwrap()
        .is_pending());
}

struct NoopBytes;

#[async_trait::async_trait]
impl DeltaApplier<Vec<u8>> for NoopBytes {
    async fn apply(&self, _delta: &CausalDelta<Vec<u8>>) -> Result<(), ApplyError> {
        Ok(())
    }
}

/// A delta with one missing parent unique to `n`, carrying `payload_len` bytes.
fn waiting_delta(n: u8, payload_len: usize) -> CausalDelta<Vec<u8>> {
    CausalDelta::new_test(
        [n; 32],
        vec![[n.wrapping_add(100); 32]],
        vec![0; payload_len],
    )
}

#[tokio::test]
async fn pending_deltas_are_bounded_by_bytes() {
    let mut dag = DagStore::<Vec<u8>>::new([0; 32]);
    dag.max_pending_bytes = 3 * pending_charge(&waiting_delta(1, 1024));

    for n in 1..=5 {
        let outcome = dag
            .add_delta_with_outcome(waiting_delta(n, 1024), &NoopBytes)
            .await;
        assert!(outcome.unwrap().is_pending());
    }

    assert!(dag.pending_bytes <= dag.max_pending_bytes);
    assert!(
        !dag.has_delta(&[1; 32]) && !dag.has_delta(&[2; 32]),
        "oldest make room"
    );
    assert!((3..=5).all(|n| dag.has_delta(&[n; 32])), "newest are kept");
}

#[tokio::test]
async fn a_larger_newcomer_evicts_as_many_of_the_oldest_as_it_needs() {
    let mut dag = DagStore::<Vec<u8>>::new([0; 32]);
    dag.max_pending_bytes = 3 * pending_charge(&waiting_delta(1, 1024));
    for n in 1..=3 {
        let _ = dag
            .add_delta_with_outcome(waiting_delta(n, 1024), &NoopBytes)
            .await
            .unwrap();
    }

    let larger = dag
        .add_delta_with_outcome(waiting_delta(4, 2048), &NoopBytes)
        .await;

    assert!(larger.unwrap().is_pending());
    assert!(dag.pending_bytes <= dag.max_pending_bytes);
    assert!(!dag.has_delta(&[1; 32]) && !dag.has_delta(&[2; 32]));
    assert!(dag.has_delta(&[3; 32]) && dag.has_delta(&[4; 32]));
}

#[tokio::test]
async fn a_delta_larger_than_the_whole_pending_budget_is_refused() {
    let mut dag = DagStore::<Vec<u8>>::new([0; 32]);
    dag.max_pending_bytes = 2 * pending_charge(&waiting_delta(1, 16));
    let held = dag
        .add_delta_with_outcome(waiting_delta(1, 16), &NoopBytes)
        .await;
    assert!(held.unwrap().is_pending());

    let huge = dag
        .add_delta_with_outcome(waiting_delta(2, 4096), &NoopBytes)
        .await;

    assert!(matches!(huge, Err(DagError::PendingTooLarge { .. })));
    assert!(
        !dag.has_delta(&[2; 32]),
        "the oversized delta is not retained"
    );
    assert!(
        dag.has_delta(&[1; 32]),
        "nothing already pending is evicted for it"
    );
}

#[tokio::test]
async fn pending_bytes_are_released_when_deltas_leave() {
    let mut dag = DagStore::<Vec<u8>>::new([0; 32]);
    let child = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![0; 64]);
    let stale = waiting_delta(3, 64);
    let _ = dag.add_delta_with_outcome(child, &NoopBytes).await.unwrap();
    let _ = dag.add_delta_with_outcome(stale, &NoopBytes).await.unwrap();
    assert!(dag.pending_bytes > 0);

    let parent = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![0; 64]);
    let _ = dag
        .add_delta_with_outcome(parent, &NoopBytes)
        .await
        .unwrap();
    let _ = dag.cleanup_stale_since(Instant::now() + Duration::from_secs(1), Duration::ZERO);

    assert_eq!(dag.pending.len(), 0);
    assert_eq!(
        dag.pending_bytes, 0,
        "applied and expired deltas give their bytes back"
    );
}

/// A writer that caps a new delta's parents at the limit, leaving the other
/// heads for its next delta, is always accepted and still merges every head.
#[tokio::test]
async fn capped_parents_from_many_heads_still_converge() {
    let mut dag = DagStore::<()>::new([0; 32]);
    let concurrent = u32::try_from(MAX_DELTA_PARENTS).unwrap() + 44;
    for i in 1..=concurrent {
        let mut id = [0x11; 32];
        id[..4].copy_from_slice(&i.to_be_bytes());
        let _ = dag
            .add_delta(CausalDelta::new_test(id, vec![[0; 32]], ()), &Noop)
            .await
            .unwrap();
    }

    let mut merges = 0_u8;
    while dag.get_heads().len() > 1 {
        let mut parents = dag.get_heads();
        parents.truncate(MAX_DELTA_PARENTS);
        merges += 1;
        let merge = CausalDelta::new_test([0xF0 + merges; 32], parents, ());
        assert!(
            dag.add_delta(merge, &Noop).await.unwrap(),
            "a capped merge applies"
        );
    }
    assert_eq!(merges, 2, "the heads left out are merged by the next delta");
}
