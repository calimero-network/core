//! Test hooks (feature `testing`, never in a release build).

use std::sync::Mutex;
use std::time::Duration;

/// Nodes whose `/attestation` answers late, by public URL.
static SLOW_ATTESTATION: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

/// Make the node at `public_url` answer `/attestation` only after `delay`, as
/// a TDX node does while it fetches Intel's collateral, so a test sees peers
/// attest each other more slowly than a Raft RPC may take.
pub fn slow_attestation(public_url: &str, delay: Duration) {
    if let Ok(mut s) = SLOW_ATTESTATION.lock() {
        s.retain(|(u, _)| u != public_url);
        s.push((public_url.to_owned(), delay));
    }
}

/// The delay [`slow_attestation`] set for the node at `public_url`.
pub(crate) fn attestation_delay(public_url: &str) -> Option<Duration> {
    SLOW_ATTESTATION
        .lock()
        .ok()?
        .iter()
        .find(|(u, _)| u == public_url)
        .map(|(_, d)| *d)
}
