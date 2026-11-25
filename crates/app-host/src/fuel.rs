//! What host work costs in fuel. The guest pays fuel for its own instructions; a host
//! call pays for the work it makes the host do: a base cost per call plus a cost per byte or
//! per result. One fuel is about one nanosecond of host time, the rate a guest burns fuel at,
//! so a run's fuel bounds its host work as it bounds its own.
//!
//! The host core adds each call's cost to what the run owes ([`crate::HostCore::take_host_fuel`]);
//! the engine takes it out of the run's fuel when the call returns, and a run that cannot pay
//! ends out of fuel, as a guest that runs out does. A host made `with_host_fuel(false)`
//! charges nothing, as before this table: the node does so below the cluster version that
//! brings it.

/// Every charged call: a context or directory lookup, a blob's record, a file or xcall
/// handle, an event, the random generator.
pub const FUEL_HOST_CALL: u64 = 1_000;
/// A read or write of app state (`storage.*`, opening a cursor, each `range.next` batch):
/// a lookup in the store and the overlay.
pub const FUEL_STATE_ACCESS: u64 = 2_000;
/// Each entry a `range.next` batch returns.
pub const FUEL_PER_RANGE_ENTRY: u64 = 200;
/// Each byte a call moves between the host and the guest (keys, values, file contents,
/// arguments and answers of xcalls and HTTP, random bytes), or hashes.
pub const FUEL_PER_BYTE: u64 = 1;
/// One search query; with the three below, the least-squares fit of a query's host time
/// in `crates/search/bench` (its README, "Host time of a query").
pub const FUEL_SEARCH_QUERY: u64 = 5_000;
/// Each document a search matched (`total`), ranked or not.
pub const FUEL_PER_SEARCH_MATCH: u64 = 14;
/// Each hit a search returns.
pub const FUEL_PER_SEARCH_HIT: u64 = 10_500;
/// Each byte of a search's hits (keys and snippets).
pub const FUEL_PER_SEARCH_BYTE: u64 = 70;

/// Searches one run may make; past it `search.query` answers [`SEARCH_TOO_MANY`].
pub const MAX_SEARCHES_PER_RUN: u32 = 32;
/// The search error past [`MAX_SEARCHES_PER_RUN`].
pub const SEARCH_TOO_MANY: &str = "too many searches in one run";

/// The cost of `bytes` bytes.
#[must_use]
pub fn per_byte(bytes: usize) -> u64 {
    (bytes as u64).saturating_mul(FUEL_PER_BYTE)
}
