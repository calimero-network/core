use std::collections::hash_map::HashMap;

use libp2p::ping::Event;
use libp2p::swarm::ConnectionId;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::{debug, warn};

use super::{EventHandler, NetworkManager};

/// Consecutive ping failures tolerated on a single connection before we
/// force it closed.
///
/// The ping behaviour probes each connection on a fixed interval
/// (`ping::Config` default: every 15s, 20s timeout). A healthy connection
/// answers every probe; a silently-dead one (partition with no FIN/RST)
/// fails every probe in a row. Three consecutive failures is a deliberate
/// trade-off: low enough that a wedged connection is torn down inside the
/// ~120s sync-recovery budget the resilience workflows assume, high enough
/// that a single dropped datagram or a one-off negotiation hiccup on an
/// otherwise-live connection never costs us the connection. The counter
/// resets on the first success, so only *sustained* failure trips it.
///
/// Note the detection clock starts when the link goes dead, not when a
/// test heals the partition: failures accrue throughout the outage, so by
/// the time connectivity returns the connection is usually already at or
/// near the threshold and closes promptly, handing off to the
/// `ConnectionClosed` recovery cascade (see `swarm.rs`).
const MAX_PING_FAILURES: u32 = 3;

/// Fold one ping result into the per-connection failure tally and decide
/// whether the connection should now be force-closed.
///
/// Pure so the streak/threshold/reset behaviour can be exercised without a
/// live `Swarm`. A success clears the streak (`remove`); a failure bumps it
/// and, on reaching `max_failures`, removes the entry and returns `true` to
/// signal the caller to close the connection. Removing on the tripping
/// failure keeps the map from carrying a counter for a connection we're
/// about to tear down — the `ConnectionClosed` handler also prunes it, but
/// only once the close lands.
fn record_ping_result(
    failures: &mut HashMap<ConnectionId, u32>,
    connection: ConnectionId,
    succeeded: bool,
    max_failures: u32,
) -> bool {
    if succeeded {
        let _previous = failures.remove(&connection);
        return false;
    }

    let count = failures
        .entry(connection)
        .and_modify(|count| *count += 1)
        .or_insert(1);

    if *count >= max_failures {
        let _trip = failures.remove(&connection);
        true
    } else {
        false
    }
}

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        debug!("{}: {:?}", "ping".yellow(), event);

        let should_close = record_ping_result(
            &mut self.ping_failures,
            event.connection,
            event.result.is_ok(),
            MAX_PING_FAILURES,
        );

        if should_close {
            warn!(
                peer_id = %event.peer,
                connection_id = ?event.connection,
                failures = MAX_PING_FAILURES,
                "Closing connection after consecutive ping failures; \
                 link is silently dead, forcing recovery",
            );

            // Returns false if the connection is already gone — a benign
            // race we don't need to act on.
            let _closing = self.swarm.close_connection(event.connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: usize) -> ConnectionId {
        ConnectionId::new_unchecked(id)
    }

    #[test]
    fn failures_accumulate_then_trip_at_threshold() {
        let mut failures = HashMap::new();
        let c = conn(1);

        // First two failures arm but don't trip.
        assert!(!record_ping_result(&mut failures, c, false, 3));
        assert!(!record_ping_result(&mut failures, c, false, 3));
        assert_eq!(failures.get(&c), Some(&2));

        // Third trips and clears the entry so the about-to-close
        // connection doesn't leave a dangling counter.
        assert!(record_ping_result(&mut failures, c, false, 3));
        assert_eq!(failures.get(&c), None);
    }

    #[test]
    fn success_resets_the_streak() {
        let mut failures = HashMap::new();
        let c = conn(1);

        assert!(!record_ping_result(&mut failures, c, false, 3));
        assert!(!record_ping_result(&mut failures, c, false, 3));

        // A live probe wipes the streak entirely...
        assert!(!record_ping_result(&mut failures, c, true, 3));
        assert_eq!(failures.get(&c), None);

        // ...so the next failure starts from one and does NOT trip,
        // even though there were two failures before the success.
        assert!(!record_ping_result(&mut failures, c, false, 3));
        assert_eq!(failures.get(&c), Some(&1));
    }

    #[test]
    fn counters_are_independent_per_connection() {
        let mut failures = HashMap::new();
        let (a, b) = (conn(1), conn(2));

        // Hammer A to the brink; B stays untouched.
        assert!(!record_ping_result(&mut failures, a, false, 3));
        assert!(!record_ping_result(&mut failures, a, false, 3));
        assert!(!record_ping_result(&mut failures, b, false, 3));

        // A's third failure trips only A; B keeps its lone failure.
        assert!(record_ping_result(&mut failures, a, false, 3));
        assert_eq!(failures.get(&a), None);
        assert_eq!(failures.get(&b), Some(&1));
    }

    #[test]
    fn success_on_unknown_connection_is_a_noop() {
        let mut failures = HashMap::new();
        // A success for a connection we never recorded must not panic or
        // insert a zero — it simply has no streak to clear.
        assert!(!record_ping_result(&mut failures, conn(9), true, 3));
        assert!(failures.is_empty());
    }

    #[test]
    fn threshold_of_one_trips_on_first_failure() {
        // Guards against an off-by-one in the `>=` comparison: with a
        // threshold of one, the very first failure must trip.
        let mut failures = HashMap::new();
        assert!(record_ping_result(&mut failures, conn(1), false, 1));
        assert!(failures.is_empty());
    }
}
