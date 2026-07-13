//! General-purpose utility functions.
//!
//! **Purpose**: Shared utilities used across the node crate.

use std::pin::pin;

use futures_util::{Stream, StreamExt};
use rand::Rng;

/// Reservoir sampling: choose one random item from a stream.
///
/// Uses Algorithm R for uniform random selection from a stream of unknown length.
///
/// # Algorithm
/// - O(n) time complexity (single pass)
/// - O(1) space complexity
/// - Uniform distribution guarantee
///
/// # Example
/// ```ignore
/// let identities = context_client.get_context_members(&context_id, Some(true));
/// let chosen = choose_stream(identities, &mut rand::thread_rng()).await;
/// ```
pub(crate) async fn choose_stream<T>(
    stream: impl Stream<Item = T>,
    rng: &mut impl Rng,
) -> Option<T> {
    let mut stream = pin!(stream);

    let mut item = stream.next().await;

    let mut stream = stream.enumerate();

    while let Some((idx, this)) = stream.next().await {
        // The first element was consumed before `enumerate()`, so this is
        // overall item `idx + 2` (1-based) and must win with probability
        // 1/(idx + 2).
        if rng.gen_range(0..idx + 2) == 0 {
            item = Some(this);
        }
    }

    item
}

#[cfg(test)]
mod tests {
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;

    #[tokio::test]
    async fn empty_stream_yields_none() {
        let mut rng = StdRng::seed_from_u64(7);
        let chosen: Option<u32> = choose_stream(futures_util::stream::iter([]), &mut rng).await;
        assert_eq!(chosen, None);
    }

    #[tokio::test]
    async fn single_item_is_always_chosen() {
        let mut rng = StdRng::seed_from_u64(7);
        let chosen = choose_stream(futures_util::stream::iter([42u32]), &mut rng).await;
        assert_eq!(chosen, Some(42));
    }

    /// Regression test for the off-by-one that made the first element
    /// unpickable: the second element replaced the reservoir with
    /// probability 1 instead of 1/2, so element 0 was chosen with
    /// probability 0 for any stream of length >= 2.
    #[tokio::test]
    async fn selection_is_uniform() {
        const ITEMS: usize = 4;
        const ROUNDS: usize = 40_000;

        let mut rng = StdRng::seed_from_u64(7);
        let mut counts = [0usize; ITEMS];

        for _ in 0..ROUNDS {
            let chosen = choose_stream(futures_util::stream::iter(0..ITEMS), &mut rng)
                .await
                .expect("non-empty stream");
            counts[chosen] += 1;
        }

        // Expected ROUNDS / ITEMS = 10_000 per element; a fair sampler
        // stays within ±5% with overwhelming probability (~3.8 sigma),
        // while the broken one put 0 on element 0 and ~13_333 elsewhere.
        let expected = ROUNDS / ITEMS;
        let tolerance = expected / 20;
        for (elem, &count) in counts.iter().enumerate() {
            assert!(
                count.abs_diff(expected) <= tolerance,
                "element {elem} chosen {count} times, expected {expected} ± {tolerance}"
            );
        }
    }
}
