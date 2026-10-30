//! Time and randomness as the app sees them (spec section 5, D16, D19).

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

use crate::types::{Invocation, InvocationKind};

/// The wall clock in milliseconds.
///
/// Calls, timers, init, migrate and xcall delivery see the entry timestamp; views see the
/// timestamp of the applied entry they read at (the executor puts it in `entry_ts_ms`);
/// actions see the node's real clock rounded down to the second, except a compute run, which sees the
/// entry timestamp.
pub(crate) fn wall_ms(inv: &Invocation) -> u64 {
    if inv.kind != InvocationKind::Action || inv.compute.is_some() {
        return inv.entry_ts_ms;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    now_ms - now_ms % 1_000
}

/// The monotonic clock in nanoseconds: fixed for the whole invocation at the entry
/// timestamp (for an action, its start), so no elapsed time is visible to guest code.
pub(crate) fn monotonic_ns(inv: &Invocation) -> u64 {
    inv.entry_ts_ms.saturating_mul(1_000_000)
}

/// The run's random source. Logged invocations and compute runs draw from ChaCha20 seeded
/// with the run's seed, so the same seed gives the same bytes; views and other actions
/// draw from the node's CSPRNG, unrecorded.
pub(crate) fn entropy(inv: &Invocation) -> ChaCha20Rng {
    let seeded = match inv.kind {
        InvocationKind::View => false,
        InvocationKind::Action => inv.compute.is_some(),
        _ => true,
    };
    let mut seed = inv.seed;
    if !seeded {
        rand::rng().fill_bytes(&mut seed);
    }
    ChaCha20Rng::from_seed(seed)
}
