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
    let _ = dag.add_delta_with_outcome(flood, &Noop).await;
    assert!(
        !dag.has_delta(&[2; 32]),
        "a delta's parent list must be bounded before it is buffered"
    );
}
