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
    let mut stream = pin!(stream.enumerate());

    let mut item = None;

    // Algorithm R over the whole stream: the (idx+1)-th item (0-based `idx`)
    // replaces the reservoir with probability 1/(idx+1). The first item
    // (idx == 0) always fills the empty reservoir. Consuming the first element
    // *before* enumerating — as an earlier version did — offset every
    // probability by one and biased selection toward later elements.
    while let Some((idx, this)) = stream.next().await {
        if rng.gen_range(0..idx + 1) == 0 {
            item = Some(this);
        }
    }

    item
}
