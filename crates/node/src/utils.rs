//! General-purpose utility functions.
//!
//! **Purpose**: Shared utilities used across the node crate.

use std::pin::pin;

use calimero_crypto::Nonce;
use calimero_primitives::application::ApplicationId;
use eyre::{bail, Result as EyreResult};
use futures_util::{Stream, StreamExt};
use rand::{thread_rng, Rng};

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
        if rng.gen_range(0..idx + 1) == 0 {
            item = Some(this);
        }
    }

    item
}

/// Generate a fresh random Nonce.
pub(crate) fn generate_nonce() -> Nonce {
    thread_rng().gen::<Nonce>()
}
