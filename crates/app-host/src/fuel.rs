//! What host work costs in fuel (C7). The guest pays fuel for its own instructions; a host
//! call pays for the work it makes the host do: a base cost per call plus a cost per byte or
//! per result. One fuel is about one nanosecond of host time, the rate a guest burns fuel at,
//! so a run's fuel bounds its host work as it bounds its own. The charges always apply.
//!
//! The engine takes [`FUEL_HOST_CALL`] out of the run's fuel as each host call starts, before
//! the host looks at its arguments. The host core adds the rest of the call's cost to what
//! the run owes ([`crate::HostCore::take_host_fuel`]), the bytes it was handed first, before
//! any check, so a refused or denied call pays for them too; the engine takes that out of
//! the run's fuel as the call returns. A run that cannot pay ends out of fuel, as a guest
//! that runs out does, and the guest never runs on fuel its host calls already owe.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Every host call: the engine takes it as the call starts.
pub const FUEL_HOST_CALL: u64 = 1_000;
/// A read or write of app state (`storage.*`, opening a cursor, each `range.next` batch), on
/// top of [`FUEL_HOST_CALL`]: a lookup in the store and the overlay.
pub const FUEL_STATE_ACCESS: u64 = 1_000;
/// Each entry a `range.next` batch returns.
pub const FUEL_PER_RANGE_ENTRY: u64 = 200;
/// Each byte a call moves between the host and the guest (keys, values, file contents,
/// arguments and answers of xcalls and HTTP, query text, stream writes and reads, random
/// bytes), or hashes.
pub const FUEL_PER_BYTE: u64 = 1;
/// One search that runs, on top of [`FUEL_HOST_CALL`]; with the three below, the
/// least-squares fit of a query's host time in `crates/search/bench` (its README, "Host time
/// of a query").
pub const FUEL_SEARCH_QUERY: u64 = 4_000;
/// Each document a search matched (`total`), ranked or not.
pub const FUEL_PER_SEARCH_MATCH: u64 = 14;
/// Each hit a search returns.
pub const FUEL_PER_SEARCH_HIT: u64 = 10_500;
/// Each byte of a search's hits (keys and snippets).
pub const FUEL_PER_SEARCH_BYTE: u64 = 70;

/// Searches one request may make, across the run and every view it reaches through
/// synchronous xcalls ([`SearchBudget`]); past it `search.query` answers [`SEARCH_TOO_MANY`].
pub const MAX_SEARCHES_PER_RUN: u32 = 32;
/// The search error past [`MAX_SEARCHES_PER_RUN`].
pub const SEARCH_TOO_MANY: &str = "too many searches in one run";

/// The cost of `bytes` bytes.
#[must_use]
pub fn per_byte(bytes: usize) -> u64 {
    (bytes as u64).saturating_mul(FUEL_PER_BYTE)
}

/// The searches one request has left, shared by its run and the views its synchronous xcalls
/// reach (`HostCore::with_search_budget`), so the request makes at most
/// [`MAX_SEARCHES_PER_RUN`] in all. A clone shares the count.
#[derive(Clone, Debug)]
pub struct SearchBudget(Arc<AtomicU32>);

impl Default for SearchBudget {
    fn default() -> Self {
        Self::with_left(MAX_SEARCHES_PER_RUN)
    }
}

impl SearchBudget {
    /// A budget of `left` searches (at most [`MAX_SEARCHES_PER_RUN`]): what a request
    /// forwarded from another node brings.
    #[must_use]
    pub fn with_left(left: u32) -> Self {
        Self(Arc::new(AtomicU32::new(left.min(MAX_SEARCHES_PER_RUN))))
    }

    /// Spends one search; false when none is left.
    pub fn take(&self) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
            .is_ok()
    }

    /// How many are left.
    #[must_use]
    pub fn left(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }

    /// Hands every search left to a view that runs elsewhere, which cannot give back what it
    /// did not use: this request has none left after it.
    pub fn take_all(&self) -> u32 {
        self.0.swap(0, Ordering::AcqRel)
    }
}
